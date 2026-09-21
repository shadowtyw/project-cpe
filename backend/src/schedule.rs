//! 定时计划 watchdog
//!
//! v3.8.4：从固定 30s 轮询改为"睡眠到下一个计划触发窗口"模式。
//! 每次迭代计算距离最近的未来计划项还有多少分钟，直接 sleep 到窗口前
//! 若干秒再醒来扫描。无计划项或全部禁用时以 3600s（1h）兜底，
//! 相比旧版减少 99% 的无意义唤醒。

use std::sync::Arc;

use chrono::{Datelike, Timelike};

use crate::config::{ConfigManager, ScheduleAction};
use zbus::Connection;

/// 无计划项时的兜底睡眠间隔（秒）。
const IDLE_SLEEP_SECS: u64 = 3600;

/// 计划执行 watchdog
pub async fn schedule_watchdog(conn: Arc<Connection>, config_manager: Arc<ConfigManager>) {
    let mut fired_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut last_fired_minute: Option<u32> = None;

    loop {
        let config = config_manager.get_schedule();
        let tolerance_min = u64::from(config.tolerance_min.max(1));

        // 以"当日分钟数"作为窗口边界：跨到新的分钟时清空去重集合，避免上一分钟的
        // 记录导致新窗口内的计划项被误跳过。
        let current_minute = minute_of_day(chrono::Local::now());
        if last_fired_minute != Some(current_minute) {
            fired_keys.clear();
            last_fired_minute = Some(current_minute);
        }

        // 扫描当前分钟窗口内是否有机触发计划项
        let mut any_fired = false;
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
            any_fired = true;
        }

        // 计算距下一个最近计划项的睡眠时长，实现自适应唤醒
        let sleep_secs = next_wake_seconds(&config, tolerance_min, current_minute);
        if any_fired {
            // 刚触发过动作，短暂休眠让系统稳定后再算下一次
            tokio::time::sleep(tokio::time::Duration::from_secs(
                sleep_secs.min(60)
            )).await;
        } else {
            tokio::time::sleep(tokio::time::Duration::from_secs(sleep_secs)).await;
        }
    }
}

/// 计算距下一个启用计划项触发窗口还有多少秒。
///
/// 返回 0 表示当前已在某窗口内（立即唤醒扫描），IDLE_SLEEP_SECS 表示无计划项。
fn next_wake_seconds(
    config: &crate::config::ScheduleConfig,
    tolerance_min: u64,
    current_minute: u32,
) -> u64 {
    let enabled_entries: Vec<_> = config.entries.iter().filter(|e| e.enabled).collect();
    if enabled_entries.is_empty() {
        return IDLE_SLEEP_SECS;
    }

    // 解析所有目标分钟并找最近的下一个
    let mut nearest_diff: Option<u32> = None;
    for entry in &enabled_entries {
        // 周几过滤：只参与今天有资格的条目
        let now = chrono::Local::now();
        if !entry.weekdays.is_empty()
            && !entry.weekdays.contains(&(now.weekday().num_days_from_sunday() as u8))
        {
            // 今天不触发，算到明天的同一时间（+1440 分钟）
            if let Some(target) = parse_time_minutes(&entry.time) {
                // 今天已过，下次是明天此时：remaining = (1440 - current) + target
                let remaining = (1440 - current_minute) + target;
                match nearest_diff {
                    None => nearest_diff = Some(remaining),
                    Some(best) if remaining < best => nearest_diff = Some(remaining),
                    _ => {}
                }
            }
            continue;
        }

        if let Some(target) = parse_time_minutes(&entry.time) {
            let diff = target.wrapping_sub(current_minute) % 1440;
            if diff == 0 {
                // 正在窗口内
                return 0;
            }
            match nearest_diff {
                None => nearest_diff = Some(diff),
                Some(best) if diff < best => nearest_diff = Some(diff),
                _ => {}
            }
        }
    }

    let minutes = nearest_diff.unwrap_or(IDLE_SLEEP_SECS as u32 / 60);
    if minutes == 0 {
        return 0;
    }

    // 在窗口前 tolerance_min + 1 分钟处醒来，确保到点能扫描到
    let wake_minutes = minutes.saturating_sub(tolerance_min as u32 + 1).max(1);
    // 上限 3600s：即使下一个计划在明天，也每小时醒来重读配置（允许计划项增删生效）
    (wake_minutes as u64 * 60).min(IDLE_SLEEP_SECS)
}

/// 解析 HH:MM 为当日分钟数。非法格式返回 None。
fn parse_time_minutes(time: &str) -> Option<u32> {
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour = parts[0].parse::<u32>().ok()?;
    let minute = parts[1].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// 当前本地时间的"当日分钟数"（0..1440）。
fn minute_of_day(now: chrono::DateTime<chrono::Local>) -> u32 {
    u32::from(now.hour()) * 60 + u32::from(now.minute())
}

/// 判断当前本地时间是否落在计划的触发窗口内。
///
/// 时间差按"环形"计算（00:00 与 23:59 只差 1 分钟），避免零点前后的计划因线性
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
            // 关闭射频：设置 Modem Online=false（与飞行模式一致，但语义上为"关闭射频"）
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