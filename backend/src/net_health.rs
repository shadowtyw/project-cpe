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

/// 断网重启熔断窗口：统计「因断网触发重启」的滑动时间窗（秒）。
///
/// 无卡 / 欠费 / 盲区环境下，设备可能表现为「能注册但数据不通」，一旦探活失败
/// 就会沿 Level 1→2→3 阶梯走到重启，重启后又重新进入同一死循环。用 1 小时窗口
/// 累计断网重启次数，超过上限即熔断，杜绝无限重启。
const DISCONNECT_REBOOT_WINDOW_SECS: i64 = 60 * 60;

/// 1 小时窗口内允许的断网重启次数上限。
const DISCONNECT_REBOOT_MAX: u32 = 3;

/// 熔断后的强制休眠时长（秒）：给网络环境与运营商侧恢复留足时间。休眠结束后
/// 滑动窗口可能已滑出，设备重新获得有限的重启额度；若环境仍不可用则再次熔断。
const BREAKER_SLEEP_SECONDS: u64 = 30 * 60;

enum RecoveryLevel {
    None,
    ResetConnection,
    AirplaneReset,
    Reboot,
}

/// Level 2 飞行模式复位的执行结果。
///
/// 必须区分「进入飞行模式就失败」与「进入成功但退不出」：前者设备仍在线，
/// 后者设备已卡在飞行模式彻底离线，需要 `stuck_in_airplane` 标记才能绕过
/// 主循环「未注册即清零」的逻辑，让失败计数爬到 Level 3 触发重启兜底。
#[derive(Debug, PartialEq, Eq)]
enum AirplaneResetOutcome {
    /// 已成功退出飞行模式，基带恢复在线。
    Recovered,
    /// 进入飞行模式失败，设备仍在线（未卡死）。
    EnterFailed,
    /// 进入飞行模式成功但重试后仍退不出，设备已离线（卡死）。
    StuckOffline,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct NetHealthState {
    consecutive_failures: u32,
    last_level: u8,
    /// Level 2 飞行模式复位未能恢复在线时置位。卡飞行模式的设备注册状态必然
    /// 不是 registered，若不单独标记，主循环的「未注册即清零计数」逻辑会让
    /// 失败计数永远无法累积到 Level 3，设备将永久离线且无兜底。
    #[serde(default)]
    stuck_in_airplane: bool,
    /// 最近一次「因断网触发重启」的 Unix 时间戳列表（每次重启追加一个，旧值滑动
    /// 淘汰）。用于 1 小时内 3 次的熔断判定。不存在该字段的旧状态文件按空列表解析。
    #[serde(default)]
    disconnect_reboots: Vec<i64>,
    /// 熔断休眠截止时间戳；为 `Some` 表示当前处于熔断期，探活失败不再触发重启。
    #[serde(default)]
    breaker_until: Option<i64>,
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

/// 决策是否放行本次「因断网触发」的重启；返回 `false` 表示被熔断拦截。
///
/// 三级判定：
/// 1. 正处于熔断期（`breaker_until` 未到期）→ 拒绝，静默等待窗口滑出；
/// 2. 滑动窗口内历史断网重启已达 `DISCONNECT_REBOOT_MAX` 次 → 首次触发熔断，
///    记录 `breaker_until` 并清空时间戳 → 拒绝；
/// 3. 否则记录本次重启时间戳（在真正重启前随 `write_state` 落盘，跨重启累计）
///    → 放行。
fn disconnect_reboot_allowed(state: &mut NetHealthState) -> bool {
    let now = chrono::Utc::now().timestamp();

    if state
        .breaker_until
        .is_some_and(|until| now < until)
    {
        warn!("Net health breaker OPEN: disconnect-triggered reboot suppressed");
        return false;
    }

    state
        .disconnect_reboots
        .retain(|ts| now.saturating_sub(*ts) < DISCONNECT_REBOOT_WINDOW_SECS);

    if state.disconnect_reboots.len() as u32 >= DISCONNECT_REBOOT_MAX {
        state.breaker_until = Some(now + BREAKER_SLEEP_SECONDS as i64);
        state.disconnect_reboots.clear();
        warn!(
            window_secs = DISCONNECT_REBOOT_WINDOW_SECS,
            sleep_secs = BREAKER_SLEEP_SECONDS,
            "Net health breaker TRIPPED: too many disconnect-triggered reboots; entering cooldown"
        );
        return false;
    }

    state.disconnect_reboots.push(now);
    true
}

/// 熔断休眠：被断开后落盘状态并长时间停止判死，给运营商侧与网络环境充分恢复时间。
///
/// 拆成 60s 一段而非单个 30 分钟 sleep，是为了保持对进程取消/关停的响应，避免
/// 单个超长 `sleep` 让 watchdog 在关机时无法及时让出。同时清零失败计数：休眠结束
/// 后设备从干净状态重新走 L1→L2→L3 阶梯，而不是因残留的 ≥L3 计数立刻再次重启。
async fn breaker_hold(state: &mut NetHealthState) {
    state.consecutive_failures = 0;
    state.last_level = 0;
    write_state(state);
    let mut remaining = BREAKER_SLEEP_SECONDS;
    while remaining > 0 {
        let step = remaining.min(60);
        tokio::time::sleep(std::time::Duration::from_secs(step)).await;
        remaining -= step;
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
            if state.stuck_in_airplane {
                // 例外：Level 2 复位失败卡飞行模式 → 设备确定离线，继续累计失败，
                // 让计数最终爬到 Level 3 触发重启兜底（飞行模式重启后自动解除）。
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                let failures = state.consecutive_failures;
                if failures >= config.l3_failures {
                    warn!(failures, "Net health: still offline after failed airplane reset; escalating to Level 3 reboot");
                    if disconnect_reboot_allowed(&mut state) {
                        if crate::restart::schedule_reboot("airplane_stuck", 3) {
                            state.last_level = 3;
                            state.stuck_in_airplane = false;
                        }
                        write_state(&state);
                    } else {
                        breaker_hold(&mut state).await;
                    }
                } else {
                    write_state(&state);
                }
            } else if state.consecutive_failures != 0 {
                state.consecutive_failures = 0;
                write_state(&state);
            }
            continue;
        }
        // 已注册即说明射频在线：清除飞行模式卡死标记。
        if state.stuck_in_airplane {
            state.stuck_in_airplane = false;
            write_state(&state);
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
                match execute_level_2(&conn).await {
                    AirplaneResetOutcome::Recovered => {
                        state.last_level = 2;
                        last_action_at = Some(std::time::Instant::now());
                    }
                    AirplaneResetOutcome::EnterFailed => {
                        // 进入飞行模式就失败：设备仍在线，不设 cooldown，下一轮重试。
                        warn!("Net health Level 2: failed to enter airplane mode; will retry next cycle");
                    }
                    AirplaneResetOutcome::StuckOffline => {
                        // 已卡飞行模式离线：标记 stuck，让后续未注册轮次继续累计失败
                        // 直至升级 Level 3 重启。不设 cooldown，下一轮立即重试。
                        state.stuck_in_airplane = true;
                        warn!("Net health Level 2: stuck in airplane mode; flagged for escalation");
                    }
                }
                write_state(&state);
            }
            RecoveryLevel::Reboot => {
                warn!(failures, "Net health Level 3: scheduling system reboot");
                // 断网重启前经过熔断闸门：无卡 / 欠费 / 盲区环境下若放任每次都走到
                // 这里，设备会在「重启→搜网→仍不通→再重启」间无限循环，既烧闪存
                // 又无法自愈。1 小时内累计 3 次即熔断休眠。
                if disconnect_reboot_allowed(&mut state) {
                    if crate::restart::schedule_reboot("net_unreachable", 3) {
                        state.last_level = 3;
                        write_state(&state);
                    }
                } else {
                    breaker_hold(&mut state).await;
                }
            }
        }
    }
}

