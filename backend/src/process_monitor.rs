use crate::models::{MemoryProcess, MemoryProcessesResponse};
use chrono::Utc;
use std::fs;
use std::path::Path;

const TOP_PROCESS_COUNT: usize = 10;
const MAX_COMMAND_LENGTH: usize = 512;
const PAGE_SIZE_BYTES: u64 = 4096;

pub fn read_top_memory_processes() -> Result<MemoryProcessesResponse, String> {
    let total_memory_bytes = read_total_memory_bytes()?;
    let proc_root = Path::new("/proc");
    let mut processes = Vec::new();
    let mut total_processes = 0usize;

    for entry in fs::read_dir(proc_root).map_err(|e| format!("Failed to read /proc: {}", e))? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let pid_text = file_name.to_string_lossy();
        if pid_text.is_empty() || !pid_text.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let pid = match pid_text.parse::<u32>() {
            Ok(pid) if pid > 0 => pid,
            _ => continue,
        };

        if let Some(process) = read_process(entry.path(), pid, total_memory_bytes) {
            total_processes = total_processes.saturating_add(1);
            processes.push(process);
        }
    }

    processes.sort_unstable_by(|left, right| {
        right
            .rss_bytes
            .cmp(&left.rss_bytes)
            .then_with(|| left.pid.cmp(&right.pid))
    });
    processes.truncate(TOP_PROCESS_COUNT);

    Ok(MemoryProcessesResponse {
        sampled_at: Utc::now().to_rfc3339(),
        total_processes,
        total_memory_bytes,
        processes,
    })
}

fn read_total_memory_bytes() -> Result<u64, String> {
    let content = fs::read_to_string("/proc/meminfo")
        .map_err(|e| format!("Failed to read /proc/meminfo: {}", e))?;
    content
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()? != "MemTotal:" {
                return None;
            }
            let kilobytes = fields.next()?.parse::<u64>().ok()?;
            Some(kilobytes.saturating_mul(1024))
        })
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| "MemTotal is missing or invalid".to_string())
}

fn read_process(path: std::path::PathBuf, pid: u32, total_memory_bytes: u64) -> Option<MemoryProcess> {
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_dir() {
        return None;
    }

    let status = fs::read_to_string(path.join("status")).ok()?;
    let fields = parse_status(&status)?;
    let name = fields.name;
    let rss_bytes = fields.rss_pages.saturating_mul(PAGE_SIZE_BYTES);
    let virtual_bytes = fields.virtual_pages.saturating_mul(PAGE_SIZE_BYTES);
    let memory_percent = if total_memory_bytes > 0 {
        (rss_bytes as f64 / total_memory_bytes as f64) * 100.0
    } else {
        0.0
    };
    let command = read_command(&path, &name);

    Some(MemoryProcess {
        pid,
        name,
        command,
        rss_bytes,
        virtual_bytes,
        memory_percent,
        threads: fields.threads,
    })
}

struct ProcessFields {
    name: String,
    rss_pages: u64,
    virtual_pages: u64,
    threads: u64,
}

fn parse_status(content: &str) -> Option<ProcessFields> {
    let mut name = None;
    let mut rss_pages = None;
    let mut virtual_pages = None;
    let mut threads = None;

    for line in content.lines() {
        let (key, value) = line.split_once(':')?;
        let value = value.trim();
        match key {
            "Name" => name = Some(value.to_string()),
            "VmRSS" => rss_pages = parse_kilobytes(value).map(|bytes| bytes / PAGE_SIZE_BYTES),
            "VmSize" => virtual_pages = parse_kilobytes(value).map(|bytes| bytes / PAGE_SIZE_BYTES),
            "Threads" => threads = value.split_whitespace().next()?.parse::<u64>().ok(),
            _ => {}
        }
    }

    Some(ProcessFields {
        name: name.filter(|value| !value.is_empty())?,
        rss_pages: rss_pages?,
        virtual_pages: virtual_pages?,
        threads: threads.unwrap_or(0),
    })
}

fn parse_kilobytes(value: &str) -> Option<u64> {
    let mut fields = value.split_whitespace();
    let amount = fields.next()?.parse::<u64>().ok()?;
    let unit = fields.next().unwrap_or("kB");
    match unit {
        "kB" | "KB" | "kb" => Some(amount.saturating_mul(1024)),
        "B" => Some(amount),
        _ => None,
    }
}

fn read_command(path: &Path, name: &str) -> String {
    let command_path = path.join("cmdline");
    let command = fs::read(command_path)
        .ok()
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| {
            bytes
                .into_iter()
                .map(|byte| if byte == 0 { b' ' } else { byte })
                .collect::<Vec<u8>>()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| format!("[{}]", name));

    let mut truncated = command.chars().take(MAX_COMMAND_LENGTH).collect::<String>();
    if command.chars().count() > MAX_COMMAND_LENGTH {
        truncated.push_str("...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::{parse_status, read_command};
    use std::path::Path;

    #[test]
    fn parses_process_status_fields() {
        let fields = parse_status("Name:\ttest\nVmSize:\t2048 kB\nVmRSS:\t1024 kB\nThreads:\t3\n").unwrap();
        assert_eq!(fields.name, "test");
        assert_eq!(fields.rss_pages, 256);
        assert_eq!(fields.virtual_pages, 512);
        assert_eq!(fields.threads, 3);
    }

    #[test]
    fn command_falls_back_to_process_name() {
        assert_eq!(read_command(Path::new("/path/that/does/not/exist"), "test"), "[test]");
    }
}
