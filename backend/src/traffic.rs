//! 流量统计与用量预警
//!
//! 周期性地从活跃网络接口读取累计字节计数，聚合为按日累计值写入 SQLite。
//! 高频读取只做内存累加与 `/sys` 计数读取，磁盘写入频率很低（每 300 秒一次），
//! 对设备 flash 仅有可忽略的磨损，不影响长期稳定运行。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::config::ConfigManager;
use crate::db::Database;

/// 采样周期：300 秒（5 分钟）。计数器单调递增，错过周期的增量会被丢弃（不回溯）。
/// 选择 5 分钟间隔是为了平衡数据粒度与 flash 寿命（~105,120 次写入/年）。
const SAMPLE_INTERVAL_SECONDS: u64 = 300;
/// 预警去重：同日只通知一次，避免阈值边缘反复推送
const ALERT_RESET_MINUTES: i64 = 24 * 60;

/// 上次采样的接口计数缓存（进程内）
lazy_static::lazy_static! {
    static ref LAST_COUNTERS: std::sync::Mutex<HashMap<String, (u64, u64)>> =
        std::sync::Mutex::new(HashMap::new());
}

/// 上次预警时间戳（Unix 秒）
static LAST_ALERT_AT: AtomicU64 = AtomicU64::new(0);

/// 今日/本月累计已在数据库里维护，这里只负责采样 + 累加 + 预警。
/// 返回当日是否新增了流量（用于日志去重）。
pub async fn traffic_watchdog(db: Arc<Database>, config_manager: Arc<ConfigManager>) {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(SAMPLE_INTERVAL_SECONDS)).await;

        // 读取当前活跃接口的累加计数
        let interfaces = match crate::utils::get_active_interfaces() {
            Ok(list) => list,
            Err(_) => continue,
        };

        let mut cur_counters: HashMap<String, (u64, u64)> = HashMap::new();
        for interface in &interfaces {
            if let Ok((rx, tx)) = crate::utils::read_interface_stats(interface) {
                cur_counters.insert(interface.clone(), (rx, tx));
            }
        }

        // 计算相对上次采样的增量
        let mut last = match LAST_COUNTERS.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let mut delta_rx: u64 = 0;
        let mut delta_tx: u64 = 0;
        for (iface, (rx, tx)) in &cur_counters {
            if let Some((last_rx, last_tx)) = last.get(iface) {
                delta_rx = delta_rx.saturating_add(rx.saturating_sub(*last_rx));
                delta_tx = delta_tx.saturating_add(tx.saturating_sub(*last_tx));
            }
        }
        *last = cur_counters;

        // 若没有任何变化（首轮或无流量），跳过写盘
        if delta_rx == 0 && delta_tx == 0 {
            continue;
        }

        if let Err(e) = db.add_traffic_delta(delta_rx, delta_tx) {
            crate::log_entry!(warn, "traffic", "Failed to persist traffic delta: {}", e);
            continue;
        }

        // 用量预警
        check_alert(&config_manager, &db).await;
    }
}

async fn check_alert(config_manager: &ConfigManager, db: &Database) {
    let alert = config_manager.get_traffic_alert();
    if !alert.enabled || alert.daily_threshold_bytes == 0 {
        return;
    }

    let (today_rx, today_tx) = match db.get_todays_traffic() {
        Ok(totals) => totals,
        Err(_) => return,
    };
    let today_total = today_rx.saturating_add(today_tx);
    if today_total < alert.daily_threshold_bytes {
        return;
    }

    // 24 小时内去重
    let now = chrono::Utc::now().timestamp();
    let last = LAST_ALERT_AT.load(Ordering::SeqCst);
    if now.saturating_sub(last) < ALERT_RESET_MINUTES {
        return;
    }
    LAST_ALERT_AT.store(now, Ordering::SeqCst);

    crate::log_entry!(warn, "traffic", "Daily traffic threshold reached: {} bytes", today_total);
}

/// 读取今日与本月累计（供 handler 调用，返回字节）
pub fn read_stats(db: &Database) -> Result<(u64, u64, u64, u64), String> {
    let today = db.get_todays_traffic().map_err(|e| e.to_string())?;
    let month = db.get_months_traffic().map_err(|e| e.to_string())?;
    // today = (rx, tx)
    let (today_rx, today_tx) = today;
    let (month_rx, month_tx) = month;
    Ok((today_rx, today_tx, month_rx, month_tx))
}