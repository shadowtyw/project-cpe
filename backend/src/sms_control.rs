//! 短信远程指令控制
//!
//! 当白名单号码（复用通话遥控的 `CallControlConfig.numbers`）发来特定格式的短信时，
//! 设备执行对应动作。用于无数据网络环境下的外部应急通道——用手机给设备发短信即可遥控。
//!
//! ## 支持的指令（中英文均可）
//! - `#STATUS#` / `#状态#` — 回复设备当前运行状态（信号强度、上网状态、运行时间等）
//! - `#REBOOT#` / `#重启#` — 延迟 10 秒重启系统（留时间给 webhook 推送）
//! - `#RECONNECT#` / `#重连#` — 重置数据连接（断开→重连）
//! - `#FLIGHTON#` / `#飞行开#` — 开启飞行模式
//! - `#FLIGHTOFF#` / `#飞行关#` — 关闭飞行模式
//! - `#DATAON#` / `#数据开#` — 开启数据连接
//! - `#DATAOFF#` / `#数据关#` — 关闭数据连接
//! - `#RADIOLTE#` / `#仅4G#` — 仅 4G
//! - `#RADIONR#` / `#仅5G#` — 仅 5G
//! - `#RADIOAUTO#` / `#自动#` — 4G/5G 自动
//! - `#RADIOOFF#` / `#关射频#` — 关闭射频
//!
//! ## 设计约束
//! - 白名单复用通话遥控的 `CallControlConfig.numbers`，两项功能共用同一组管理员号码。
//! - 短信遥控有独立的 `SmsControlConfig.enabled` 开关，与通话遥控互不干扰。
//! - 命中指令的短信不会触发 Webhook/短信推送转发，避免把控制指令泄漏到第三方平台。
//! - 每条指令执行后都会回复一条确认短信，耗费一条普通短信费。
//! - 开启飞行模式（#飞行开# / #关射频#）后 10 秒自动恢复网络，防止设备永久断网。

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use zbus::Connection;

use crate::config::{normalize_phone_number, ConfigManager};

// ── 通知发送器 trait（由 main.rs 注入） ──────────────────────

/// 短信遥控通知回调：接收 JSON 字符串推送到 Webhook 和短信推送平台。
pub trait SmsControlNotifier: Send + Sync {
    fn notify(&self, json_payload: &str);
}

/// 全局通知器，由 main.rs 在启动时设置一次。
static NOTIFIER: std::sync::OnceLock<Arc<dyn SmsControlNotifier>> = std::sync::OnceLock::new();

pub fn set_notifier(n: Arc<dyn SmsControlNotifier>) {
    let _ = NOTIFIER.set(n);
}

fn notify(payload: &serde_json::Value) {
    if let Some(n) = NOTIFIER.get() {
        if let Ok(json) = serde_json::to_string(payload) {
            n.notify(&json);
        }
    }
}

/// 指令映射：统一的管理名称 → (英文关键词, 中文关键词)
/// 英文关键词用于向后兼容，中文关键词用于用户易用性。
const COMMANDS: &[(&str, &[&str])] = &[
    ("STATUS", &["STATUS", "状态"]),
    ("REBOOT", &["REBOOT", "重启"]),
    ("RECONNECT", &["RECONNECT", "重连"]),
    ("FLIGHTON", &["FLIGHTON", "飞行开"]),
    ("FLIGHTOFF", &["FLIGHTOFF", "飞行关"]),
    ("DATAON", &["DATAON", "数据开"]),
    ("DATAOFF", &["DATAOFF", "数据关"]),
    ("RADIOLTE", &["RADIOLTE", "仅4G", "仅 4G", "仅4g"]),
    ("RADIONR", &["RADIONR", "仅5G", "仅 5G", "仅5g"]),
    ("RADIOAUTO", &["RADIOAUTO", "自动"]),
    ("RADIOOFF", &["RADIOOFF", "关射频"]),
];

/// 解析短信内容，返回命中的统一指令名，None 表示未命中。
/// 格式：`#关键字#`，忽略大小写和前后空白。
fn parse_command(content: &str) -> Option<&'static str> {
    let trimmed = content.trim();
    let upper = trimmed.to_uppercase();
    // 去掉前后的 # 号
    let inner = upper.strip_prefix('#').and_then(|s| s.strip_suffix('#'));
    let inner = match inner {
        Some(s) => s,
        None => return None,
    };
    if inner.is_empty() {
        return None;
    }
    for &(cmd_name, keywords) in COMMANDS {
        for kw in keywords {
            if inner == kw.to_uppercase() {
                return Some(cmd_name);
            }
        }
    }
    None
}

/// 检查号码是否在白名单中（复用 call_control 的号码匹配逻辑）。
fn is_whitelisted(numbers: &[String], sender: &str) -> bool {
    if sender.is_empty() {
        return false;
    }
    let normalized = normalize_phone_number(sender);
    if normalized.is_empty() {
        return false;
    }
    numbers.iter().any(|entry| {
        let entry = normalize_phone_number(entry);
        !entry.is_empty() && (normalized == entry || normalized.ends_with(&entry))
    })
}

