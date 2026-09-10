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
#[allow(dead_code)]
struct PendingCommand {
    number: String,
    action: ScheduleAction,
    action_label: String,
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
    let is_confirmation = { lock_state().pending.as_ref().map_or(false, |p| p.number == normalized) };

    if !is_confirmation {
        // 首次通话：记录活跃通话并自动接听
        {
            let mut state = lock_state();
            state.active_call = Some(ActiveCall {
                number: normalized,
                path: path.to_string(),
                begin: Instant::now(),
            });
        }
        let _ = crate::dbus::answer_call(conn, path).await;
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
    execute_action(conn, action).await;

    // 挂断确认来电
    let _ = crate::dbus::hangup_call(conn, path).await;
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

    // 通知：检测到命令
    send_notification(&serde_json::json!({
        "event": "call_control_detected",
        "number": active.number,
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

/// 定时轮询：检查待确认命令是否过期。
pub async fn poll() {
    let expired = {
        let mut state = lock_state();
        let now = Instant::now();
        if state.pending.as_ref().map_or(true, |p| p.expires_at > now) {
            return;
        }
        state.pending.take()
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
    state.triggered_at = Some(chrono::Utc::now().to_rfc3339());
    state.triggered_action = Some(format!("{:?}", action).to_lowercase());
    state.triggered_number = Some(number.to_string());
}

fn send_notification(payload: &serde_json::Value) {
    // add timestamp
    let ts = chrono::Utc::now().to_rfc3339();
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
            let _ = crate::restart::schedule_reboot("call_control", 3);
        }
        ScheduleAction::AirplaneOn => {
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
            spawn_airplane_recovery(conn);
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
            spawn_airplane_recovery(conn);
        }
    }
}

fn spawn_airplane_recovery(conn: &Connection) {
    let conn = conn.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        tracing::info!("Call control airplane auto-recovery");
        let _ = crate::dbus::set_airplane_mode(&conn, false).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = crate::dbus::set_data_connection(&conn, true).await;
    });
}