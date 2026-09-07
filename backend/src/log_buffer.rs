//! 内存日志缓冲模块
//!
//! 使用环形缓冲在进程内存中保留最近的应用日志，供 Web 管理页面查看。
//! 不写磁盘、不引入额外 IO，避免对设备 flash 造成磨损，适合长期运行。
//!
//! 日志等级沿用 tracing 的语义，通过 `min_level` 支持页面筛选：
//! 查看“运行日志”传 `min_level = 0`（全部），查看“报错日志”传 `min_level = 2`
//! （仅 warn 与 error）。进程重启后缓冲清空。

use std::collections::VecDeque;
use std::sync::Mutex;

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

lazy_static::lazy_static! {
    static ref LOG_BUFFER: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
}

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
    chrono::Utc::now().to_rfc3339()
}

/// 内部写入逻辑。锁被毒化时尽量恢复，避免日志写入成为新的崩溃点。
fn push(level: &str, module: &str, message: String) {
    let entry = LogEntry {
        timestamp: now_rfc3339(),
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

/// 记录一条 error 日志
pub fn error(module: &str, message: impl Into<String>) {
    push("error", module, message.into());
}

/// 记录一条 debug 日志
pub fn debug(module: &str, message: impl Into<String>) {
    push("debug", module, message.into());
}

/// 读取日志快照，最新的记录在前。
///
/// # Arguments
/// * `min_level` - 最小日志等级数值（0=debug, 1=info, 2=warn, 3=error）
/// * `limit` - 最多返回条数
pub fn snapshot(min_level: u8, limit: usize) -> Vec<LogEntry> {
    let buffer = match LOG_BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let limit = limit.clamp(1, MAX_LOG_ENTRIES);

    buffer
        .iter()
        .rev()
        .filter(|entry| level_rank(&entry.level) >= min_level)
        .take(limit)
        .cloned()
        .collect()
}

/// 清空内存日志缓冲
pub fn clear() {
    let mut buffer = match LOG_BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    buffer.clear();
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
    (debug, $module:expr, $($arg:tt)*) => {
        $crate::log_buffer::debug($module, format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::{clear, snapshot};

    #[test]
    fn snapshot_supports_min_level_filtering() {
        clear();
        super::info("test", "info message");
        super::warn("test", "warn message");
        super::error("test", "error message");

        let all = snapshot(0, 100);
        assert_eq!(all.len(), 3);

        let warnings_up = snapshot(2, 100);
        assert_eq!(warnings_up.len(), 2);
        assert!(warnings_up.iter().all(|e| e.level == "warn" || e.level == "error"));
    }

    #[test]
    fn snapshot_returns_newest_first() {
        clear();
        super::info("test", "first");
        super::info("test", "second");

        let entries = snapshot(0, 100);
        assert_eq!(entries[0].message, "second");
        assert_eq!(entries[1].message, "first");
    }
}