//! 通话远程指令控制 — 通话时长编码 + 二次呼叫确认
//!
//! ## 交互流程
//!
//! 1. **第一次来电**：白名单号码来电 → 自动接听，开始计时。用户保持通话 N 秒后挂断。
//! 2. **命令识别**：根据通话时长在配置的 `duration_commands` 中匹配（±2s 容差），
//!    识别出对应的动作（如 5s=重启、10s=飞行模式）。
//! 3. **通知推送**：通过 Webhook/短信推送发送"检测到命令，10 秒内再次来电确认执行"。
//! 4. **确认窗口**：10 秒倒计时开始，等待二次来电确认。
//! 5. **二次来电**：同一号码在 10 秒内再次拨入 → 确认并执行动作，推送执行结果。
//! 6. **超时取消**：10 秒内无二次来电 → 推送"命令已取消"，丢弃待确认命令。
//!
//! ## 设计约束
//! - 同一时刻最多一个待确认命令（后到的覆盖先到的）。
//! - 重启类动作执行后设备自然离线，无需额外清理。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::config::{normalize_phone_number, CallControlConfig, ScheduleAction};
use zbus::Connection;

// ── 通知发送器 trait（由 main.rs 注入） ──────────────────────

/// 通话遥控通知回调：接收 JSON 字符串推送到 Webhook 和短信推送平台。
pub trait CallControlNotifier: Send + Sync {
    fn notify(&self, json_payload: &str);
}

/// 全局通知器，由 main.rs 在启动时设置一次。
static NOTIFIER: std::sync::OnceLock<Arc<dyn CallControlNotifier>> = std::sync::OnceLock::new();

pub fn set_notifier(n: Arc<dyn CallControlNotifier>) {
    let _ = NOTIFIER.set(n);
}

fn notify(payload: &serde_json::Value) {
    if let Some(n) = NOTIFIER.get() {
        if let Ok(json) = serde_json::to_string(payload) {
            n.notify(&json);
        }
    }
}

// ── 状态机 ────────────────────────────────────────────────

/// 活跃通话（正在计时的白名单来电）。
struct ActiveCall {
    number: String,
    path: String,
    begin: Instant,
}

/// 待确认命令。
struct PendingCommand {
    number: String,
    action: ScheduleAction,
    action_label: String,
    #[allow(dead_code)]
    duration_secs: u64,
    expires_at: Instant,
}

struct State {
    active_call: Option<ActiveCall>,
    pending: Option<PendingCommand>,
    /// 最近一次触发的记录（供 GET 查询）
    triggered_at: Option<String>,
    triggered_action: Option<String>,
    triggered_number: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    active_call: None,
    pending: None,
    triggered_at: None,
    triggered_action: None,
    triggered_number: None,
});

fn lock_state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

// ── 公开 API ──────────────────────────────────────────────

