/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2026-09-23
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @FilePath: /udx710-backend/backend/src/network_pref.rs
 * @Description: 智能优先选网引擎（v3.8.10）——把 network_preference.mode 映射为
 *               ofono TechnologyPreference，并为「优先 4G / 优先 5G」实现
 *               脱网应急切换 + 闲时静默回切。
 *
 * Copyright (c) 2025 by 1orz, All Rights Reserved.
 */
//! 智能优先选网引擎。
//!
//! 与 `data_connection_watchdog`（只管数据连接 Active 的常驻自愈）正交：本引擎只管
//! 制式（RAT）选择，即 `TechnologyPreference` 的常驻 + 脱网应急 + 闲时探测。
//!
//! 模式语义（`network_preference.mode`）：
//! - `prefer_lte`：常驻 4G（LTE only）；连续脱网超时后升级为原生(auto)以 5G 应急保活；
//!   每隔 probe_interval_mins 分钟静默切回 4G 探测，有信号则常驻回 4G。
//! - `prefer_5g`：常驻 5G（NR only）；5G 盲区/异常脱网超时后下沉 4G（LTE only）保活；
//!   每隔 probe_interval_mins 分钟静默切回 5G 探测，有信号则常驻回 5G。
//! - `lte_only`：强行锁定 4G，引擎自愈抵抗任何漂移（定时切换 / 短信切换）。
//! - `auto`：原生策略，引擎被动不干预（保持旧版本 radio_mode / 5G 开关等机制不变）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};
use zbus::Connection;

use crate::config::ConfigManager;
use crate::dbus::{get_radio_mode, get_registration_status, set_radio_mode};
use crate::models::RadioMode;

/// 引擎轮询周期（秒）。脱网判定粒度受此影响：30s 超时实际在 [30, 30+5)s 内命中。
const ENGINE_POLL_SECS: u64 = 5;
/// 探测首选制式后，等待注网的窗口（秒）。等待结束后仍未注册视为「首选网络不可用」，
/// 退回备用制式继续保活（规格要求 10 秒内注网成功则留在首选网络）。
const PROBE_REGISTER_WINDOW_SECS: u64 = 10;

/// 注网状态是否为「已注网 / 漫游」。
fn status_is_registered(status: Option<&str>) -> bool {
    matches!(status, Some("registered") | Some("roaming"))
}

/// 注网状态是否为「明确的脱网」。`unknown` 或 ofono 不可达（None）不计，
/// 避免在 ofono 尚未就绪 / 刚开机时误触发应急降级。
fn status_is_offline(status: Option<&str>) -> bool {
    matches!(status, Some("searching") | Some("unregistered") | Some("denied"))
}

