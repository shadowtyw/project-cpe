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

// ── 孤儿活跃通话清理（不再依赖 poll 轮询） ────────────────

/// 活跃通话最长保留时间：远超单次通话时长，超过则视为 CallRemoved 信号丢失。
const ACTIVE_CALL_TTL: Duration = Duration::from_secs(600);

/// 通话遥控每次登记新来电或被匹配结束时，顺便清理残留的过期活跃通话条目。
///
/// 此函数是 `call_cleanup_loop`（sms_listener.rs）的精确补充：
/// - `call_cleanup_loop` 每 60 秒全量扫描，覆盖"长时间无新通话"时的孤儿回收。
/// - 本函数在每次状态变更时顺带调用，零额外唤醒，消灭"上次通话异常挂断后，
///   下一个遥控来电到来时，残留条目阻塞白名单匹配"的窗口期。
fn purge_stale_active_call() {
    let mut state = lock_state();
    if state
        .active_call
        .as_ref()
        .is_some_and(|a| a.begin.elapsed() > ACTIVE_CALL_TTL)
    {
        if let Some(stale) = state.active_call.take() {
            tracing::warn!(
                number = %stale.number,
                path = %stale.path,
                "Stale call-control active call purged during incoming call handling"
            );
        }
    }
}

// ── 异步过期定时器（替代轮询） ────────────────────────────

/// 为待确认命令派生一次性过期定时器。
///
/// 在 `PendingCommand` 登记后立即派生一个 `tokio::spawn` 任务，精确休眠到
/// `expires_at` 时刻。唤醒后如果命令尚未被二次来电消耗，则执行过期清理。
/// 这彻底消除了此前为检查过期而维持的每 5 秒轮询协程——无来电时零唤醒。
///
/// 注意：如果同一号码在定时器触发前再次来电（二次确认成功），`on_incoming_call`
/// 会取出 `PendingCommand`，定时器唤醒后发现 `pending` 已为 `None`，直接退出。
fn spawn_expiry_timer(expires_at: Instant) {
    tokio::spawn(async move {
        let now = Instant::now();
        if expires_at > now {
            tokio::time::sleep(expires_at - now).await;
        }
        // 取出待确认命令（二次来电会提前消费掉 `pending`，此时为 None）。
        let expired = lock_state().pending.take();
        if let Some(pending) = expired {
            send_notification(&serde_json::json!({
                "event": "call_control_cancelled",
                "number": pending.number,
                "action": format!("{:?}", pending.action),
                "action_label": pending.action_label,
                "message": format!("命令已取消: {}（10秒内未收到二次确认来电）", pending.action_label),
            }));
        }
    });
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

    // 每次来电顺便清理一次残留的过期活跃通话条目，无需独立轮询任务。
    purge_stale_active_call();

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
    let expires_at = state.pending.as_ref().unwrap().expires_at;
    drop(state);

    // 派生一次性过期定时器，替代每 5 秒一次的轮询循环：定时器精确休眠到
    // expires_at 时刻才唤醒一次，无来电时协程零开销挂起。
    spawn_expiry_timer(expires_at);
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
    state.triggered_at = Some(crate::utils::beijing_now_rfc3339());
    state.triggered_action = Some(format!("{:?}", action).to_lowercase());
    state.triggered_number = Some(number.to_string());
}

fn send_notification(payload: &serde_json::Value) {
    // add timestamp
    let ts = crate::utils::beijing_now_rfc3339();
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