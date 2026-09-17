//! 定时计划 watchdog
//!
//! 每隔 30 秒检查一次已启用的计划项，到点且位于容差窗口内即触发对应动作。
//! 所有动作都通过 ofono D-Bus 或系统接口执行，复用现有 `dbus.rs` / `restart.rs`。
//! 默认无任何计划项，不影响既有设备行为。

use std::sync::Arc;

use chrono::{Datelike, Timelike};

use crate::config::{ConfigManager, ScheduleAction};
use zbus::Connection;

const CHECK_INTERVAL_SECONDS: u64 = 30;

/// 计划执行 watchdog
pub async fn schedule_watchdog(conn: Arc<Connection>, config_manager: Arc<ConfigManager>) {
    let mut fired_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut last_fired_minute: Option<u32> = None;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(CHECK_INTERVAL_SECONDS)).await;

        let config = config_manager.get_schedule();
        let tolerance_min = u64::from(config.tolerance_min.max(1));

        // 以“当日分钟数”作为窗口边界：跨到新的分钟时清空去重集合，避免上一分钟的
        // 记录导致新窗口内的计划项被误跳过。
        let current_minute = minute_of_day(chrono::Local::now());
        if last_fired_minute != Some(current_minute) {
            fired_keys.clear();
            last_fired_minute = Some(current_minute);
        }

        for entry in &config.entries {
            if !entry.enabled {
                continue;
            }
            if !is_within_schedule_window(&entry.time, tolerance_min, &entry.weekdays) {
                continue;
            }

            // 每个计划项（时间+动作）在同一个分钟窗口内只触发一次；不同计划项互不覆盖。
            let fire_key = format!("{}|{:?}", entry.time, entry.action);
            if !fired_keys.insert(fire_key.clone()) {
                continue;
            }

            execute_action(&conn, entry.action).await;
            crate::log_entry!(info, "schedule", "Scheduled action {:?} fired at local {}", entry.action, entry.time);
        }
    }
}

/// 当前本地时间的“当日分钟数”（0..1440）。
fn minute_of_day(now: chrono::DateTime<chrono::Local>) -> u32 {
    u32::from(now.hour()) * 60 + u32::from(now.minute())
}

/// 判断当前本地时间是否落在计划的触发窗口内。
///
/// 时间差按“环形”计算（00:00 与 23:59 只差 1 分钟），避免零点前后的计划因线性
/// 差值（1439 分钟）被错误判定为窗口外。
fn is_within_schedule_window(time: &str, tolerance_min: u64, weekdays: &[u8]) -> bool {
    let now = chrono::Local::now();
    let now_minutes = minute_of_day(now);

    // 解析计划时间 HH:MM
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    let (Ok(hour), Ok(minute)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
        return false;
    };
    if hour > 23 || minute > 59 {
        return false;
    }
    let target = hour.saturating_mul(60).saturating_add(minute);

    let linear = now_minutes.abs_diff(target);
    let circular = linear.min(1440 - linear);
    let within_time = circular <= tolerance_min as u32;

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
    use chrono::Timelike;

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