/// 等待注网就绪，最长 `max_secs` 秒，每 1 秒轮询一次。
async fn wait_registered(conn: &Connection, max_secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    while Instant::now() < deadline {
        if status_is_registered(get_registration_status(conn).await.as_deref()) {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

/// 蜂窝数据网卡接口名（与 dbus.rs 中 Always-On 看门狗一致）。
const SIPA_ETH0_IFACE: &str = "sipa_eth0";
/// 「大流量」粗筛门限：探测前采样 sipa_eth0 收发字节速率，低于此值（字节/秒）视为空闲。
/// 128 KB/s ≈ 1 Mbps，空闲/轻量浏览不受影响；高于此值（下载、视频）跳过本轮探测。
const IDLE_TRAFFIC_THRESHOLD_BPS: u64 = 128 * 1024;
/// 空闲采样窗口（秒）：两次读取网卡累计字节数的间隔。
const IDLE_SAMPLE_WINDOW_SECS: u64 = 5;

/// 读取 sipa_eth0 累计收发字节数（rx + tx）。网卡不存在或计数不可读时返回 None。
fn read_sipa_total_bytes() -> Option<u64> {
    let base = std::path::Path::new("/sys/class/net")
        .join(SIPA_ETH0_IFACE)
        .join("statistics");
    let rx: u64 = std::fs::read_to_string(base.join("rx_bytes")).ok()?.trim().parse().ok()?;
    let tx: u64 = std::fs::read_to_string(base.join("tx_bytes")).ok()?.trim().parse().ok()?;
    Some(rx.saturating_add(tx))
}

/// 判断当前是否「无大流量」（空闲）：
/// - 网卡未就绪 / 无数据通路 → 视为空闲（可安全探测，不会打断任何正在进行的业务）；
/// - 采样窗口内字节增速低于阈值 → 视为空闲。
async fn sipa_eth0_idle() -> bool {
    let Some(before) = read_sipa_total_bytes() else {
        return true;
    };
    tokio::time::sleep(Duration::from_secs(IDLE_SAMPLE_WINDOW_SECS)).await;
    let Some(after) = read_sipa_total_bytes() else {
        return true;
    };
    let rate = after.saturating_sub(before) / IDLE_SAMPLE_WINDOW_SECS;
    rate < IDLE_TRAFFIC_THRESHOLD_BPS
}

/// 把模式字符串映射为首选/备用制式。非 prefer_* 模式返回 None。
fn home_fallback_for(mode: &str) -> Option<(RadioMode, RadioMode)> {
    match mode {
        // 4G 优先：常驻 LTE only；脱网升级为原生(auto)以 5G 应急（auto 优先 NR，4G 兜底）。
        "prefer_lte" => Some((RadioMode::LteOnly, RadioMode::Auto)),
        // 5G 优先：常驻 NR only；脱网下沉为 LTE only 稳定 4G 保活（避免 5G 反复重扫）。
        "prefer_5g" => Some((RadioMode::NrOnly, RadioMode::LteOnly)),
        _ => None,
    }
}

/// 确保当前制式等于 `target`；不一致才写入，避免每次轮询都 SetProperty 而触发
/// 无谓的射频重建（Radio Cycle）。ofono 尚未就绪时跳过，下一轮再试。
async fn ensure_radio_mode(conn: &Connection, target: RadioMode) {
    let target_str = match target {
        RadioMode::Auto => "auto",
        RadioMode::LteOnly => "lte",
        RadioMode::NrOnly => "nr",
    };
    let current = match get_radio_mode(conn).await {
        Ok(resp) => resp.mode,
        Err(_) => return,
    };
    if current == target_str {
        return;
    }
    match set_radio_mode(conn, target).await {
        Ok(_) => info!(target = %target_str, "network_pref: TechnologyPreference -> {}", target_str),
        Err(e) => warn!(error = %e, target = %target_str, "network_pref: failed to set TechnologyPreference"),
    }
}

/// 引擎内部：优先策略当前所处阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 常驻首选制式。
    Home,
    /// 已因脱网切换到备用制式，等待下次闲时探测。
    Fallback,
}

/// 智能优先选网引擎主循环。
pub async fn network_preference_engine(conn: Arc<Connection>, config_manager: Arc<ConfigManager>) {
    // 上次已生效的模式字符串，用于检测配置变更并重置状态机。
    let mut applied_mode = String::new();
    let mut phase = Phase::Home;
    // 进入「明确脱网」计时的时间点（仅 Home 阶段使用）。
    let mut failover_since: Option<Instant> = None;
    // 上次探测时间（进入 Fallback 时初始化）。
    let mut last_probe: Option<Instant> = None;

    loop {
        let pref = config_manager.get_network_preference();
        let mode = pref.mode.clone();

        // 配置模式变更：重置状态机。初始制式的生效应由下方每轮自愈 ensure 完成，
        // 避免「引擎接管」与「apply_persisted_locks 等外部写制式」之间重竞争。
        if mode != applied_mode {
            applied_mode = mode.clone();
            phase = Phase::Home;
            failover_since = None;
            last_probe = None;
            if home_fallback_for(&mode).is_some() {
                info!(mode = %mode, "network_pref: strategy engaged");
            } else if mode == "lte_only" {
                info!("network_pref: hard-lock LTE only engaged");
            }
        }

        if let Some((home, fallback)) = home_fallback_for(&mode) {
            // 每轮自愈：确保当前制式与阶段目标一致（读后写，无漂移则不写），
            // 抵抗外部对 TechnologyPreference 的修改（开机 apply_persisted_locks 竞态、
            // 定时切换 / 短信切换 / 手动 5G 开关等）。
            let target = match phase {
                Phase::Home => home.clone(),
                Phase::Fallback => fallback.clone(),
            };
            ensure_radio_mode(&conn, target).await;

            let status = get_registration_status(&conn).await;
            let registered = status_is_registered(status.as_deref());

            match phase {
                Phase::Home => {
                    if registered {
                        // 首选制式稳定注网，清空脱网计时。
                        failover_since = None;
                    } else if status_is_offline(status.as_deref()) {
                        let since = *failover_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= Duration::from_secs(pref.failover_timeout_secs) {
                            info!(
                                mode = %mode,
                                seconds = pref.failover_timeout_secs,
                                "network_pref: preferred network offline, switching to fallback"
                            );
                            ensure_radio_mode(&conn, fallback).await;
                            phase = Phase::Fallback;
                            last_probe = Some(Instant::now());
                            failover_since = None;
                        }
                    }
                    // unknown / ofono 不可达：不计数。
                }
                Phase::Fallback => {
                    let due = match last_probe {
                        Some(t) => {
                            t.elapsed() >= Duration::from_secs(pref.probe_interval_mins * 60)
                        }
                        None => true,
                    };
                    if due {
                        // 「闲时静默探测」：仅在无大流量时发起，避免打断用户
                        // 正在进行的下载/大流量业务；有流量则不探测、不推进计时，
                        // 下一轮继续观察闲时状态。
                        if sipa_eth0_idle().await {
                            info!(mode = %mode, "network_pref: silently probing preferred network");
                            ensure_radio_mode(&conn, home).await;
                            if wait_registered(&conn, PROBE_REGISTER_WINDOW_SECS).await {
                                info!(mode = %mode, "network_pref: preferred network restored");
                                phase = Phase::Home;
                            } else {
                                info!(mode = %mode, "network_pref: preferred network unavailable, staying on fallback");
                                ensure_radio_mode(&conn, fallback).await;
                            }
                            last_probe = Some(Instant::now());
                        }
                    }
                }
            }
        } else if mode == "lte_only" {
            // 强行锁定 4G：每轮自愈，抵抗定时切换/短信切换等造成的制式漂移。
            ensure_radio_mode(&conn, RadioMode::LteOnly).await;
        }
        // "auto"：原生策略，引擎被动不干预。

        tokio::time::sleep(Duration::from_secs(ENGINE_POLL_SECS)).await;
    }
}