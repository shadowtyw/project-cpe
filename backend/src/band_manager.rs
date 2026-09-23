//! 锁频/锁网配置持久化重套管理
//!
//! 解决三类常见问题：
//! 1. **4G 物联卡自动模式无信号**：物联卡只签约 LTE，但 modem auto 模式优先搜 5G NR，
//!    搜不到再回落到 4G，反复震荡表现为"无信号"。→ 开机自动设为仅 LTE。
//! 2. **Band-lock NVRAM 不可靠**：AT+SPLBAND 虽然通常写入 NVRAM 但不同固件行为不一致。
//! 3. **Cell-lock RAM 态丢失**：AT+SPFORCEFRQ 掉电/去附着必然丢失。
//!
//! 本模块在开机（init_data_connection 成功后）将 config.json 中保存的射频模式、
//! 频段锁、小区锁配置重新下发到 modem；Watchdog 重连成功后仅重套 RAM 态小区锁，
//! 避免每次重连都重发频段锁/制式而触发 Radio 重启造成断流震荡。

use std::time::Duration;

use tracing::{info, warn};
use zbus::Connection;

use crate::config::{BandLockConfig, CellLockConfig, ConfigManager};

/// 频段/射频操作后的注网静默窗口（秒）。
///
/// 展锐基带执行 SPLBAND / 制式切换会触发 Radio 重启，必须给射频留出足够时间
/// 重新搜网注网；若紧接着就发起数据连接激活，会因射频尚未注网而失败并引发
/// 「重连→重启射频→断流」震荡。
const BAND_QUIET_WINDOW_SECS: u64 = 6;

/// 开机初期 `/ril_0` 的 RadioSettings 接口导出延迟的重试参数。
///
/// `wait_for_ofono` 只确认 ofono 服务名字已注册，但 `/ril_0` 上的 RadioSettings
/// 接口通常还要再延迟 1~3 秒才导出。若在此窗口内下发 SetProperty 会瞬时失败并
/// 打出 `Failed to apply radio mode` 的 warning。以 1 秒间隔重试几次即可静默覆盖。
const RADIO_READY_RETRIES: usize = 3;
const RADIO_READY_RETRY_INTERVAL_SECS: u64 = 1;

/// 带开机就绪等待与重试地应用持久化射频模式。
///
/// 沿用 `init_data_connection` 同款「瞬时错误有限重试」思路：就绪窗口内的失败
/// 静默重试，只有重试耗尽仍失败才打 warning（真正的配置错误才值得上报）。
async fn apply_radio_mode_retry(conn: &Connection, target: crate::models::RadioMode, mode: &str) {
    let mut last_error = String::new();
    for attempt in 0..=RADIO_READY_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(RADIO_READY_RETRY_INTERVAL_SECS)).await;
        }
        match crate::dbus::set_radio_mode(conn, target.clone()).await {
            Ok(()) => {
                if attempt > 0 {
                    info!(
                        mode = %mode,
                        retries = ?attempt,
                        "Persisted radio mode applied after readiness retry"
                    );
                }
                return;
            }
            Err(e) => last_error = e.to_string(),
        }
    }
    warn!(error = %last_error, mode = %mode, "Failed to apply radio mode");
}

