//! 内存日志缓冲模块
//!
//! 使用环形缓冲在进程内存中保留最近的应用日志，供 Web 管理页面查看。
//! 不写磁盘、不引入额外 IO，避免对设备 flash 造成磨损，适合长期运行。
//!
//! 日志等级沿用 tracing 的语义，通过 `min_level` 支持页面筛选：
//! 查看“运行日志”传 `min_level = 0`（全部），查看“报错日志”传 `min_level = 2`
//! （仅 warn 与 error）。进程重启后缓冲清空。

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

/// 单条日志记录
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    /// ISO 8601 时间戳
    pub timestamp: String,
    /// 日志等级：debug / info / warn / error
    pub level: String,
    /// 来源模块
    pub module: String,
    /// 日志正文
    pub message: String,
}

/// 缓冲容量上限，超过后丢弃最旧记录
const MAX_LOG_ENTRIES: usize = 2000;

static LOG_BUFFER: LazyLock<Mutex<VecDeque<LogEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// 日志等级到数值的映射，用于 min_level 筛选
fn level_rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warn" => 2,
        "error" => 3,
        _ => 1,
    }
}

fn now_rfc3339() -> String {
    crate::utils::beijing_now_rfc3339()
}

/// 内部写入逻辑。锁被毒化时尽量恢复，避免日志写入成为新的崩溃点。
fn push(level: &str, module: &str, message: String) {
    // 在锁外获取时间戳，避免持有 Mutex 期间调用 chrono 的潜在阻塞。
    let timestamp = now_rfc3339();

    let entry = LogEntry {
        timestamp,
        level: level.to_string(),
        module: module.to_string(),
        message,
    };

    let mut buffer = match LOG_BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if buffer.len() >= MAX_LOG_ENTRIES {
        buffer.pop_front();
    }
    buffer.push_back(entry);
}

/// 记录一条 info 日志
pub fn info(module: &str, message: impl Into<String>) {
    push("info", module, message.into());
}

/// 记录一条 warn 日志
pub fn warn(module: &str, message: impl Into<String>) {
    push("warn", module, message.into());
}

/// 记录一条 error 日志。仅通过 `log_entry!` 宏间接调用，直接调用点可能为零。
#[allow(dead_code)]
pub fn error(module: &str, message: impl Into<String>) {
    push("error", module, message.into());
}

/// 读取日志快照，最新的记录在前。
///
/// # Arguments
/// * `min_level` - 最小日志等级数值（0=debug, 1=info, 2=warn, 3=error）
/// * `limit` - 最多返回条数
pub fn snapshot(min_level: u8, limit: usize) -> Vec<LogEntry> {
    snapshot_filtered(min_level, limit, None)
}

