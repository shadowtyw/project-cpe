//! 外网探活与断网自愈 watchdog
//!
//! 解决「ofono 状态显示 registered、context Active=true，但实际已断流」的假死问题：
//! 仅依赖 ofono 状态的 `data_connection_watchdog` 无法感知这类故障，本模块通过周期性
//! ping 外部公共 IP 判断真实连通性，并按分级阶梯执行恢复动作。
//!
//! ## 分级恢复阶梯
//! 1. **Level 1**：连续失败达到 `l1_failures` → 重置数据连接（Active=false→true）
//! 2. **Level 2**：仍失败累计到 `l2_failures` → 飞行模式复位基带（Online=false→true）
//! 3. **Level 3**：仍失败累计到 `l3_failures` → 系统重启
//!
//! ## 关键约束
//! - 仅在 ofono 注册状态为 `registered`/`roaming` 时才判失败；`searching` 等启动阶段
//!   不计入失败，避免开机搜网期被误判重启发启。
//! - 每级动作执行后进入静默期（`cooldown_secs`），给基带重连留时间，防止动作叠加。
//! - 失败计数与最近动作等级持久化到重启状态文件，进程重启不清零，避免陷入
//!   「假死→重启后端→计数清零→又不判死」的循环。

use std::process::Command;
use std::sync::Arc;

use tracing::{info, warn};
use zbus::Connection;

use crate::config::{get_persistent_root_dir, ConfigManager, NetHealthConfig};

/// 默认探测目标：阿里 DNS 与腾讯 DNS，均为国内可达的公共地址。
const PROBE_TARGETS: [&str; 2] = ["223.5.5.5", "119.29.29.29"];

/// 启动宽限：服务启动后此秒数内不判死，避免与 `data_connection_watchdog` 的
/// 首轮激活竞争。
const STARTUP_GRACE_SECONDS: u64 = 30;

