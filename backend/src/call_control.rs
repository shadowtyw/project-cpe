//! 通话远程指令控制
//!
//! 监听来电：当来电号码匹配白名单时自动接听，接通后所有动作同时计时，各自在到达
//! 配置的等待时长后触发对应动作。支持多条独立动作（如 10s 重启、15s 飞行、20s 切4G）。
//! 用于无数据网络环境下的远程管理——用一部手机拨打设备号码即可触发多种预配置操作。
//!
//! 飞行模式相关动作（AirplaneOn / RadioOff）触发后 10 秒自动恢复网络，防止设备永久断网。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::config::{normalize_phone_number, CallControlAction, CallControlConfig, ScheduleAction};
use zbus::Connection;

/// 最近一次触发的记录（供 GET 查询与页面展示）
struct TriggerState {
    triggered_at: Option<String>,
    action: Option<String>,
    from_number: Option<String>,
}

static TRIGGER_STATE: Mutex<TriggerState> = Mutex::new(TriggerState {
    triggered_at: None,
    action: None,
    from_number: None,
});

/// 正在计时的通话。key 为 ofono 通话对象路径。
static ACTIVE_TIMERS: Mutex<Option<HashMap<String, ActiveTimer>>> = Mutex::new(None);

/// 防止同一时刻对同一触发动作重复调度（如重启）。
static PROCESSING: AtomicBool = AtomicBool::new(false);

struct ActiveTimer {
    number: String,
    begin: Instant,
    fired: bool,
    duration: Duration,
    action: ScheduleAction,
}

/// RAII guard，确保 PROCESSING 标志在异常时也能复位。
struct ProcessingGuard;

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        PROCESSING.store(false, Ordering::SeqCst);
    }
}

/// 处理一条新的来电（CallAdded，状态为 incoming/alerting）。
///
/// 命中白名单时：自动接听并开始计时；返回 true。未命中或未启用时返回 false。
pub async fn on_incoming_call(
    conn: &Connection,
    config: &CallControlConfig,
    path: &str,
    number: &str,
) -> bool {
    if !config.enabled || config.numbers.is_empty() {
        return false;
    }

    let normalized = normalize_phone_number(number);
    if !is_whitelisted(&config.numbers, &normalized) {
        return false;
    }

    // 自动接听，随后 ofono 会抛 State -> active 信号，无需在此重复计时。
    let _ = crate::dbus::answer_call(conn, path).await;

    // 立即开始所有动作的计时，即使接听失败也保留计时器，poll 会到期执行（并尝试挂断）。
    start_timers(path, normalized, &config.actions);
    true
}

/// 通话结束（CallRemoved）时清理该通话的所有定时器。
pub fn on_call_removed(path: &str) {
    let mut timers = lock_timers();
    if let Some(map) = timers.as_mut() {
        // 移除所有以 path 开头的 key（多动作定时器格式：path::hold_seconds）
        map.retain(|key, _| !key.starts_with(path));
    }
}

/// 定时轮询入口：由主循环周期调用，检查到期的计时器并执行动作。
pub async fn poll(conn: &Connection, _config: &CallControlConfig) {
    let due: Vec<(String, ActiveTimer)> = {
        let mut timers = lock_timers();
        let map = match timers.as_mut() {
            Some(map) => map,
            None => return,
        };
        let expired: Vec<String> = map
            .iter()
            .filter(|(_, timer)| !timer.fired && timer.begin.elapsed() >= timer.duration)
            .map(|(path, _)| path.clone())
            .collect();

        let mut due = Vec::new();
        for path in expired {
            if let Some(timer) = map.remove(&path) {
                due.push((path, timer));
            }
        }
        due
    };

    for (path, timer) in due {
        if PROCESSING.swap(true, Ordering::SeqCst) {
            continue;
        }
        let _guard = ProcessingGuard;

        crate::log_entry!(
            info,
            "call_control",
            "Call control triggered: number={} action={:?} hold_seconds={}",
            timer.number,
            timer.action,
            timer.duration.as_secs()
        );
        record_trigger(&timer.number, timer.action);

        execute_action(conn, timer.action).await;

        // 挂断本次通话，避免持续占用线路（重启类动作挂断与否都会重启）。
        let _ = crate::dbus::hangup_call(conn, &path).await;
    }
}

/// 为一次来电创建多条定时器，每条动作对应一个独立的计时器。
/// 计时器互不影响，各自在到达 hold_seconds 后触发对应动作。
fn start_timers(path: &str, number: String, actions: &[CallControlAction]) {
    if actions.is_empty() {
        return;
    }
    let mut timers = lock_timers();
    let map = timers.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    for action in actions {
        // 为每条动作生成一个唯一 key（path + index）
        let key = format!("{}::{}", path, action.hold_seconds);
        map.entry(key).or_insert_with(|| ActiveTimer {
            number: number.clone(),
            begin: now,
            fired: false,
            duration: Duration::from_secs(action.hold_seconds),
            action: action.action,
        });
    }
}

fn lock_timers() -> MutexGuard<'static, Option<HashMap<String, ActiveTimer>>> {
    match ACTIVE_TIMERS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 号码是否命中白名单：规范化后，以任一白名单号码结尾即匹配。
fn is_whitelisted(numbers: &[String], normalized: &str) -> bool {
    if normalized.is_empty() {
        return false;
    }
    numbers.iter().any(|entry| {
        let entry = normalize_phone_number(entry);
        !entry.is_empty() && (normalized == entry || normalized.ends_with(&entry))
    })
}

async fn execute_action(conn: &Connection, action: ScheduleAction) {
    match action {
        ScheduleAction::Reboot => {
            let _ = crate::restart::schedule_reboot("call_control", 3);
        }
        ScheduleAction::AirplaneOn => {
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
            spawn_airplane_recovery(conn);
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
            let _ = crate::dbus::set_airplane_mode(conn, true).await;
            spawn_airplane_recovery(conn);
        }
    }
}

/// 飞行模式自动恢复：10 秒后关闭飞行模式并开启数据连接，防止远程指令导致
/// 设备永久断网（无论是短信遥控还是通话遥控触发）。
fn spawn_airplane_recovery(conn: &Connection) {
    let conn = conn.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        tracing::info!("Call control airplane auto-recovery: turning off airplane mode");
        let _ = crate::dbus::set_airplane_mode(&conn, false).await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        tracing::info!("Call control airplane auto-recovery: enabling data connection");
        let _ = crate::dbus::set_data_connection(&conn, true).await;
        tracing::info!("Call control airplane auto-recovery completed");
    });
}

fn record_trigger(number: &str, action: ScheduleAction) {
    let mut state = match TRIGGER_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.triggered_at = Some(chrono::Utc::now().to_rfc3339());
    state.action = Some(format!("{:?}", action).to_lowercase());
    state.from_number = Some(number.to_string());
}

/// 读取上次触发的通话遥控记录
pub fn get_last_trigger() -> crate::models::CallControlTrigger {
    let state = match TRIGGER_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    crate::models::CallControlTrigger {
        triggered_at: state.triggered_at.clone(),
        action: state.action.clone(),
        from_number: state.from_number.clone(),
    }
}