/// 处理一条新的来电：命中白名单则自动接听并开始计时。
///
/// 所有 D-Bus 调用（接听、执行动作、挂断）都在独立任务中进行，函数本身只做
/// 同步的状态登记后立即返回。原因：本函数由 `call_listener` 的 D-Bus 消息循环
/// 调用，而 D-Bus 操作需等待全局串口锁（`serial::DBUS_LOCK`），最长可达 30s，
/// 运营商扫描期间甚至 120s。若在循环里 `await`，后续的 `CallRemoved` 信号虽不会
/// 丢失（zbus 缓冲），但处理会被推迟，而通话时长由 `Instant::now() - begin` 计算，
/// 会被这段延迟灌水，导致通话遥控的时长匹配失准。
pub async fn on_incoming_call(
    conn: &Connection,
    config: &CallControlConfig,
    path: &str,
    number: &str,
) -> bool {
    if !config.enabled || config.duration_commands.is_empty() || config.numbers.is_empty() {
        return false;
    }

    let normalized = normalize_phone_number(number);
    if normalized.is_empty() {
        return false;
    }
    if !config.numbers.iter().any(|entry| {
        let entry = normalize_phone_number(entry);
        !entry.is_empty() && (normalized == entry || normalized.ends_with(&entry))
    }) {
        return false;
    }

    // 检查是否为二次确认来电
    let is_confirmation = { lock_state().pending.as_ref().is_some_and(|p| p.number == normalized) };

    if !is_confirmation {
        // 首次通话：同步登记活跃通话（保证后续 CallRemoved 一定能匹配到），
        // 接听动作放到后台任务，不阻塞消息循环。
        {
            let mut state = lock_state();
            state.active_call = Some(ActiveCall {
                number: normalized,
                path: path.to_string(),
                begin: Instant::now(),
            });
        }
        let conn = conn.clone();
        let path = path.to_string();
        tokio::spawn(async move {
            let _ = crate::dbus::answer_call(&conn, &path).await;
        });
        return true;
    }

    // 二次来电确认：取出待确认命令
    let pending = match lock_state().pending.take() {
        Some(p) if p.number == normalized => p,
        _ => return false,
    };

    let action = pending.action;
    let label = pending.action_label;
    let number = pending.number;

    send_notification(&serde_json::json!({
        "event": "call_control_confirmed",
        "number": number,
        "action": format!("{:?}", action),
        "action_label": label,
        "message": format!("命令已确认: {}，开始执行", label),
    }));

    record_trigger(&number, action);

    // 执行动作与挂断同样放到后台任务：execute_action 内部可能触发射频切换等
    // 较慢的 D-Bus 操作，不能阻塞消息循环。
    let conn = conn.clone();
    let path = path.to_string();
    tokio::spawn(async move {
        execute_action(&conn, action).await;
        let _ = crate::dbus::hangup_call(&conn, &path).await;
    });

    true
}

/// 通话结束：测量通话时长，匹配命令，开始确认倒计时。
pub async fn on_call_removed(_conn: &Connection, config: &CallControlConfig, path: &str) {
    let mut state = lock_state();

    // 确认这不是旧的活跃通话的残留信号
    let active = match state.active_call.take() {
        Some(a) if a.path == path => a,
        _ => return,
    };

    let duration = active.begin.elapsed().as_secs();
    drop(state);

    // 太短（<3s）或太长（>30s）视为误操作
    if duration < 3 || duration > 30 {
        return;
    }

    // 在 duration_commands 中匹配（±2s 容差）
    let matched = config.duration_commands.iter().find(|cmd| {
        let lower = cmd.duration_secs.saturating_sub(2);
        let upper = cmd.duration_secs.saturating_add(2);
        duration >= lower && duration <= upper
    });

    let cmd = match matched {
        Some(c) => c,
        None => return,
    };

    let label = if cmd.label.is_empty() {
        format!("{:?}", cmd.action)
    } else {
        cmd.label.clone()
    };

    // 记录触发状态：检测到命令即视为一次触发，前端"上次触发状态"据此显示
    record_trigger(&active.number, cmd.action);

    // 通知：检测到命令
    send_notification(&serde_json::json!({
        "event": "call_control_detected",
        "number": active.number.clone(),
        "duration_secs": duration,
        "action": format!("{:?}", cmd.action),
        "action_label": label,
        "message": format!("检测到遥控命令: {} (通话{}秒)，请在10秒内再次来电确认", label, duration),
    }));

    let mut state = lock_state();
    state.pending = Some(PendingCommand {
        number: active.number,
        action: cmd.action,
        action_label: label,
        duration_secs: duration,
        expires_at: Instant::now() + Duration::from_secs(10),
    });
}

