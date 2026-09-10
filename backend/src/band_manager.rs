//! 锁频/锁网配置持久化重套管理
//!
//! 解决三类常见问题：
//! 1. **4G 物联卡自动模式无信号**：物联卡只签约 LTE，但 modem auto 模式优先搜 5G NR，
//!    搜不到再回落到 4G，反复震荡表现为"无信号"。→ 开机自动设为仅 LTE。
//! 2. **Band-lock NVRAM 不可靠**：AT+SPLBAND 虽然通常写入 NVRAM 但不同固件行为不一致。
//! 3. **Cell-lock RAM 态丢失**：AT+SPFORCEFRQ 掉电/去附着必然丢失。
//!
//! 本模块在每次开机（init_data_connection 成功后）和每次 Watchdog 重连成功后自动调用，
//! 将 config.json 中保存的射频模式、频段锁、小区锁配置重新下发到 modem。

use tracing::{info, warn};
use zbus::Connection;

use crate::config::{BandLockConfig, CellLockConfig, ConfigManager};

/// 一次性套用所有持久化锁频/锁网配置。
///
/// 应在 ofono 已就绪、网络已注册后调用。
/// 每个步骤的失败只打日志，不阻断后续步骤。
pub async fn apply_persisted_locks(conn: &Connection, config_manager: &ConfigManager) {
    // 1. 射频模式：仅 LTE / 仅 NR / auto
    let radio_mode = config_manager.get_radio_mode_cfg();
    if !radio_mode.is_auto() {
        info!("Re-applying persisted radio mode: {}", radio_mode.mode);
        let target = match radio_mode.mode.as_str() {
            "lte" => crate::models::RadioMode::LteOnly,
            "nr" => crate::models::RadioMode::NrOnly,
            _ => crate::models::RadioMode::Auto,
        };
        if let Err(e) = crate::dbus::set_radio_mode(conn, target).await {
            warn!(error = %e, mode = %radio_mode.mode, "Failed to re-apply radio mode");
        }
    }

    // 2. 频段锁定（AT+SPLBAND）
    let band_lock = config_manager.get_band_lock();
    if !band_lock.is_empty() {
        info!(
            lte_fdd = ?band_lock.lte_fdd_bands,
            lte_tdd = ?band_lock.lte_tdd_bands,
            nr_fdd = ?band_lock.nr_fdd_bands,
            nr_tdd = ?band_lock.nr_tdd_bands,
            "Re-applying persisted band lock"
        );
        if let Err(e) = apply_band_lock(conn, &band_lock).await {
            warn!(error = %e, "Failed to re-apply band lock");
        }
    }

    // 3. 小区锁定（AT+SPFORCEFRQ）
    let cell_lock = config_manager.get_cell_lock();
    if !cell_lock.is_empty() {
        info!(
            lte_arfcn = ?cell_lock.lte_arfcn,
            lte_pci = ?cell_lock.lte_pci,
            nr_arfcn = ?cell_lock.nr_arfcn,
            nr_pci = ?cell_lock.nr_pci,
            "Re-applying persisted cell lock"
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

/// 下发小区锁定 AT 指令到 modem。
async fn apply_cell_lock(conn: &Connection, config: &CellLockConfig) -> Result<(), String> {
    // 进入工程模式
    send_at(conn, "AT+SFUN=5").await?;

    // 清空现有锁定，避免冲突
    let _ = send_at(conn, "AT+SPFORCEFRQ=16,0").await;
    let _ = send_at(conn, "AT+SPFORCEFRQ=12,0").await;

    // 下发 LTE 小区锁
    if config.has_lte() {
        let arfcn = config.lte_arfcn.unwrap();
        let pci = config.lte_pci.unwrap();
        let cmd = format!("AT+SPFORCEFRQ=12,2,{},{}", arfcn, pci);
        send_at(conn, &cmd)
            .await
            .map_err(|e| format!("LTE cell lock failed (arfcn={}, pci={}): {}", arfcn, pci, e))?;
    }

    // 下发 NR 小区锁
    if config.has_nr() {
        let arfcn = config.nr_arfcn.unwrap();
        let pci = config.nr_pci.unwrap();
        let cmd = format!("AT+SPFORCEFRQ=16,2,{},{}", arfcn, pci);
        send_at(conn, &cmd)
            .await
            .map_err(|e| format!("NR cell lock failed (arfcn={}, pci={}): {}", arfcn, pci, e))?;
    }

    // 恢复正常模式
    send_at(conn, "AT+SFUN=4").await?;

    Ok(())
}