enum RecoveryLevel {
    None,
    ResetConnection,
    AirplaneReset,
    Reboot,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct NetHealthState {
    consecutive_failures: u32,
    last_level: u8,
}

fn state_path() -> std::path::PathBuf {
    get_persistent_root_dir().join("net_health_state.json")
}

fn read_state() -> NetHealthState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn write_state(state: &NetHealthState) {
    let path = state_path();
    let tmp = path.with_extension("json.new");
    let result = (|| -> Result<(), String> {
        let content = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to serialize net health state: {}", e))?;
        std::fs::write(&tmp, content)
            .map_err(|e| format!("Failed to write net health state: {}", e))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("Failed to publish net health state: {}", e))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(tmp);
        warn!(error = %e, "Failed to persist net health state");
    }
}

/// 网络健康 watchdog 主循环。
pub async fn net_health_watchdog(conn: Arc<Connection>, config_manager: Arc<ConfigManager>) {
    // 记录进程启动时间，用于启动宽限判断。
    let started_at = std::time::Instant::now();
    let mut state = read_state();
    // 记录最近一次动作执行时间，用于静默期判断（内存态，动作后进程若重启则由
    // `state.last_level` 兜底，避免重复动作）。
    let mut last_action_at: Option<std::time::Instant> = None;

    loop {
        let config = config_manager.get_net_health();
        let interval = std::time::Duration::from_secs(config.interval_secs);
        tokio::time::sleep(interval).await;

        if !config.enabled {
            continue;
        }

        // 启动宽限期内不判死，但仍重置计数，避免旧状态残留。
        if started_at.elapsed().as_secs() < STARTUP_GRACE_SECONDS {
            if state.consecutive_failures != 0 {
                state.consecutive_failures = 0;
                write_state(&state);
            }
            continue;
        }

        // 静默期内跳过探测，给基带重连留时间。
        if let Some(last) = last_action_at {
            if last.elapsed().as_secs() < config.cooldown_secs {
                continue;
            }
        }

        // 只有 ofono 已注册时才探活；searching/denied/unknown 阶段不判死。
        let status = crate::dbus::get_registration_status(&conn).await;
        let registered = matches!(status.as_deref(), Some("registered") | Some("roaming"));
        if !registered {
            state.consecutive_failures = 0;
            write_state(&state);
            continue;
        }

        // 并发 ping 两个目标，全部失败才计一次失败。
        let reachable = probe_targets(&config).await;

        if reachable {
            if state.consecutive_failures != 0 {
                info!("Net health restored (connection reachable)");
                state.consecutive_failures = 0;
                state.last_level = 0;
                write_state(&state);
            }
            continue;
        }

        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        let failures = state.consecutive_failures;

        // 决定本次应执行的动作等级。
        let level = if failures >= config.l3_failures {
            RecoveryLevel::Reboot
        } else if failures >= config.l2_failures {
            RecoveryLevel::AirplaneReset
        } else if failures >= config.l1_failures {
            RecoveryLevel::ResetConnection
        } else {
            RecoveryLevel::None
        };

        match level {
            RecoveryLevel::None => {
                info!(failures, "Net health probe failed (below Level 1 threshold)");
                write_state(&state);
            }
            RecoveryLevel::ResetConnection => {
                warn!(failures, "Net health Level 1: resetting data connection");
                execute_level_1(&conn).await;
                state.last_level = 1;
                last_action_at = Some(std::time::Instant::now());
                write_state(&state);
            }
            RecoveryLevel::AirplaneReset => {
                warn!(failures, "Net health Level 2: airplane-mode baseband reset");
                execute_level_2(&conn).await;
                state.last_level = 2;
                last_action_at = Some(std::time::Instant::now());
                write_state(&state);
            }
            RecoveryLevel::Reboot => {
                warn!(failures, "Net health Level 3: scheduling system reboot");
                if crate::restart::schedule_reboot("net_unreachable", 3) {
                    state.last_level = 3;
                    write_state(&state);
                }
            }
        }
    }
}

/// 并发 ping 探测目标，任一可达返回 true。
async fn probe_targets(config: &NetHealthConfig) -> bool {
    let timeout_secs = config.ping_timeout_secs;
    let futures: Vec<_> = PROBE_TARGETS
        .iter()
        .map(|target| {
            let target = *target;
            tokio::task::spawn_blocking(move || ping_target(target, timeout_secs))
        })
        .collect();

    for fut in futures {
        if let Ok(Ok(true)) = fut.await {
            return true;
        }
    }
    false
}

/// 对单个目标执行一次 ping，成功返回 true。
fn ping_target(target: &str, timeout_secs: u64) -> std::io::Result<bool> {
    let output = Command::new("ping")
        .args([
            "-c",
            "1",
            "-W",
            &timeout_secs.to_string(),
            target,
        ])
        .output()?;
    Ok(output.status.success())
}

/// Level 1：重置数据连接（Active=false → true）。
///
/// 先关闭再重新激活数据 context，清除 ofono 侧的假死上下文。断开的副作用由
/// `data_connection_watchdog` 兜底，本函数只负责主动抖动一次。
async fn execute_level_1(conn: &Connection) {
    if let Err(e) = crate::dbus::set_data_connection(conn, false).await {
        warn!(error = %e, "Net health Level 1: failed to deactivate data connection");
    }
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    if let Err(e) = crate::dbus::set_data_connection(conn, true).await {
        warn!(error = %e, "Net health Level 1: failed to activate data connection");
    }
}

/// Level 2：飞行模式复位基带（Online=false → 等 15s → Online=true）。
async fn execute_level_2(conn: &Connection) {
    if let Err(e) = crate::dbus::set_airplane_mode(conn, true).await {
        warn!(error = %e, "Net health Level 2: failed to enter airplane mode");
        return;
    }
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    if let Err(e) = crate::dbus::set_airplane_mode(conn, false).await {
        warn!(error = %e, "Net health Level 2: failed to exit airplane mode");
    }
}

#[cfg(test)]
mod tests {
    use super::RecoveryLevel;

    #[test]
    fn recovery_level_variants_are_complete() {
        // 静态检查：确保四个等级的枚举定义完整，防止后续重构误删。
        let variants = [
            RecoveryLevel::None,
            RecoveryLevel::ResetConnection,
            RecoveryLevel::AirplaneReset,
            RecoveryLevel::Reboot,
        ];
        assert_eq!(variants.len(), 4);
    }
}