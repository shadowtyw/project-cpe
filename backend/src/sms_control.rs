//! 短信远程指令控制
//!
//! 当白名单号码（复用通话遥控的 `CallControlConfig.numbers`）发来特定格式的短信时，
//! 设备执行对应动作。用于无数据网络环境下的外部应急通道——用手机给设备发短信即可遥控。
//!
//! ## 支持的指令
//! - `#REBOOT#` — 延迟 3 秒重启系统
//! - `#RECONNECT#` — 重置数据连接（断开→重连）
//! - `#STATUS#` — 回复设备当前运行状态（信号强度、上网状态、运行时间等）
//!
//! ## 设计约束
//! - 白名单复用通话遥控的 `CallControlConfig.numbers`，两项功能共用同一组管理员号码。
//! - 短信遥控有独立的 `SmsControlConfig.enabled` 开关，与通话遥控互不干扰。
//! - 命中指令的短信不会触发 Webhook/短信推送转发，避免把控制指令泄漏到第三方平台。
//! - 每条指令执行后都会回复一条确认短信，耗费一条普通短信费。

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use zbus::Connection;

use crate::config::{normalize_phone_number, ConfigManager};

/// 短信指令关键字（不含前后 # 号）。匹配时忽略大小写，但要求前后各有一个 `#`。
const COMMAND_REBOOT: &str = "REBOOT";
const COMMAND_RECONNECT: &str = "RECONNECT";
const COMMAND_STATUS: &str = "STATUS";

/// 解析短信内容，返回命中的指令（不含 # 号），大小写敏感匹配失败返回 None。
fn parse_command(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    for cmd in &[COMMAND_REBOOT, COMMAND_RECONNECT, COMMAND_STATUS] {
        let pattern = format!("#{}#", cmd);
        // 忽略大小写比较
        if trimmed.len() >= pattern.len() {
            let upper = trimmed.to_uppercase();
            if upper == pattern.to_uppercase() {
                return Some(cmd);
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

    let reply = execute_command(conn, command).await;

    // 发送确认短信回复管理员
    match crate::dbus::send_sms(conn, sender, &reply).await {
        Ok(_) => info!(sender, command, "SMS control: reply sent"),
        Err(e) => warn!(error = %e, sender, command, "SMS control: failed to send reply SMS"),
    }

    true
}

/// 执行指令并返回回复短信内容。
async fn execute_command(conn: &Connection, command: &str) -> String {
    match command.to_uppercase().as_str() {
        COMMAND_REBOOT => {
            if crate::restart::schedule_reboot("sms_control", 3) {
                "[UDX710] 重启指令已收到，设备将在 3 秒后重启。".to_string()
            } else {
                "[UDX710] 重启指令被拒绝：设备可能正在处理其他重启请求。".to_string()
            }
        }
        COMMAND_RECONNECT => {
            warn!("SMS control: reconnecting data connection");
            let _ = crate::dbus::set_data_connection(conn, false).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            match crate::dbus::set_data_connection(conn, true).await {
                Ok(()) => "[UDX710] 数据连接已重置，请稍候查看网络状态。".to_string(),
                Err(e) => format!("[UDX710] 数据连接重置失败：{}", e),
            }
        }
        COMMAND_STATUS => {
            build_status_reply(conn).await
        }
        _ => "[UDX710] 未知指令。支持：#REBOOT# #RECONNECT# #STATUS#".to_string(),
    }
}

/// 构造设备状态回复短信。
async fn build_status_reply(conn: &Connection) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("[UDX710] 设备状态报告".to_string());

    // 1. 注册状态
    let reg_status = crate::dbus::get_registration_status(conn).await;
    lines.push(format!("网络: {}", reg_status.as_deref().unwrap_or("unknown")));

    // 2. 信号强度
    match crate::dbus::get_signal_strength(conn).await {
        Ok(sig) => lines.push(format!("信号: {}dBm", sig.strength)),
        Err(_) => lines.push("信号: 获取失败".to_string()),
    }

    // 3. 数据连接状态
    match crate::dbus::get_data_connection_status(conn).await {
        Ok(active) => lines.push(format!("上网: {}", if active { "已连接" } else { "未连接" })),
        Err(_) => lines.push("上网: 获取失败".to_string()),
    }

    // 4. 运行时间
    let uptime = read_uptime_seconds();
    lines.push(format!("运行: {}分钟", uptime / 60));

    // 5. 内存
    let mem_avail = read_mem_available_kb();
    lines.push(format!("内存: {}MB 可用", mem_avail / 1024));

    lines.join("\n")
}

fn read_uptime_seconds() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
                .map(|v| v as u64)
        })
        .unwrap_or(0)
}

fn read_mem_available_kb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|line| line.starts_with("MemAvailable:"))
                .and_then(|line| {
                    line.split_whitespace()
                        .nth(1)?
                        .parse::<u64>()
                        .ok()
                })
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reboot() {
        assert_eq!(parse_command("#REBOOT#"), Some("REBOOT"));
        assert_eq!(parse_command("#reboot#"), Some("REBOOT"));
        assert_eq!(parse_command("  #REBOOT#  "), Some("REBOOT"));
    }

    #[test]
    fn parse_reconnect() {
        assert_eq!(parse_command("#RECONNECT#"), Some("RECONNECT"));
        assert_eq!(parse_command("#reconnect#"), Some("RECONNECT"));
    }

    #[test]
    fn parse_status() {
        assert_eq!(parse_command("#STATUS#"), Some("STATUS"));
        assert_eq!(parse_command("#status#"), Some("STATUS"));
    }

    #[test]
    fn parse_non_command() {
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("REBOOT"), None);
        assert_eq!(parse_command("#REBOOT"), None);
        assert_eq!(parse_command("REBOOT#"), None);
        assert_eq!(parse_command(""), None);
    }
}