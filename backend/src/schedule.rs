//! 定时计划 watchdog
//!
//! 每隔 30 秒检查一次已启用的计划项，到点且位于容差窗口内即触发对应动作。
//! 所有动作都通过 ofono D-Bus 或系统接口执行，复用现有 `dbus.rs` / `restart.rs`。
//! 默认无任何计划项，不影响既有设备行为。

use std::sync::Arc;

use crate::config::{ConfigManager, ScheduleAction};
use zbus::Connection;

const CHECK_INTERVAL_SECONDS: u64 = 30;

/// 计划执行 watchdog
pub async fn schedule_watchdog(conn: Arc<Connection>, config_manager: Arc<ConfigManager>) {
    let mut last_fired: Option<(String, String)> = None;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(CHECK_INTERVAL_SECONDS)).await;

        let config = config_manager.get_schedule();
        let tolerance_min = u64::from(config.tolerance_min.max(1));

        for entry in &config.entries {
            if !entry.enabled {
                continue;
            }
            if !is_within_schedule_window(&entry.time, tolerance_min, &entry.weekdays) {
                continue;
            }

            // 每个计划项在同一个分钟窗口内只触发一次
            let fire_key = (entry.time.clone(), format!("{:?}", entry.action));
            if last_fired.as_ref() == Some(&fire_key) {
                continue;
            }

            execute_action(&conn, entry.action).await;
            crate::log_entry!(info, "schedule", "Scheduled action {:?} fired at local {}", entry.action, entry.time);
            last_fired = Some(fire_key);
        }
    }
}

/// 判断当前本地时间是否落在计划的触发窗口内
fn is_within_schedule_window(time: &str, tolerance_min: u64, weekdays: &[u8]) -> bool {
    let now = chrono::Local::now();
    let now_minutes = u32::from(now.hour()) * 60 + u32::from(now.minute());

    // 解析计划时间 HH:MM
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    let (Ok(hour), Ok(minute)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
        return false;
    };
    let target = hour.saturating_mul(60).saturating_add(minute);

    let diff = now_minutes.abs_diff(target);
    let within_time = diff <= tolerance_min as u32;

    // 周几过滤：空表示每天，否则要求当天匹配（0=周日）
    let weekday_ok = weekdays.is_empty()
        || weekdays.contains(&(now.weekday().num_days_from_sunday() as u8));

    within_time && weekday_ok
}

/// 执行计划动作
async fn execute_action(conn: &Connection, action: ScheduleAction) {
    match action {
        ScheduleAction::Reboot => {
            let _ = crate::restart::schedule_reboot("scheduled_task", 3);
        }
        ScheduleAction::AirplaneOn => {
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
        }
        ScheduleAction::AirplaneOff => {
            let _ = crate::dbus::set_airplane_mode(conn, false).await;
        }
        ScheduleAction::DataOn => {
            let _ = crate::dbus::set_data_connection(conn, true).await;
        }
        ScheduleAction::DataOff => {
            let _ = crate::dbus::set_data_connection(conn, false).await;
        }
        ScheduleAction::RadioLte => {
            let _ = crate::dbus::set_radio_mode(conn, crate::models::RadioMode::LteOnly).await;
        }
        ScheduleAction::RadioNr => {
            let _ = crate::dbus::set_radio_mode(conn, crate::models::RadioMode::NrOnly).await;
        }
        ScheduleAction::RadioAuto => {
            let _ = crate::dbus::set_radio_mode(conn, crate::models::RadioMode::Auto).await;
        }
        ScheduleAction::RadioOff => {
            // 关闭射频：设置 Modem Online=false（与飞行模式一致，但语义上为“关闭射频”）
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_within_schedule_window;

    #[test]
    fn schedule_window_rejects_invalid_time() {
        assert!(!is_within_schedule_window("25:00", 1, &[]));
        assert!(!is_within_schedule_window("bad", 1, &[]));
        assert!(!is_within_schedule_window("12", 1, &[]));
    }

    #[test]
    fn schedule_window_accepts_any_weekday_when_empty() {
        let now = chrono::Local::now();
        let target = format!("{:02}:{:02}", now.hour(), now.minute());
        assert!(is_within_schedule_window(&target, 1, &[]));
    }
}