/// 一次性套用所有持久化锁频/锁网配置（仅开机调用一次）。
///
/// 应在 ofono 已就绪后调用。每个步骤的失败只打日志，不阻断后续步骤。
///
/// 频段锁（SPLBAND）与制式（TechnologyPreference）均为持久态，且重发会触发
/// Radio 重启，因此只在开机重套；小区锁（SPFORCEFRQ）为 RAM 态，见
/// [`reapply_runtime_locks`] 在重连后另行重套。
pub async fn apply_persisted_locks(conn: &Connection, config_manager: &ConfigManager) {
    // 1. 射频模式：仅 LTE / 仅 NR / auto
    let radio_mode = config_manager.get_radio_mode_cfg();
    if !radio_mode.is_auto() {
        info!("Applying persisted radio mode: {}", radio_mode.mode);
        let target = match radio_mode.mode.as_str() {
            "lte" => crate::models::RadioMode::LteOnly,
            "nr" => crate::models::RadioMode::NrOnly,
            _ => crate::models::RadioMode::Auto,
        };
        apply_radio_mode_retry(conn, target, &radio_mode.mode).await;
    }

    // 2. 频段锁定（AT+SPLBAND）
    let band_lock = config_manager.get_band_lock();
    if band_lock.is_empty() {
        // 容错：四个数组均为空时，强制将 modem 恢复为出厂全频段。
        // 历史上「清空锁」曾下发过零掩码导致脱网；启动时无条件刷新全频段
        // 可清除任何残留锁，保证设备以全频段可用状态开机。
        info!("Band lock config empty, restoring modem to factory full-band");
        if let Err(e) = apply_band_unlock(conn).await {
            warn!(error = %e, "Failed to restore modem to factory full-band");
        }
    } else {
        info!(
            lte_fdd = ?band_lock.lte_fdd_bands,
            lte_tdd = ?band_lock.lte_tdd_bands,
            nr_fdd = ?band_lock.nr_fdd_bands,
            nr_tdd = ?band_lock.nr_tdd_bands,
            "Applying persisted band lock"
        );
        if let Err(e) = apply_band_lock(conn, &band_lock).await {
            warn!(error = %e, "Failed to apply band lock");
        }
    }

    // 射频操作后留出注网静默窗口，避免立即激活数据导致断流震荡。
    tokio::time::sleep(Duration::from_secs(BAND_QUIET_WINDOW_SECS)).await;

    // 3. 小区锁定（AT+SPFORCEFRQ）
    let cell_lock = config_manager.get_cell_lock();
    if !cell_lock.is_empty() {
        info!(
            lte_arfcn = ?cell_lock.lte_arfcn,
            lte_pci = ?cell_lock.lte_pci,
            nr_arfcn = ?cell_lock.nr_arfcn,
            nr_pci = ?cell_lock.nr_pci,
            "Applying persisted cell lock"
        );
        if let Err(e) = apply_cell_lock(conn, &cell_lock).await {
            warn!(error = %e, "Failed to apply cell lock");
        }
    }
}

/// Watchdog 重连成功后仅重套 RAM 态的小区锁。
///
/// 频段锁（SPLBAND）与制式（TechnologyPreference）重发会触发 Radio 重启，若每次
/// 重连都重新下发，会导致「重连→重启射频→断流→再重连」震荡，因此它们只在开机由
/// [`apply_persisted_locks`] 重套。小区锁为 RAM 态，去附着/掉电必然丢失，必须重套。
pub async fn reapply_runtime_locks(conn: &Connection, config_manager: &ConfigManager) {
    let cell_lock = config_manager.get_cell_lock();
    if !cell_lock.is_empty() {
        info!(
            lte_arfcn = ?cell_lock.lte_arfcn,
            lte_pci = ?cell_lock.lte_pci,
            nr_arfcn = ?cell_lock.nr_arfcn,
            nr_pci = ?cell_lock.nr_pci,
            "Re-applying persisted cell lock (runtime)"
        );
        if let Err(e) = apply_cell_lock(conn, &cell_lock).await {
            warn!(error = %e, "Failed to re-apply cell lock");
        }
    }
}

/// 通过 AT 串口发送指令（与 handlers 复用同一 D-Bus 通道）。
async fn send_at(conn: &Connection, cmd: &str) -> Result<String, String> {
    crate::dbus::send_at_command(conn, cmd)
        .await
        .map_err(|e| format!("AT command '{}' failed: {}", cmd, e))
}