/// 并发 ping 探测目标，任一可达返回 true。
///
/// `ping` 子进程理论上受 `-W` 超时约束，但命令不存在、参数被裁剪或系统负载
/// 异常时仍可能长时间挂起，占满 tokio 阻塞线程池。此处为每个探测加一道
/// `ping_timeout + 3s` 的硬超时，超时按「不可达」处理。
async fn probe_targets(config: &NetHealthConfig) -> bool {
    let timeout_secs = config.ping_timeout_secs;
    let hard_timeout = std::time::Duration::from_secs(timeout_secs.saturating_add(3));
    let futures: Vec<_> = PROBE_TARGETS
        .iter()
        .map(|target| {
            let target = *target;
            tokio::task::spawn_blocking(move || ping_target(target, timeout_secs))
        })
        .collect();

    for fut in futures {
        match tokio::time::timeout(hard_timeout, fut).await {
            Ok(Ok(Ok(true))) => return true,
            Ok(Ok(Ok(false))) => {}
            Ok(Ok(Err(e))) => warn!(error = %e, "Net health ping command failed"),
            Ok(Err(e)) => warn!(error = %e, "Net health ping task panicked"),
            Err(_) => warn!("Net health ping timed out (hard limit {}s)", hard_timeout.as_secs()),
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
///
/// 返回三态结果以区分「设备仍在线的失败」与「设备已卡离线的失败」：后者必须
/// 由调用方设置 `stuck_in_airplane` 标记，否则主循环的「未注册即清零计数」逻辑
/// 会让失败计数永远到不了 Level 3，设备将永久离线且无兜底。
async fn execute_level_2(conn: &Connection) -> AirplaneResetOutcome {
    if let Err(e) = crate::dbus::set_airplane_mode(conn, true).await {
        warn!(error = %e, "Net health Level 2: failed to enter airplane mode");
        return AirplaneResetOutcome::EnterFailed;
    }
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;

    // 退出飞行模式是关键一步，失败必须重试：ofono/基带刚复位时首个 D-Bus 调用
    // 常因 modem 尚未 ready 而失败，重试几次通常即可成功。
    const EXIT_RETRIES: u32 = 4;
    for attempt in 1..=EXIT_RETRIES {
        match crate::dbus::set_airplane_mode(conn, false).await {
            Ok(()) => {
                if attempt > 1 {
                    info!(attempt, "Net health Level 2: exited airplane mode after retry");
                }
                return AirplaneResetOutcome::Recovered;
            }
            Err(e) => {
                warn!(
                    error = %e,
                    attempt,
                    "Net health Level 2: failed to exit airplane mode ({}/{})",
                    attempt,
                    EXIT_RETRIES
                );
                if attempt < EXIT_RETRIES {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    // 所有重试失败：设备仍处于飞行模式（离线）。
    warn!("Net health Level 2: device stuck in airplane mode; will escalate to reboot");
    AirplaneResetOutcome::StuckOffline
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