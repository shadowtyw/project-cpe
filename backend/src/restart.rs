use crate::config::{get_persistent_root_dir, ConfigManager};
use crate::utils::{read_memory_info, read_uptime};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

const CHECK_INTERVAL_SECONDS: u64 = 60;
const STARTUP_GRACE_SECONDS: u64 = 10 * 60;
const LOW_MEMORY_CONSECUTIVE_CHECKS: u8 = 3;
const LOW_MEMORY_MAX_RESTARTS_PER_DAY: u8 = 1;
const OTA_STAGING_DIR: &str = "/tmp/ota_staging";

static REBOOT_PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Deserialize, Serialize)]
struct RestartState {
    low_memory_window_started_at: Option<i64>,
    low_memory_restart_count: u8,
}

pub fn schedule_reboot(reason: &'static str, delay_seconds: u64) -> bool {
    if REBOOT_PENDING.swap(true, Ordering::SeqCst) {
        warn!(reason, "Reboot request ignored because a reboot is already pending");
        return false;
    }

    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(delay_seconds));
        match std::process::Command::new("reboot").spawn() {
            Ok(_) => info!(reason, "System reboot requested"),
            Err(error) => {
                REBOOT_PENDING.store(false, Ordering::SeqCst);
                warn!(reason, error = %error, "Failed to request system reboot");
            }
        }
    });
    true
}

pub async fn restart_watchdog(config_manager: Arc<ConfigManager>) {
    let mut consecutive_low_memory_checks = 0u8;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(CHECK_INTERVAL_SECONDS)).await;

        if REBOOT_PENDING.load(Ordering::SeqCst) || std::path::Path::new(OTA_STAGING_DIR).exists() {
            continue;
        }

        let config = config_manager.get_restart();
        let uptime_seconds = match read_uptime() {
            Ok((uptime_seconds, _)) => uptime_seconds,
            Err(error) => {
                warn!(error = %error, "Restart watchdog could not read system uptime");
                continue;
            }
        };

        let low_memory_ready = config.low_memory_enabled && uptime_seconds >= STARTUP_GRACE_SECONDS;
        let low_memory_triggered = if low_memory_ready {
            let memory = match read_memory_info() {
                Ok(memory) => memory,
                Err(error) => {
                    warn!(error = %error, "Restart watchdog could not read memory information");
                    continue;
                }
            };
            if memory.total_bytes == 0 {
                continue;
            }
            if memory.available_estimated {
                warn!(source = %memory.available_source, "Low-memory watchdog is using an estimated available-memory value");
            }

            // A value exactly at the configured threshold does not trigger.
            let available_percent = memory.available_bytes.saturating_mul(100) / memory.total_bytes;
            if available_percent < u64::from(config.low_memory_threshold_percent) {
                consecutive_low_memory_checks = consecutive_low_memory_checks.saturating_add(1);
            } else {
                consecutive_low_memory_checks = 0;
            }

            if consecutive_low_memory_checks >= LOW_MEMORY_CONSECUTIVE_CHECKS {
                if low_memory_restart_limit_reached() {
                    warn!(
                        available_percent,
                        threshold = config.low_memory_threshold_percent,
                        "Low-memory restart suppressed because the daily limit was reached"
                    );
                    consecutive_low_memory_checks = 0;
                    false
                } else {
                    record_low_memory_restart();
                    info!(
                        available_percent,
                        threshold = config.low_memory_threshold_percent,
                        "Low-memory restart threshold reached"
                    );
                    consecutive_low_memory_checks = 0;
                    schedule_reboot("low_memory", 3)
                }
            } else {
                false
            }
        } else {
            consecutive_low_memory_checks = 0;
            false
        };

        // Low memory has priority when both conditions are true in one check.
        if low_memory_triggered {
            continue;
        }

        if config.schedule_enabled
            && uptime_seconds >= u64::from(config.schedule_interval_days) * 24 * 60 * 60
        {
            info!(days = config.schedule_interval_days, "Scheduled restart interval reached");
            schedule_reboot("scheduled", 3);
        }
    }
}

fn state_path() -> std::path::PathBuf {
    get_persistent_root_dir().join("restart_state.json")
}

fn read_restart_state() -> RestartState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn write_restart_state(state: &RestartState) {
    let path = state_path();
    let temporary_path = path.with_extension("json.new");
    let result = (|| -> Result<(), String> {
        let content = serde_json::to_vec(state)
            .map_err(|error| format!("Failed to serialize restart state: {}", error))?;
        fs::write(&temporary_path, content)
            .map_err(|error| format!("Failed to write restart state: {}", error))?;
        fs::rename(&temporary_path, &path)
            .map_err(|error| format!("Failed to publish restart state: {}", error))?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_file(temporary_path);
        warn!(error = %error, "Failed to persist restart state");
    }
}

fn low_memory_restart_limit_reached() -> bool {
    let now = chrono::Utc::now().timestamp();
    let state = read_restart_state();
    let Some(window_started_at) = state.low_memory_window_started_at else {
        return false;
    };

    now >= window_started_at
        && now.saturating_sub(window_started_at) < 24 * 60 * 60
        && state.low_memory_restart_count >= LOW_MEMORY_MAX_RESTARTS_PER_DAY
}

fn record_low_memory_restart() {
    let now = chrono::Utc::now().timestamp();
    let mut state = read_restart_state();
    let within_current_window = state
        .low_memory_window_started_at
        .is_some_and(|started_at| now >= started_at && now.saturating_sub(started_at) < 24 * 60 * 60);

    if within_current_window {
        state.low_memory_restart_count = state.low_memory_restart_count.saturating_add(1);
    } else {
        state.low_memory_window_started_at = Some(now);
        state.low_memory_restart_count = 1;
    }
    write_restart_state(&state);
}

#[cfg(test)]
mod tests {
    use super::{LOW_MEMORY_CONSECUTIVE_CHECKS, STARTUP_GRACE_SECONDS};

    #[test]
    fn watchdog_constants_require_sustained_low_memory_after_startup() {
        assert_eq!(LOW_MEMORY_CONSECUTIVE_CHECKS, 3);
        assert_eq!(STARTUP_GRACE_SECONDS, 600);
    }
}
