//! 短信远程指令控制
//!
//! 复用现有短信监听（`sms_listener`）读取到的短信，匹配口令前缀后执行对应动作。
//! 默认关闭；启用后短信内容需以配置的 `command_prefix` 开头，且（若配置了口令）
//! 携带正确口令才执行。通过短信实现无网环境下的远程管理（重启/飞行模式/状态查询）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::config::ConfigManager;
use crate::db::SmsMessage;
use zbus::Connection;

/// 上次触发的远程指令（供 GET 查询与页面展示）
pub struct TriggerState {
    pub triggered_at: Option<String>,
    pub command: Option<String>,
    pub from_number: Option<String>,
}

static TRIGGER_STATE: Mutex<TriggerState> = Mutex::new(TriggerState {
    triggered_at: None,
    command: None,
    from_number: None,
});

/// 用于在连续处理中避免重复回复的开关
static PROCESSING: AtomicBool = AtomicBool::new(false);

/// RAII guard to ensure PROCESSING flag is always reset, even on panic.
struct ProcessingGuard;

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        PROCESSING.store(false, Ordering::SeqCst);
    }
}

/// 处理一条新收到的短信，判断是否为远程控制指令。
/// 仅在”确实是控制指令且执行/回复成功”时返回 true。
pub async fn handle_incoming(
    conn: &Connection,
    config_manager: &ConfigManager,
    message: &SmsMessage,
) -> bool {
    if PROCESSING.swap(true, Ordering::SeqCst) {
        return false;
    }

    // Use RAII guard to ensure PROCESSING is always reset, even on panic
    let _guard = ProcessingGuard;

    let mut result = false;
    let _ = async {
        let config = config_manager.get_remote_control();
        if !config.enabled {
            return;
        }
        // 只响应 incoming
        if message.direction != "incoming" {
            return;
        }

        let content = message.content.trim();
        let Some(after_prefix) = content.strip_prefix(config.command_prefix.trim()) else {
            return;
        };
        let parts: Vec<&str> = after_prefix.split_whitespace().collect();

        // 口令校验：若配置了口令，则第一个参数必须匹配
        if !config.passcode.is_empty() {
            match parts.first() {
                Some(token) if *token == config.passcode.as_str() => {}
                _ => return,
            }
        }

        // 提取命令（跳过口令）
        let command_index = usize::from(!config.passcode.is_empty());
        let command = parts.get(command_index).copied().unwrap_or_default().to_lowercase();

        let action_message = match command.as_str() {
            "reboot" | "restart" => {
                let _ = crate::restart::schedule_reboot("remote_control", 3);
                Some("rebooting".to_string())
            }
            "airplane" | "flight" => {
                let _ = crate::dbus::set_airplane_mode(conn, true).await;
                Some("airplane mode on".to_string())
            }
            "airplane-off" | "flight-off" => {
                let _ = crate::dbus::set_airplane_mode(conn, false).await;
                Some("airplane mode off".to_string())
            }
            "data-on" => {
                let _ = crate::dbus::set_data_connection(conn, true).await;
                Some("data on".to_string())
            }
            "data-off" => {
                let _ = crate::dbus::set_data_connection(conn, false).await;
                Some("data off".to_string())
            }
            "status" => None, // 状态查询，在下方统一回复在线状态
            _ => return,
        };

        if let Some(ok) = action_message {
            record_trigger(&command, &message.phone_number);
            if config.reply {
                let reply = format!("[CPE] OK: {}", ok);
                let _ = crate::dbus::send_sms(conn, &message.phone_number, &reply).await;
            }
            result = true;
        } else if command == "status" {
            record_trigger(&command, &message.phone_number);
            if config.reply {
                let reply = build_status_reply(conn).await;
                let _ = crate::dbus::send_sms(conn, &message.phone_number, &reply).await;
            }
            result = true;
        }
    }
    .await;

    PROCESSING.store(false, Ordering::SeqCst);
    result
}

fn record_trigger(command: &str, from_number: &str) {
    let mut state = match TRIGGER_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.triggered_at = Some(chrono::Utc::now().to_rfc3339());
    state.command = Some(command.to_string());
    state.from_number = Some(from_number.to_string());
}

/// 读取上次触发的远程指令
pub fn get_last_trigger() -> crate::models::RemoteControlTrigger {
    let state = match TRIGGER_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    crate::models::RemoteControlTrigger {
        triggered_at: state.triggered_at.clone(),
        command: state.command.clone(),
        from_number: state.from_number.clone(),
    }
}

async fn build_status_reply(conn: &Connection) -> String {
    let online = match crate::dbus::get_airplane_mode(conn).await {
        Ok(am) => am.online,
        Err(_) => false,
    };
    let data_active = match crate::dbus::get_data_connection_status(conn).await {
        Ok(active) => active,
        Err(_) => false,
    };
    format!("[CPE] online={} data={}", online, data_active)
}