/// 读取日志快照，可选按模块过滤。
///
/// # Arguments
/// * `min_level` - 最小日志等级数值（0=debug, 1=info, 2=warn, 3=error）
/// * `limit` - 最多返回条数
/// * `module` - 仅返回这些模块的日志；`None` 表示不过滤。
///   支持逗号分隔的多个模块名（如 `"mqtt,mqtt_service"`），命中任意一个即保留。
///   用于各功能页面（如 MQTT 遥控）展示自己的独立日志，而系统日志页保持全量。
///   传逗号列表是因为同一功能可能有两个来源：`log_entry!` 显式写的模块名
///   （如 `mqtt`）与 tracing 从 target 推导的模块名（如 `mqtt_service`）。
pub fn snapshot_filtered(min_level: u8, limit: usize, module: Option<&str>) -> Vec<LogEntry> {
    let buffer = match LOG_BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let limit = limit.clamp(1, MAX_LOG_ENTRIES);
    // 过滤条件先归一化：空字符串等同于「不过滤」，避免前端传空值时得到空列表。
    let wanted: Vec<&str> = module
        .map(|m| {
            m.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    buffer
        .iter()
        .rev()
        .filter(|entry| level_rank(&entry.level) >= min_level)
        .filter(|entry| wanted.is_empty() || wanted.iter().any(|m| *m == entry.module))
        .take(limit)
        .cloned()
        .collect()
}

/// 列出缓冲中出现过的全部模块名（去重并按字典序排序）。
///
/// 供前端「系统日志」页生成模块筛选下拉，避免把模块名硬编码到前端后与实际不符。
pub fn modules() -> Vec<String> {
    let buffer = match LOG_BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in buffer.iter() {
        seen.insert(entry.module.clone());
    }
    seen.into_iter().collect()
}

/// 清空内存日志缓冲
pub fn clear() {
    let mut buffer = match LOG_BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    buffer.clear();
}

/// 自己通过 `log_entry!` 显式写缓冲的模块，其 **info/debug 级** tracing 事件不再重复入缓冲。
///
/// 背景：`mqtt_service.rs` 里每个关键事件都成对出现——一行 `tracing::info!`（英文，
/// 走 fmt layer 到 stdout，同时被本 Layer 按 target 推导出模块名 `mqtt_service`）
/// 和一行 `log_entry!(info, "mqtt", ...)`（中文，显式写入模块名 `mqtt`）。
/// 两行都进缓冲，于是 Web 日志页里每条事件显示两遍，排障时要读双倍的内容。
///
/// 这里按 target 末段过滤掉前者的 info/debug。保留 tracing 调用本身是有意的：
/// 它仍然输出到 stdout，`RUST_LOG=debug` 时能在串口/控制台看到英文细节
/// （`log_entry!` 只写缓冲，不经过 tracing，串口上是看不到的），
/// 只是不再占用 Web 页面的缓冲。
///
/// **warn/error 一律不过滤**：宁可让这两级在页面上出现中英各一条，也不能冒
/// 「将来有人在 mqtt_service.rs 加了 `error!` 却忘了配 `log_entry!`，于是这条错误
/// 在 Web 页面上彻底消失」的风险。设备无人值守，看不见的错误比重复的错误危险得多。
/// 重复的代价只是多一行噪音，丢失的代价是排障时完全没有线索。
///
/// 新增模块若同样双写，把它的 target 末段加进来即可。
const SELF_LOGGING_MODULES: &[&str] = &["mqtt_service"];

/// 判断一条 tracing 事件是否应因「双写去重」而被丢弃。
///
/// 抽成纯函数是为了能直接单元测试这条规则，不必在测试里搭一套 tracing subscriber。
fn should_dedup(module: &str, level: &str) -> bool {
    let is_routine = level == "info" || level == "debug";
    is_routine && SELF_LOGGING_MODULES.contains(&module)
}

/// tracing Layer：把进程的 tracing 日志转发到内存环形缓冲，供“系统日志”页面查看。
///
/// 使用 `with_target(false)` 后 `target` 为空；这里从事件元数据的 `target` 提取模块名，
/// 并格式化 message 字段作为日志正文。等级字符串取自 `Level`（小写，与页面约定一致）。
pub struct LogBufferLayer;

impl<S> tracing_subscriber::Layer<S> for LogBufferLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let level = metadata.level().as_str().to_lowercase();

        // 模块名取 target 的最后一段（如 "udx710::dbus" -> "dbus"），既保证页面可读，
        // 又兼容 RUST_LOG 开启后 target 为空的场景。
        let module = metadata
            .target()
            .rsplit("::")
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("app")
            .to_string();

        // 双写模块的 info/debug 跳过，避免与它们自己的 log_entry! 重复；
        // warn/error 保留（理由见 SELF_LOGGING_MODULES 的文档）
        if should_dedup(&module, &level) {
            return;
        }

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        push(&level, &module, visitor.message);
    }
}

/// 简单字段访问器：采集 `message` 字段，缺省时回退到 `log.message` 或按 Debug 覆盖剩余字段。
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" || field.name() == "log.message" {
            self.message = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" || field.name() == "log.message" {
            self.message = value.to_string();
        }
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        if field.name() == "message" || field.name() == "log.message" {
            self.message = value.to_string();
        }
    }
}

