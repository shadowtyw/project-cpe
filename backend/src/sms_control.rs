//! 短信远程指令控制
//!
//! 当白名单号码（复用通话遥控的 `CallControlConfig.numbers`）发来特定格式的短信时，
//! 设备执行对应动作。用于无数据网络环境下的外部应急通道——用手机给设备发短信即可遥控。
//!
//! ## 支持的指令（中英文均可）
//! - `#STATUS#` / `#状态#` — 回复设备当前运行状态（信号强度、上网状态、运行时间等）
//! - `#REBOOT#` / `#重启#` — 延迟 3 秒重启系统
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

use std::time::Duration;

use tracing::{info, warn};
use zbus::Connection;

use crate::config::{normalize_phone_number, ConfigManager};

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
    match command {
        "STATUS" => build_status_reply(conn).await,
        "REBOOT" => {
            if crate::restart::schedule_reboot("sms_control", 3) {
                "[UDX710] 重启指令已收到，设备将在 3 秒后重启。".to_string()
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
                    spawn_airplane_recovery(conn);
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
                    spawn_airplane_recovery(conn);
                    "[UDX710] 射频已关闭，将在10秒后自动关闭飞行模式并恢复网络。".to_string()
                }
                Err(e) => format!("[UDX710] 关闭射频失败：{}", e),
            }
        }
        _ => "[UDX710] 未知指令。支持的命令请查看说明。".to_string(),
    }
}

/// 飞行模式自动恢复：10 秒后关闭飞行模式并开启数据连接。
///
/// 无论是短信遥控还是通话遥控触发的飞行模式，都通过此函数确保设备不因
/// 远程指令而永久断网。恢复操作在独立后台任务中执行，不阻塞主流程。
fn spawn_airplane_recovery(conn: &Connection) {
    let conn = conn.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        info!("Airplane mode auto-recovery: turning off airplane mode");
        let _ = crate::dbus::set_airplane_mode(&conn, false).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        info!("Airplane mode auto-recovery: enabling data connection");
        let _ = crate::dbus::set_data_connection(&conn, true).await;
        info!("Airplane mode auto-recovery completed");
    });
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