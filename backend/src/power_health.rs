//! 低功耗健康看板（一期 — 非侵入只读）。
//!
//! 本模块观测三类**应用层省电信号**，用于直观展示 v3.8.0–3.8.5 低功耗重构的效果，
//! 不修改、不驱动任何后台任务：
//!
//! 1. **CPU 空闲度**：复用 [`crate::handlers::read_system_stats_cache`] 里已有 2s 采样
//!    得到的 busy%，`idle = 100 - busy`，零新增采样。
//! 2. **中断唤醒率**：首次读取 `/proc/interrupts` 累计计数，与上次请求的基线求差值、
//!    除以真实时间窗口，得到「自上次轮询以来的平均每秒中断次数」。按需采样，请求路径
//!    上不 sleep、不阻塞（`/proc` 读取放进 `spawn_blocking`）。
//! 3. **各看门狗规划间隔对照表**：只读列出各后台任务的真实计划周期。

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::config::ConfigManager;
use crate::models::*;

/// 中断采样基线：`(上次累计中断总数, 上次采样时刻)`。首次采样前为 `None`。
static INTERRUPT_STATE: LazyLock<Mutex<Option<(u64, Instant)>>> =
    LazyLock::new(|| Mutex::new(None));

/// 读取并汇总 `/proc/interrupts` 所有 CPU 列的中断累计计数。
///
/// - 跳过表头行（以空白 + `CPU` 开头）；
/// - 每行取首个空白分隔字段作为标签（`N:`/`IPI:`/`ERR:` …），其余字段
///   `filter_map` 解析为 `u64` 后 `saturating_add` 求和——表尾的中断名等非数字
///   字段自然被丢弃（与 [`crate::utils::parse_cpu_stat`] 同款手法）；
/// - `saturating_*` 防计数重置与溢出，任何读取/解析失败都以 `Err` 上抛，不 panic。
fn parse_interrupt_totals() -> Result<u64, String> {
    use std::fs;

    let content = fs::read_to_string("/proc/interrupts")
        .map_err(|e| format!("Failed to read /proc/interrupts: {}", e))?;

    let mut total: u64 = 0;
    for line in content.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            continue;
        }
        // 表头形如 "            CPU0       CPU1 ..."，跳过
        if line.starts_with("CPU") {
            continue;
        }
        let mut fields = line.split_whitespace();
        let _label = fields.next(); // "1:", "IPI0:", "ERR:" 等
        for field in fields {
            if let Ok(v) = field.parse::<u64>() {
                total = total.saturating_add(v);
            }
        }
    }
    Ok(total)
}

/// 按需采样中断唤醒率。
///
/// 返回 `None` 仅两种情况：`/proc/interrupts` 不可读，或本次是进程内首次采样
/// （尚无基线，均值无意义）。同时无论如何都会更新基线，供下一次请求计算窗口。
pub async fn sample_interrupt_rate() -> Option<InterruptRate> {
    let totals = match tokio::task::spawn_blocking(parse_interrupt_totals).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return None,
        Err(_) => return None,
    };

    let now = Instant::now();
    let mut guard = match INTERRUPT_STATE.lock() {
        Ok(g) => g,
        // 锁被毒化时尽力恢复，避免看板读取成为新的崩溃点
        Err(poisoned) => poisoned.into_inner(),
    };

    let prev = guard.replace((totals, now));

    let Some((prev_total, prev_instant)) = prev else {
        // 首次采样：仅建立基线，无速率可报
        return None;
    };

    let window = now.duration_since(prev_instant);
    let window_secs = window.as_secs_f64();
    if window_secs <= 0.0 {
        return None;
    }

    let delta = totals.saturating_sub(prev_total);
    Some(InterruptRate {
        irqs_per_sec: delta as f64 / window_secs,
        total_interrupts: totals,
        window_secs,
    })
}