/// 处理一条来信短信。
///
/// 如果命中了短信指令且发送者位于白名单，则执行对应动作并回复短信，返回 `true`。
/// 调用方应据此跳过 Webhook/推送转发。
///
/// 如果未命中指令、白名单未启用、或发送者不在白名单中，返回 `false`。
pub async fn handle_incoming_sms(
    conn: &Connection,
    config_manager: &ConfigManager,
    sender: &str,
    content: &str,
) -> bool {
    let sms_config = config_manager.get_sms_control();
    if !sms_config.enabled {
        return false;
    }

    let whitelist = config_manager.get_sms_control_whitelist();
    if whitelist.is_empty() {
        return false;
    }

    if !is_whitelisted(&whitelist, sender) {
        return false;
    }

    let command = match parse_command(content) {
        Some(cmd) => cmd,
        None => return false,
    };

    info!(
        command,
        sender,
        "SMS control: executing command from whitelisted number"
    );

    // 推送通知：告知管理员有人通过短信遥控执行了指令
    {
        let cmd_name = command_display_name(command);
        let ts = crate::utils::now_beijing_rfc3339();
        let msg = serde_json::json!({
            "timestamp": ts,
            "type": "sms_control",
            "data": {
                "event": "sms_command_executed",
                "command": command,
                "command_name": cmd_name,
                "source": sender,
                "message": format!("短信遥控：号码 {} 执行了「{}」指令", sender, cmd_name),
            },
        });
        notify(&msg);
    }

    let reply = execute_command(conn, command).await;

    // 发送确认短信回复管理员
    match crate::dbus::send_sms(conn, sender, &reply).await {
        Ok(_) => info!(sender, command, "SMS control: reply sent"),
        Err(e) => warn!(error = %e, sender, command, "SMS control: failed to send reply SMS"),
    }

    true
}

/// 将指令常量翻译为中文显示名（用于 webhook/推送通知）
fn command_display_name(cmd: &str) -> &str {
    match cmd {
        "STATUS" => "查询状态",
        "REBOOT" => "重启设备",
        "RECONNECT" => "重置数据连接",
        "FLIGHTON" => "开启飞行模式",
        "FLIGHTOFF" => "关闭飞行模式",
        "DATAON" => "开启数据连接",
        "DATAOFF" => "关闭数据连接",
        "RADIOLTE" => "切换仅 4G",
        "RADIONR" => "切换仅 5G",
        "RADIOAUTO" => "切换自动模式",
        "RADIOOFF" => "关闭射频",
        _ => "未知指令",
    }
}

/// 执行指令并返回回复短信内容。
async fn execute_command(conn: &Connection, command: &str) -> String {
    match command {
        "STATUS" => build_status_reply(conn).await,
        "REBOOT" => {
            if crate::restart::schedule_reboot("sms_control", 10) {
                "[UDX710] 重启指令已收到，设备将在 10 秒后重启。".to_string()
            } else {
                "[UDX710] 重启指令被拒绝：设备可能正在处理其他重启请求。".to_string()
            }
        }
        "RECONNECT" => {
            warn!("SMS control: reconnecting data connection");
            let _ = crate::dbus::set_data_connection(conn, false).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            match crate::dbus::set_data_connection(conn, true).await {
                Ok(()) => "[UDX710] 数据连接已重置，请稍候查看网络状态。".to_string(),
                Err(e) => format!("[UDX710] 数据连接重置失败：{}", e),
            }
        }
        "FLIGHTON" => {
            match crate::dbus::set_airplane_mode(conn, true).await {
                Ok(()) => {
                    crate::dbus::spawn_airplane_recovery(conn);
                    "[UDX710] 飞行模式已开启，将在10秒后自动关闭并恢复网络。".to_string()
                }
                Err(e) => format!("[UDX710] 开启飞行模式失败：{}", e),
            }
        }
        "FLIGHTOFF" => {
            match crate::dbus::set_airplane_mode(conn, false).await {
                Ok(()) => "[UDX710] 飞行模式已关闭。".to_string(),
                Err(e) => format!("[UDX710] 关闭飞行模式失败：{}", e),
            }
        }
        "DATAON" => {
            match crate::dbus::set_data_connection(conn, true).await {
                Ok(()) => "[UDX710] 数据连接已开启。".to_string(),
                Err(e) => format!("[UDX710] 开启数据连接失败：{}", e),
            }
        }
        "DATAOFF" => {
            match crate::dbus::set_data_connection(conn, false).await {
                Ok(()) => "[UDX710] 数据连接已关闭。".to_string(),
                Err(e) => format!("[UDX710] 关闭数据连接失败：{}", e),
            }
        }
        "RADIOLTE" => {
            match crate::dbus::set_radio_mode(conn, crate::models::RadioMode::LteOnly).await {
                Ok(()) => "[UDX710] 已切换为仅 4G 模式。".to_string(),
                Err(e) => format!("[UDX710] 切换仅 4G 模式失败：{}", e),
            }
        }
        "RADIONR" => {
            match crate::dbus::set_radio_mode(conn, crate::models::RadioMode::NrOnly).await {
                Ok(()) => "[UDX710] 已切换为仅 5G 模式。".to_string(),
                Err(e) => format!("[UDX710] 切换仅 5G 模式失败：{}", e),
            }
        }
        "RADIOAUTO" => {
            match crate::dbus::set_radio_mode(conn, crate::models::RadioMode::Auto).await {
                Ok(()) => "[UDX710] 已切换为 4G/5G 自动模式。".to_string(),
                Err(e) => format!("[UDX710] 切换自动模式失败：{}", e),
            }
        }
        "RADIOOFF" => {
            match crate::dbus::set_airplane_mode(conn, true).await {
                Ok(()) => {
                    crate::dbus::spawn_airplane_recovery(conn);
                    "[UDX710] 射频已关闭，将在10秒后自动关闭飞行模式并恢复网络。".to_string()
                }
                Err(e) => format!("[UDX710] 关闭射频失败：{}", e),
            }
        }
        _ => "[UDX710] 未知指令。支持的命令请查看说明。".to_string(),
    }
}