/// 记录一条日志的便捷宏。
///
/// # Example
/// ```rust
/// crate::log_entry!(error, "sms", "Failed to insert SMS: {}", e);
/// ```
#[macro_export]
macro_rules! log_entry {
    (info, $module:expr, $($arg:tt)*) => {
        $crate::log_buffer::info($module, format!($($arg)*))
    };
    (warn, $module:expr, $($arg:tt)*) => {
        $crate::log_buffer::warn($module, format!($($arg)*))
    };
    (error, $module:expr, $($arg:tt)*) => {
        $crate::log_buffer::error($module, format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::snapshot;

    #[test]
    fn snapshot_supports_min_level_filtering() {
        let module = "test_snapshot_supports_min_level_filtering";
        super::info(module, "info message");
        super::warn(module, "warn message");
        super::error(module, "error message");

        let all: Vec<_> = snapshot(0, 500)
            .into_iter()
            .filter(|e| e.module == module)
            .collect();
        assert_eq!(all.len(), 3);

        let warnings_up: Vec<_> = snapshot(2, 500)
            .into_iter()
            .filter(|e| e.module == module)
            .collect();
        assert_eq!(warnings_up.len(), 2);
        assert!(warnings_up.iter().all(|e| e.level == "warn" || e.level == "error"));
    }

    #[test]
    fn snapshot_returns_newest_first() {
        let module = "test_snapshot_returns_newest_first";
        super::info(module, "first");
        super::info(module, "second");

        let entries: Vec<_> = snapshot(0, 500)
            .into_iter()
            .filter(|e| e.module == module)
            .collect();
        assert_eq!(entries[0].message, "second");
        assert_eq!(entries[1].message, "first");
    }

    #[test]
    fn module_filter_returns_only_matching_module() {
        let target = "test_module_filter_returns_only_matching_module";
        let other = "test_module_filter_other_module";
        super::info(target, "mine");
        super::warn(other, "not mine");

        let filtered = super::snapshot_filtered(0, 500, Some(target));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].message, "mine");
        assert!(filtered.iter().all(|e| e.module == target));
    }

    #[test]
    fn module_filter_combines_with_level_filter() {
        let target = "test_module_filter_combines_with_level_filter";
        super::info(target, "info line");
        super::error(target, "error line");

        // min_level=3 只保留 error，且模块过滤仍然生效
        let errors = super::snapshot_filtered(3, 500, Some(target));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "error line");
    }

    #[test]
    fn comma_separated_module_filter_matches_any() {
        let a = "test_comma_module_a";
        let b = "test_comma_module_b";
        let c = "test_comma_module_c";
        super::info(a, "from a");
        super::info(b, "from b");
        super::info(c, "from c");

        // MQTT 页同时取 log_entry! 写的模块名与 tracing 推导的模块名
        let filtered = super::snapshot_filtered(0, 500, Some(&format!("{a},{b}")));
        let msgs: Vec<_> = filtered.iter().map(|e| e.message.as_str()).collect();
        assert!(msgs.contains(&"from a"), "缺少 a: {msgs:?}");
        assert!(msgs.contains(&"from b"), "缺少 b: {msgs:?}");
        assert!(!msgs.contains(&"from c"), "不应包含 c: {msgs:?}");
    }

    #[test]
    fn comma_separated_filter_ignores_empty_segments() {
        let a = "test_comma_empty_segments";
        super::info(a, "mine");
        // 前后逗号与空格不应影响匹配
        let filtered = super::snapshot_filtered(0, 500, Some(&format!(" , {a} , ")));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].message, "mine");
    }

    #[test]
    fn empty_module_filter_is_treated_as_no_filter() {
        let target = "test_empty_module_filter_is_treated_as_no_filter";
        super::info(target, "some line");

        // 空字符串 / 纯空白不应把结果清空
        assert!(!super::snapshot_filtered(0, 500, Some("")).is_empty());
        assert!(!super::snapshot_filtered(0, 500, Some("   ")).is_empty());
    }

    /// 双写去重规则：mqtt_service 的 info/debug 丢弃（避免与 log_entry! 重复），
    /// 但 warn/error 必须保留，且未登记的模块完全不受影响。
    #[test]
    fn self_logging_module_routine_events_are_deduped() {
        use super::should_dedup;

        // info/debug：丢弃
        assert!(should_dedup("mqtt_service", "info"));
        assert!(should_dedup("mqtt_service", "debug"));

        // warn/error：保留。宁可页面上中英各一条，也不能让将来漏配 log_entry! 的
        // 错误在 Web UI 上彻底消失——无人值守设备看不见错误比重复错误危险得多。
        assert!(!should_dedup("mqtt_service", "warn"));
        assert!(!should_dedup("mqtt_service", "error"));

        // 未登记为双写的模块：任何等级都不过滤
        for level in ["debug", "info", "warn", "error"] {
            assert!(!should_dedup("dbus", level), "dbus/{level} 不应被过滤");
            assert!(
                !should_dedup("mqtt", level),
                "log_entry! 写的 mqtt 模块不应被过滤: {level}"
            );
        }
    }

    #[test]
    fn modules_lists_distinct_module_names() {
        let target = "test_modules_lists_distinct_module_names";
        super::info(target, "a");
        super::info(target, "b");

        let listed = super::modules();
        assert!(listed.contains(&target.to_string()));
        // 去重：同一模块只出现一次
        assert_eq!(
            listed.iter().filter(|m| *m == target).count(),
            1
        );
        // 已排序
        let mut sorted = listed.clone();
        sorted.sort();
        assert_eq!(listed, sorted);
    }
}