/// 构造 10 行看门狗规划间隔表。读取 4 个实时配置，其余为编译期常量。
fn build_planned_intervals(config: &ConfigManager) -> Vec<PlannedInterval> {
    let net_health = config.get_net_health();
    let restart = config.get_restart();
    let schedule = config.get_schedule();
    let refresh = config.get_refresh();

    vec![
        PlannedInterval {
            key: "net_health".into(),
            name: "断网自愈探测".into(),
            interval_secs: if net_health.enabled { net_health.interval_secs } else { 0 },
            interval_text: if net_health.enabled {
                format!("自适应 {}s×1.5 上限120s，失败重置", net_health.interval_secs)
            } else {
                "已禁用".into()
            },
            classification: "adaptive".into(),
            enabled: net_health.enabled,
            note: "默认 60s；探测成功后 1.5 倍退避封顶 120s，失败立即回到基础间隔".into(),
        },
        PlannedInterval {
            key: "restart".into(),
            name: "周期/低内存重启".into(),
            interval_secs: if restart.schedule_enabled || restart.low_memory_enabled { 60 } else { 3600 },
            interval_text: "60s 检查 / 1h 低功耗休眠".into(),
            classification: "adaptive".into(),
            enabled: restart.schedule_enabled || restart.low_memory_enabled,
            note: "10 分钟启动宽限；低内存连续 3 次才重启，每日至多 1 次".into(),
        },
        PlannedInterval {
            key: "schedule".into(),
            name: "定时计划".into(),
            interval_secs: 0,
            interval_text: "自适应：距下一计划唤醒，上限 3600s".into(),
            classification: "adaptive".into(),
            enabled: schedule.entries.iter().any(|e| e.enabled),
            note: "无计划项时 1h 兜底休眠".into(),
        },
        PlannedInterval {
            key: "data_connection".into(),
            name: "数据连接自愈".into(),
            interval_secs: 0,
            interval_text: "90s（已连接）/ 2s（断线）".into(),
            classification: "adaptive".into(),
            enabled: true,
            note: "断网 2s 高频重试以快速恢复，空闲设备也能自愈".into(),
        },
        PlannedInterval {
            key: "traffic".into(),
            name: "流量统计采样".into(),
            interval_secs: 300,
            interval_text: "300s 固定".into(),
            classification: "low_power".into(),
            enabled: true,
            note: "低频采样读库归档，避免频繁写 flash".into(),
        },
        PlannedInterval {
            key: "db_cleanup".into(),
            name: "数据库清理".into(),
            interval_secs: 86400,
            interval_text: "86400s（24h）".into(),
            classification: "low_power".into(),
            enabled: true,
            note: "短信/通话记录仅保留最近 N 条，防止 data.db 占满分区".into(),
        },
        PlannedInterval {
            key: "stats_cache".into(),
            name: "系统状态缓存采样".into(),
            interval_secs: 2,
            interval_text: "约 2s".into(),
            classification: "active".into(),
            enabled: true,
            note: "预热 /api/stats 与诊断报告；整机唯一 2s 高频项".into(),
        },
        PlannedInterval {
            key: "mqtt_keepalive".into(),
            name: "MQTT 保活/心跳".into(),
            interval_secs: 120,
            interval_text: "120s keepalive / 900s 心跳".into(),
            classification: "event_driven".into(),
            enabled: false,
            note: "仅 MQTT 遥控开启时生效，默认关闭".into(),
        },
        PlannedInterval {
            key: "refresh_heartbeat".into(),
            name: "前端刷新/看门狗".into(),
            interval_secs: refresh.interval_ms / 1000,
            interval_text: format!(
                "活动 {}ms / 空闲×6 上限 120s",
                refresh.active_watchdog_interval_ms()
            ),
            classification: "low_power".into(),
            enabled: refresh.interval_ms > 0,
            note: "前端空闲时看门狗降频至 120s 级".into(),
        },
        PlannedInterval {
            key: "irq_rate".into(),
            name: "中断唤醒率采样".into(),
            interval_secs: 0,
            interval_text: "按需轮询（默认 ≥60s）".into(),
            classification: "on_demand".into(),
            enabled: true,
            note: "本页面首次读取 /proc/interrupts，按轮询间隔求均值".into(),
        },
    ]
}

/// GET /api/power/health - 低功耗健康看板（只读）
pub async fn get_power_health(
    State(config_manager): State<Arc<ConfigManager>>,
) -> (StatusCode, Json<ApiResponse<PowerHealthResponse>>) {
    // CPU 空闲度：复用 2s 采样缓存，零新增采样。冷启动缓存未就绪时退化为 0。
    let cpu = match crate::handlers::read_system_stats_cache() {
        Some(stats) => CpuIdleInfo {
            busy_percent: stats.cpu_load.load_percent,
            idle_percent: (100.0 - stats.cpu_load.load_percent).clamp(0.0, 100.0),
            load_1min: stats.cpu_load.load_1min,
            core_count: stats.cpu_load.core_count,
        },
        None => CpuIdleInfo::default(),
    };

    let wakeups = sample_interrupt_rate().await;

    // irq_rate 行的 enabled 反映本次中断采样是否成功（有基线即成功）
    let mut planned_intervals = build_planned_intervals(&config_manager);
    if let Some(interval) = planned_intervals.iter_mut().find(|p| p.key == "irq_rate") {
        interval.enabled = wakeups.is_some();
    }

    let response = PowerHealthResponse {
        cpu,
        wakeups,
        planned_intervals,
        sampled_at: crate::utils::beijing_now_rfc3339(),
    };

    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", response)),
    )
}