/// 定时轮询：检查待确认命令是否过期，并清理超时的活跃通话残留。
///
/// 返回下次需要检查的等待时长；`None` 表示当前无任何计时状态，调用方可长时间
/// 休眠（避免 1 秒一次的无谓唤醒，降低嵌入式设备 CPU/功耗占用）。
pub async fn poll() -> Option<Duration> {
    /// 活跃通话最长保留时间：远超单次通话时长，超过则视为 CallRemoved 信号丢失。
    const ACTIVE_CALL_TTL: Duration = Duration::from_secs(600);

    let expired = {
        let mut state = lock_state();

        // 清理孤儿活跃通话（CallRemoved 信号丢失时防止状态残留，阻塞后续判断）。
        if state.active_call.as_ref().is_some_and(|a| a.begin.elapsed() > ACTIVE_CALL_TTL) {
            if let Some(stale) = state.active_call.take() {
                tracing::warn!(
                    number = %stale.number,
                    "Stale call-control active call cleared after {}s",
                    ACTIVE_CALL_TTL.as_secs()
                );
            }
        }

        match state.pending.as_ref() {
            Some(p) if p.expires_at > Instant::now() => {
                // 未到期：精确休眠到到期时刻。
                return Some(p.expires_at - Instant::now());
            }
            Some(_) => state.pending.take(),
            // 无待确认命令：若还有活跃通话在计时，按其 TTL 兜底唤醒一次即可。
            None => {
                return state.active_call.as_ref().map(|a| {
                    ACTIVE_CALL_TTL
                        .checked_sub(a.begin.elapsed())
                        .unwrap_or(Duration::from_secs(1))
                });
            }
        }
    };

    if let Some(pending) = expired {
        send_notification(&serde_json::json!({
            "event": "call_control_cancelled",
            "number": pending.number,
            "action": format!("{:?}", pending.action),
            "action_label": pending.action_label,
            "message": format!("命令已取消: {}（10秒内未收到二次确认来电）", pending.action_label),
        }));
    }

    None
}

/// 读取上次触发的通话遥控记录。
pub fn get_last_trigger() -> crate::models::CallControlTrigger {
    let state = lock_state();
    crate::models::CallControlTrigger {
        triggered_at: state.triggered_at.clone(),
        action: state.triggered_action.clone(),
        from_number: state.triggered_number.clone(),
    }
}

// ── 内部 ──────────────────────────────────────────────────

fn record_trigger(number: &str, action: ScheduleAction) {
    let mut state = lock_state();
    state.triggered_at = Some(crate::utils::now_beijing_rfc3339());
    state.triggered_action = Some(format!("{:?}", action).to_lowercase());
    state.triggered_number = Some(number.to_string());
}

fn send_notification(payload: &serde_json::Value) {
    // add timestamp
    let ts = crate::utils::now_beijing_rfc3339();
    let msg = serde_json::json!({
        "timestamp": ts,
        "type": "call_control",
        "data": payload,
    });
    notify(&msg);
}

async fn execute_action(conn: &Connection, action: ScheduleAction) {
    match action {
        ScheduleAction::Reboot => {
            let _ = crate::restart::schedule_reboot("call_control", 10);
        }
        ScheduleAction::AirplaneOn => {
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
            crate::dbus::spawn_airplane_recovery(conn);
        }
        ScheduleAction::AirplaneOff => {
            let _ = crate::dbus::set_airplane_mode(conn, false).await;
        }
        ScheduleAction::DataOn => {
            let _ = crate::dbus::set_data_connection(conn, true).await;
        }
        ScheduleAction::DataOff => {
            let _ = crate::dbus::set_data_connection(conn, false).await;
        }
        ScheduleAction::RadioLte => {
            let _ = crate::dbus::set_radio_mode(conn, crate::models::RadioMode::LteOnly).await;
        }
        ScheduleAction::RadioNr => {
            let _ = crate::dbus::set_radio_mode(conn, crate::models::RadioMode::NrOnly).await;
        }
        ScheduleAction::RadioAuto => {
            let _ = crate::dbus::set_radio_mode(conn, crate::models::RadioMode::Auto).await;
        }
        ScheduleAction::RadioOff => {
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
            crate::dbus::spawn_airplane_recovery(conn);
        }
    }
}

// 飞行模式自动恢复已统一到 dbus::spawn_airplane_recovery（短信/通话遥控共用）。