/// 飞行模式自动恢复已统一到 `dbus::spawn_airplane_recovery`（短信/通话遥控共用）。

/// 构造设备状态回复短信。
///
/// 复用 [`crate::device_report::DeviceReport`]，与 MQTT `status` 指令同源，
/// 保证「短信看到的」和「MQTT 推送看到的」是同一份数据：内存以可用百分比为主指标，
/// 并补齐了旧版缺失的 CPU 占用率与设备温度。短信版走 `format_sms`（精简纯文本），
/// 避免 emoji 与逐传感器温度让短信过长。
async fn build_status_reply(conn: &Connection) -> String {
    crate::device_report::DeviceReport::collect(conn)
        .await
        .format_sms()
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 英文指令（向后兼容）===

    #[test]
    fn parse_reboot_en() {
        assert_eq!(parse_command("#REBOOT#"), Some("REBOOT"));
        assert_eq!(parse_command("#reboot#"), Some("REBOOT"));
        assert_eq!(parse_command("  #REBOOT#  "), Some("REBOOT"));
    }

    #[test]
    fn parse_reconnect_en() {
        assert_eq!(parse_command("#RECONNECT#"), Some("RECONNECT"));
        assert_eq!(parse_command("#reconnect#"), Some("RECONNECT"));
    }

    #[test]
    fn parse_status_en() {
        assert_eq!(parse_command("#STATUS#"), Some("STATUS"));
        assert_eq!(parse_command("#status#"), Some("STATUS"));
    }

    // === 中文指令 ===

    #[test]
    fn parse_reboot_cn() {
        assert_eq!(parse_command("#重启#"), Some("REBOOT"));
    }

    #[test]
    fn parse_reconnect_cn() {
        assert_eq!(parse_command("#重连#"), Some("RECONNECT"));
    }

    #[test]
    fn parse_status_cn() {
        assert_eq!(parse_command("#状态#"), Some("STATUS"));
    }

    #[test]
    fn parse_flight_on_cn() {
        assert_eq!(parse_command("#飞行开#"), Some("FLIGHTON"));
    }

    #[test]
    fn parse_flight_off_cn() {
        assert_eq!(parse_command("#飞行关#"), Some("FLIGHTOFF"));
    }

    #[test]
    fn parse_data_on_cn() {
        assert_eq!(parse_command("#数据开#"), Some("DATAON"));
    }

    #[test]
    fn parse_data_off_cn() {
        assert_eq!(parse_command("#数据关#"), Some("DATAOFF"));
    }

    #[test]
    fn parse_radio_lte_cn() {
        assert_eq!(parse_command("#仅4G#"), Some("RADIOLTE"));
        assert_eq!(parse_command("#仅4g#"), Some("RADIOLTE"));
    }

    #[test]
    fn parse_radio_nr_cn() {
        assert_eq!(parse_command("#仅5G#"), Some("RADIONR"));
    }

    #[test]
    fn parse_radio_auto_cn() {
        assert_eq!(parse_command("#自动#"), Some("RADIOAUTO"));
    }

    #[test]
    fn parse_radio_off_cn() {
        assert_eq!(parse_command("#关射频#"), Some("RADIOOFF"));
    }

    // === 非法输入 ===

    #[test]
    fn parse_non_command() {
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("REBOOT"), None);
        assert_eq!(parse_command("#REBOOT"), None);
        assert_eq!(parse_command("REBOOT#"), None);
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("# #"), None);
        assert_eq!(parse_command("##"), None);
    }
}