/// 下发频段锁定 AT 指令到 modem。
async fn apply_band_lock(conn: &Connection, config: &BandLockConfig) -> Result<(), String> {
    let lte_fdd_mask = crate::utils::bands_to_bitmask(&config.lte_fdd_bands, 1);
    let lte_tdd_mask = crate::utils::bands_to_bitmask(&config.lte_tdd_bands, 33);

    if lte_fdd_mask != 0 || lte_tdd_mask != 0 {
        let cmd = crate::utils::build_splband_lte_command(lte_fdd_mask, lte_tdd_mask);
        send_at(conn, &cmd).await?;
    }

    let nr_fdd_mask = crate::utils::bands_to_bitmask(&config.nr_fdd_bands, 100);
    let nr_tdd_mask = crate::utils::bands_to_bitmask(&config.nr_tdd_bands, 41);

    if nr_fdd_mask != 0 || nr_tdd_mask != 0 {
        let cmd = crate::utils::build_splband_nr_command(nr_fdd_mask, nr_tdd_mask);
        send_at(conn, &cmd).await?;
    }

    Ok(())
}

/// 恢复 LTE + NR 出厂全频段（解除频段锁的正确方式）。
///
/// 必须下发全频段掩码而非 0：下发 0 会令 modem 锁到零频段导致脱网。
async fn apply_band_unlock(conn: &Connection) -> Result<(), String> {
    send_at(conn, &crate::utils::build_splband_lte_full_command()).await?;
    send_at(conn, &crate::utils::build_splband_nr_full_command()).await?;
    Ok(())
}

/// 下发小区锁定 AT 指令到 modem。
///
/// 关键约束：进入工程模式（SFUN=5）后，无论中途哪一步失败，都必须恢复
/// SFUN=4，否则 modem 将滞留工程模式导致无信号。此函数在每次开机和 watchdog
/// 重连后都会被调用，一旦泄漏工程模式状态，设备将在重启后依旧无网。
async fn apply_cell_lock(conn: &Connection, config: &CellLockConfig) -> Result<(), String> {
    // 进入工程模式
    send_at(conn, "AT+SFUN=5").await?;

    // 内层执行；失败也先恢复 SFUN=4 再向上抛错
    let result = apply_cell_lock_inner(conn, config).await;

    // 恢复正常模式（无论成败都执行；失败仅告警，不覆盖内层错误）
    if let Err(e) = send_at(conn, "AT+SFUN=4").await {
        warn!(error = %e, "Failed to restore normal mode (SFUN=4) after cell lock");
    }

    result
}

async fn apply_cell_lock_inner(conn: &Connection, config: &CellLockConfig) -> Result<(), String> {
    // 清空现有锁定，避免冲突
    let _ = send_at(conn, "AT+SPFORCEFRQ=16,0").await;
    let _ = send_at(conn, "AT+SPFORCEFRQ=12,0").await;

    // 下发 LTE 小区锁。用 if let 双 Some 模式匹配，取代「has_lte() 守卫 + unwrap」，
    // 从结构上杜绝 unwrap panic（release 构建 panic = abort，任何 panic 都会杀进程）。
    if let (Some(arfcn), Some(pci)) = (config.lte_arfcn, config.lte_pci) {
        let cmd = format!("AT+SPFORCEFRQ=12,2,{},{}", arfcn, pci);
        send_at(conn, &cmd)
            .await
            .map_err(|e| format!("LTE cell lock failed (arfcn={}, pci={}): {}", arfcn, pci, e))?;
    }

    // 下发 NR 小区锁
    if let (Some(arfcn), Some(pci)) = (config.nr_arfcn, config.nr_pci) {
        let cmd = format!("AT+SPFORCEFRQ=16,2,{},{}", arfcn, pci);
        send_at(conn, &cmd)
            .await
            .map_err(|e| format!("NR cell lock failed (arfcn={}, pci={}): {}", arfcn, pci, e))?;
    }

    Ok(())
}