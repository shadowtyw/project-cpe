//! 设备状态报告采集与格式化
//!
//! MQTT 遥控、短信遥控、Web 诊断三条路径都需要「设备当前状况」这份数据。
//! 之前 MQTT 与短信各写一份采集逻辑，结果两边的报告内容不一致：MQTT 有温度没 CPU，
//! 短信两者都没有，内存还都显示「已用百分比」（用户要的是可用百分比）。
//!
//! 本模块把采集统一为一次 [`DeviceReport::collect`]，再按投递渠道提供两种格式化：
//! * [`DeviceReport::format_summary`] — 富文本（带 emoji 与分隔线），用于 MQTT / Webhook 推送
//! * [`DeviceReport::format_sms`] — 精简纯文本，用于短信回复（受字数与终端可读性约束）
//!
//! 采集遵循两条硬约束：
//! 1. **任何单项失败都不影响整体**。设备离线、D-Bus 未就绪、平台没有 thermal zone
//!    都是正常运行状态，报告里对应行缺失即可，绝不 `unwrap`（release 下 `panic = "abort"`
//!    会直接杀掉整个进程）。
//! 2. **不长时间阻塞 async runtime**。`/proc`、`/sys`、`statvfs` 都是微秒级读取，直接同步调用；
//!    只有 `ip addr` 这类会 fork 子进程的操作走 `spawn_blocking`。

use crate::models::{
    AirplaneModeResponse, CpuLoadInfo, DeviceInfoResponse, DiskInfo, MemoryInfo,
    NetworkInfoResponse, NetworkInterfaceInfo, ServingCell, SystemInfo, ThermalZone,
};
use serde::Serialize;
use zbus::Connection;

/// 一次完整的设备状态快照
#[derive(Debug, Default, Serialize)]
pub struct DeviceReport {
    /// 采集时间（RFC 3339）
    pub timestamp: String,
    /// 后端程序版本（来自 build.rs 的 APP_VERSION）
    pub app_version: String,
    /// 构建 commit 短哈希
    pub git_commit: String,
    /// Modem 设备信息（IMEI / 厂商 / 型号 / 固件）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfoResponse>,
    /// 运营商与注册状态
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkInfoResponse>,
    /// 服务小区（制式 / cell id / TAC）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serving_cell: Option<ServingCell>,
    /// 原始信号强度（ofono 语义：0-100 百分比，或负数 dBm）
    pub signal_strength: Option<i32>,
    /// 数据连接是否已建立；`None` 表示查询失败
    pub data_connected: Option<bool>,
    /// 飞行模式状态
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airplane: Option<AirplaneModeResponse>,
    /// 内存详情
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryInfo>,
    /// CPU 占用率（%，两次 /proc/stat 采样差值）；采样失败时为 `None`
    pub cpu_usage_percent: Option<f64>,
    /// CPU 负载均值与核心数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_load: Option<CpuLoadInfo>,
    /// 各温度传感器读数
    pub thermal: Vec<ThermalZone>,
    /// 系统运行时长（秒）
    pub uptime_seconds: Option<u64>,
    /// 磁盘/分区使用情况
    pub disks: Vec<DiskInfo>,
    /// 网络接口与 IP 地址
    pub interfaces: Vec<NetworkInterfaceInfo>,
    /// uname 信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemInfo>,
}

impl DeviceReport {
    /// 采集一份完整的设备状态。
    ///
    /// 所有子项都独立容错：D-Bus 调用失败只让对应字段变成 `None`，
    /// 不会让整份报告失败，更不会 panic。
    pub async fn collect(dbus_conn: &Connection) -> Self {
        // D-Bus 调用并发发起。全局串行锁 with_serial 会让它们实际排队执行，
        // 但并发写法避免了「等完一个再发下一个」的额外调度往返。
        // 每个调用各自 .ok()：单项失败只让对应字段为 None，绝不让整份报告失败。
        let (device, network, serving_cell, data_connected, airplane, signal_strength) = tokio::join!(
            async { crate::dbus::get_device_info_data(dbus_conn).await.ok() },
            async { crate::dbus::get_network_info_data(dbus_conn).await.ok() },
            async { crate::dbus::get_serving_cell_info(dbus_conn).await.ok() },
            async { crate::dbus::get_data_connection_status(dbus_conn).await.ok() },
            async { crate::dbus::get_airplane_mode(dbus_conn).await.ok() },
            async {
                crate::dbus::get_signal_strength(dbus_conn)
                    .await
                    .ok()
                    .map(|s| s.strength)
            },
        );

        // CPU 占用率需要两次采样（间隔 2.0s），是整份报告里最慢的一步，先启动它
        let cpu_usage = crate::utils::sample_cpu_usage().await.ok();

        // 以下均为 /proc、/sys、statvfs 级别的同步读取，耗时微秒级
        let memory = crate::utils::read_memory_info().ok();
        let thermal = crate::utils::read_temperature_sensors();
        let uptime_seconds = crate::utils::read_uptime().ok().map(|(secs, _)| secs);
        let cpu_load = crate::utils::read_cpu_load_sync().ok();
        let disks = crate::utils::read_disk_info();
        let system = crate::utils::read_system_info().ok();

        // ip addr 会 fork 子进程，放到阻塞线程池，避免占用 async worker
        let interfaces = tokio::task::spawn_blocking(crate::utils::read_network_interfaces)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

        Self {
            timestamp: crate::utils::beijing_now_rfc3339(),
            app_version: env!("APP_VERSION").to_string(),
            git_commit: env!("GIT_COMMIT").to_string(),
            device,
            network,
            serving_cell,
            signal_strength,
            data_connected,
            airplane,
            memory,
            cpu_usage_percent: cpu_usage,
            cpu_load,
            thermal,
            uptime_seconds,
            disks,
            interfaces,
            system,
        }
    }

    /// 富文本摘要（MQTT 推送 / Webhook 通知）。
    ///
    /// `title` 作为首行，调用方按渠道传入不同标题。
    pub fn format_summary(&self, title: &str) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push(title.to_string());
        lines.push("━━━━━━━━━━━━━━━".to_string());

        self.push_device_lines(&mut lines);
        self.push_network_lines(&mut lines);
        self.push_resource_lines(&mut lines);
        self.push_storage_lines(&mut lines);
        self.push_runtime_lines(&mut lines);

        lines.push(format!("🕐 {}", self.timestamp));
        lines.join("\n")
    }

    /// 精简纯文本（短信回复）。
    ///
    /// 短信按 70 字/条计费分段，这里刻意去掉 emoji、分隔线与厂商/型号等
    /// 用户已知的信息，只保留排障时真正会看的字段。
    pub fn format_sms(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push("[UDX710] 设备状态".to_string());

        // 网络：运营商 + 注册状态 + 制式
        if let Some(net) = &self.network {
            let mut parts: Vec<String> = Vec::new();
            if !net.operator_name.is_empty() {
                parts.push(net.operator_name.clone());
            }
            parts.push(translate_registration(&net.registration_status));
            if !net.technology_preference.is_empty() {
                parts.push(net.technology_preference.clone());
            }
            lines.push(format!("网络: {}", parts.join(" ")));
        } else {
            lines.push("网络: 获取失败".to_string());
        }

        // 信号
        lines.push(format!("信号: {}", self.signal_text_compact()));

        // 上网
        lines.push(match self.data_connected {
            Some(true) => format!(
                "上网: 已连接{}",
                self.primary_ipv4()
                    .map(|ip| format!(" {ip}"))
                    .unwrap_or_default()
            ),
            Some(false) => "上网: 未连接".to_string(),
            None => "上网: 获取失败".to_string(),
        });

        // 内存：可用百分比为主指标
        if let Some(mem) = &self.memory {
            if mem.total_bytes > 0 {
                lines.push(format!(
                    "内存: 可用{:.0}% ({}MB/{}MB)",
                    mem.available_percent,
                    mem.available_bytes / 1024 / 1024,
                    mem.total_bytes / 1024 / 1024
                ));
            }
        }

        // CPU 占用率
        if let Some(usage) = self.cpu_usage_text_compact() {
            lines.push(format!("CPU: {usage}"));
        }

        // 温度：只报最高值，短信里逐个传感器列出太长
        if let Some(t) = self.hottest_sensor() {
            lines.push(format!("温度: {:.1}°C", t.1));
        }

        // 运行时长
        if let Some(secs) = self.uptime_seconds {
            lines.push(format!("运行: {}", crate::utils::format_uptime(secs)));
        }

        lines.push(format!("版本: v{}", self.app_version));
        lines.join("\n")
    }

    // ── 富文本各段 ──────────────────────────────────────────

    fn push_device_lines(&self, lines: &mut Vec<String>) {
        if let Some(dev) = &self.device {
            let mut model = Vec::new();
            if !dev.manufacturer.is_empty() {
                model.push(dev.manufacturer.clone());
            }
            if !dev.model.is_empty() {
                model.push(dev.model.clone());
            }
            let model_str = if model.is_empty() {
                "未知型号".to_string()
            } else {
                model.join(" ")
            };
            lines.push(format!("📱 设备: {model_str}"));

            if !dev.imei.is_empty() {
                lines.push(format!("   IMEI: {}", dev.imei));
            }
            if let Some(rev) = dev.revision.as_ref().filter(|r| !r.is_empty()) {
                lines.push(format!("   模组固件: {rev}"));
            }
            // online=false 意味着射频未开（飞行模式），是最常见的「设备失联」原因，必须显式提示
            if !dev.online || !dev.powered {
                let reason = if !dev.powered {
                    "已关机"
                } else if self.airplane.as_ref().is_some_and(|a| a.enabled) {
                    "飞行模式"
                } else {
                    "射频关闭"
                };
                lines.push(format!("   ⚠️ Modem {reason}"));
            }
        }

        let commit = if self.git_commit.len() > 7 {
            &self.git_commit[..7]
        } else {
            self.git_commit.as_str()
        };
        if commit.is_empty() || commit == "unknown" {
            lines.push(format!("🔧 程序: v{}", self.app_version));
        } else {
            lines.push(format!("🔧 程序: v{} ({commit})", self.app_version));
        }

        if let Some(sys) = &self.system {
            if !sys.release.is_empty() {
                lines.push(format!("   内核: {} {}", sys.release, sys.machine));
            }
        }
    }

    fn push_network_lines(&self, lines: &mut Vec<String>) {
        if let Some(net) = &self.network {
            let mut parts: Vec<String> = Vec::new();
            if !net.operator_name.is_empty() {
                parts.push(net.operator_name.clone());
            }
            parts.push(translate_registration(&net.registration_status));
            if !net.technology_preference.is_empty() {
                parts.push(net.technology_preference.clone());
            }
            lines.push(format!("📡 网络: {}", parts.join(" · ")));
        } else {
            lines.push("📡 网络: 获取失败".to_string());
        }

        lines.push(format!("   信号: {}", self.signal_text_rich()));

        if let Some(cell) = &self.serving_cell {
            if !cell.tech.is_empty() && cell.tech != "unknown" {
                lines.push(format!(
                    "   小区: {} CellID {} / TAC {}",
                    cell.tech.to_uppercase(),
                    cell.cell_id,
                    cell.tac
                ));
            }
        }

        let data_text = match self.data_connected {
            Some(true) => "已连接".to_string(),
            Some(false) => "未连接".to_string(),
            None => "获取失败".to_string(),
        };
        let ip_suffix = match self.data_connected {
            Some(true) => self
                .primary_ipv4()
                .map(|ip| format!(" · {ip}"))
                .unwrap_or_default(),
            _ => String::new(),
        };
        lines.push(format!("🌐 上网: {data_text}{ip_suffix}"));

        if let Some(ap) = &self.airplane {
            if ap.enabled {
                lines.push("✈️ 飞行模式: 已开启".to_string());
            }
        }
    }

    fn push_resource_lines(&self, lines: &mut Vec<String>) {
        // 内存：以「可用百分比」为主指标。
        // MemAvailable 才是内核认为可分配给新进程的内存，比 total-free 更贴近实际余量，
        // 用户排障时关心的是「还剩多少」而不是「用了多少」。
        if let Some(mem) = &self.memory {
            if mem.total_bytes > 0 {
                lines.push(format!(
                    "💾 内存: 可用 {:.0}% (可用 {}MB / 总计 {}MB)",
                    mem.available_percent,
                    mem.available_bytes / 1024 / 1024,
                    mem.total_bytes / 1024 / 1024
                ));
                // buff/cache 可被内核回收，单独列出避免「可用太少」的误判
                if mem.buff_cache_bytes > 0 {
                    lines.push(format!(
                        "   缓存: {}MB (可回收)",
                        mem.buff_cache_bytes / 1024 / 1024
                    ));
                }
                if mem.available_estimated {
                    lines.push("   ⚠️ 可用值由兼容公式估算（内核未提供 MemAvailable）".to_string());
                }
            }
        }

        // CPU 占用率：优先用采样差值，失败时回落到 load 百分比
        match self.cpu_usage_percent {
            Some(usage) => {
                let load_suffix = self
                    .cpu_load
                    .as_ref()
                    .map(|l| {
                        format!(
                            " · 负载 {:.2}/{:.2}/{:.2} ({}核)",
                            l.load_1min, l.load_5min, l.load_15min, l.core_count
                        )
                    })
                    .unwrap_or_default();
                lines.push(format!("⚙️ CPU: 占用 {:.1}%{load_suffix}", usage));
            }
            None => match &self.cpu_load {
                Some(l) => lines.push(format!(
                    "⚙️ CPU: 负载 {:.2}/{:.2}/{:.2} ({}核，约 {:.0}%)",
                    l.load_1min, l.load_5min, l.load_15min, l.core_count, l.load_percent
                )),
                None => lines.push("⚙️ CPU: 获取失败".to_string()),
            },
        }

        // 温度：全部传感器逐个列出，设备过热是最常见的宕机诱因
        if self.thermal.is_empty() {
            lines.push("🌡️ 温度: 无可用传感器".to_string());
        } else {
            let mut rendered: Vec<String> = Vec::new();
            for tz in &self.thermal {
                let emoji = temperature_emoji(tz.temperature);
                // 传感器类型为空时退回用 zone 名，避免出现「❄️ : 48.2°C」这种断裂输出
                let name = if tz.sensor_type.is_empty() {
                    tz.zone.as_str()
                } else {
                    tz.sensor_type.as_str()
                };
                rendered.push(format!("{emoji} {name} {:.1}°C", tz.temperature));
            }
            lines.push(format!("🌡️ 温度: {}", rendered.join(" · ")));
        }
    }

    fn push_storage_lines(&self, lines: &mut Vec<String>) {
        // 只报根分区与数据分区：嵌入式设备上 /proc、/sys、/dev 等伪文件系统没有容量概念
        let notable: Vec<&DiskInfo> = self
            .disks
            .iter()
            .filter(|d| d.total_bytes > 0)
            .filter(|d| matches!(d.mount_point.as_str(), "/" | "/data" | "/userdata" | "/mnt" | "/overlay"))
            .collect();

        if notable.is_empty() {
            return;
        }

        let rendered: Vec<String> = notable
            .iter()
            .map(|d| {
                format!(
                    "{} 已用 {:.0}% (剩余 {})",
                    d.mount_point,
                    d.used_percent,
                    format_bytes(d.available_bytes)
                )
            })
            .collect();
        lines.push(format!("💽 存储: {}", rendered.join(" · ")));

        // 根分区写满会让 SQLite 提交失败、日志丢失，属于必须立刻知道的状态
        if let Some(root) = notable.iter().find(|d| d.mount_point == "/") {
            if root.used_percent >= 90.0 {
                lines.push(format!("   ⚠️ 根分区仅剩 {}", format_bytes(root.available_bytes)));
            }
        }
    }

    fn push_runtime_lines(&self, lines: &mut Vec<String>) {
        if let Some(secs) = self.uptime_seconds {
            lines.push(format!("⏱️ 已运行: {}", crate::utils::format_uptime(secs)));
        }
    }

    // ── 单字段渲染辅助 ──────────────────────────────────────

    /// 信号强度富文本（带进度条与档位描述）
    fn signal_text_rich(&self) -> String {
        // ofono 的 Strength 是 0-100 的百分比，但部分模组返回负数 dBm，两种都要能显示
        match self.signal_value() {
            None => "未知".to_string(),
            Some(v) if v < 0 => format!("{v} dBm"),
            Some(v) => {
                let (bars, label) = signal_bars(v);
                format!("{v}% {bars} ({label})")
            }
        }
    }

    /// 信号强度精简文本
    fn signal_text_compact(&self) -> String {
        match self.signal_value() {
            None => "未知".to_string(),
            Some(v) if v < 0 => format!("{v}dBm"),
            Some(v) => format!("{v}%"),
        }
    }

    /// 原始信号值。优先取 `GetSignalStrength` 的原始值（可能是负数 dBm），
    /// 缺失时回落到网络信息里的 0-100 百分比。
    fn signal_value(&self) -> Option<i64> {
        if let Some(v) = self.signal_strength {
            return Some(v as i64);
        }
        self.network
            .as_ref()
            .filter(|net| net.signal_strength > 0)
            .map(|net| net.signal_strength as i64)
    }

    fn cpu_usage_text_compact(&self) -> Option<String> {
        self.cpu_usage_percent
            .map(|u| format!("{:.1}%", u))
            .or_else(|| {
                self.cpu_load
                    .as_ref()
                    .map(|l| format!("负载 {:.2}", l.load_1min))
            })
    }

    /// 温度最高的传感器，返回 (名称, 摄氏度)
    fn hottest_sensor(&self) -> Option<(&str, f64)> {
        self.thermal
            .iter()
            .map(|tz| {
                let name = if tz.sensor_type.is_empty() {
                    tz.zone.as_str()
                } else {
                    tz.sensor_type.as_str()
                };
                (name, tz.temperature)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// 主 IPv4 地址：优先 up 状态接口上的公网地址，其次内网，最后链路本地。
    ///
    /// 蜂窝拨号拿到的通常是运营商内网地址（10.x / 100.64.x），排在公网之后正好符合
    /// 「有公网 IP 就报公网」的排障直觉。
    fn primary_ipv4(&self) -> Option<String> {
        let candidates: Vec<(u8, &str)> = self
            .interfaces
            .iter()
            .filter(|iface| iface.status.eq_ignore_ascii_case("up"))
            .flat_map(|iface| {
                iface
                    .ip_addresses
                    .iter()
                    .filter(|ip| ip.ip_type == "ipv4")
                    .filter(|ip| ip.scope != "loopback")
                    .map(move |ip| (scope_rank(&ip.scope), ip.address.as_str()))
            })
            .collect();

        candidates
            .into_iter()
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, addr)| addr.to_string())
    }
}

// ── 纯函数辅助 ──────────────────────────────────────────────

/// IP 地址范围排序权重，越小越优先
fn scope_rank(scope: &str) -> u8 {
    match scope {
        "public" => 0,
        "private" => 1,
        "link-local" => 2,
        _ => 3,
    }
}

/// 信号百分比 → (进度条, 档位描述)
fn signal_bars(percent: i64) -> (&'static str, &'static str) {
    if percent >= 80 {
        ("█████", "极好")
    } else if percent >= 60 {
        ("████", "良好")
    } else if percent >= 40 {
        ("███", "一般")
    } else if percent >= 20 {
        ("██", "较弱")
    } else {
        ("█", "弱")
    }
}

/// 温度 → emoji，阈值与前端保持一致
fn temperature_emoji(celsius: f64) -> &'static str {
    if celsius >= 75.0 {
        "🔥"
    } else if celsius >= 55.0 {
        "🌡️"
    } else {
        "❄️"
    }
}

/// ofono 注册状态英文枚举 → 中文
fn translate_registration(status: &str) -> String {
    let zh = match status {
        "registered" => "已注册",
        "roaming" => "漫游中",
        "searching" => "搜网中",
        "denied" => "被拒绝",
        "unknown" | "" => "未知",
        other => other,
    };
    zh.to_string()
}

/// 字节数 → 人类可读（GB / MB / KB）
fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0}MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{IpAddress, ThermalZone};

    fn report_with_memory() -> DeviceReport {
        DeviceReport {
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            app_version: "9.9.9".to_string(),
            git_commit: "abcdef1234567".to_string(),
            memory: Some(MemoryInfo {
                total_bytes: 256 * 1024 * 1024,
                available_bytes: 128 * 1024 * 1024,
                available_percent: 50.0,
                used_percent: 50.0,
                buff_cache_bytes: 32 * 1024 * 1024,
                ..MemoryInfo::default()
            }),
            cpu_usage_percent: Some(12.34),
            thermal: vec![
                ThermalZone {
                    zone: "thermal_zone0".to_string(),
                    sensor_type: "soc".to_string(),
                    temperature: 48.2,
                },
                ThermalZone {
                    zone: "thermal_zone1".to_string(),
                    sensor_type: "wlan".to_string(),
                    temperature: 80.0,
                },
            ],
            uptime_seconds: Some(90_061),
            ..DeviceReport::default()
        }
    }

    #[test]
    fn summary_shows_available_memory_percent() {
        let report = report_with_memory();
        let text = report.format_summary("📊 设备状态报告");
        // 主指标是「可用」百分比，而不是旧版的「已用」
        assert!(text.contains("可用 50%"), "实际输出:\n{text}");
        assert!(!text.contains("已用 50% (已用"), "不应再出现旧版已用百分比格式:\n{text}");
    }

    #[test]
    fn summary_shows_cpu_and_temperature() {
        let text = report_with_memory().format_summary("📊 设备状态报告");
        assert!(text.contains("占用 12.3%"), "CPU 占用率缺失:\n{text}");
        // 两个传感器都要出现，且过热的用 🔥 标记
        assert!(text.contains("soc 48.2°C"), "温度传感器缺失:\n{text}");
        assert!(text.contains("🔥 wlan 80.0°C"), "高温标记缺失:\n{text}");
    }

    #[test]
    fn summary_formats_uptime_via_utils() {
        let text = report_with_memory().format_summary("📊 设备状态报告");
        // 90061s = 1天 1小时 1分钟 1秒
        assert!(text.contains("1天"), "运行时长缺失:\n{text}");
        assert!(text.contains("1小时"), "运行时长缺失:\n{text}");
    }

    #[test]
    fn summary_truncates_commit_to_seven_chars() {
        let text = report_with_memory().format_summary("📊 设备状态报告");
        assert!(text.contains("v9.9.9 (abcdef1)"), "版本号/commit 格式不符:\n{text}");
        assert!(!text.contains("abcdef1234567"), "commit 未截断:\n{text}");
    }

    #[test]
    fn summary_survives_all_fields_missing() {
        // release 下 panic = "abort"，任何 unwrap 都会杀掉整个进程；
        // 空报告必须能正常渲染出标题与时间戳。
        let empty = DeviceReport {
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            app_version: "1.0.0".to_string(),
            git_commit: "unknown".to_string(),
            ..DeviceReport::default()
        };
        let text = empty.format_summary("📊 设备状态报告");
        assert!(text.contains("📊 设备状态报告"));
        assert!(text.contains("v1.0.0"));
        // git_commit 为 unknown 时不应输出空括号
        assert!(!text.contains("()"), "unknown commit 产生了空括号:\n{text}");
        assert!(text.contains("温度: 无可用传感器"), "空传感器应有占位提示:\n{text}");
    }

    #[test]
    fn sms_report_is_compact_and_has_no_emoji() {
        let text = report_with_memory().format_sms();
        assert!(text.starts_with("[UDX710] 设备状态"));
        assert!(text.contains("内存: 可用50% (128MB/256MB)"), "内存行不符:\n{text}");
        assert!(text.contains("CPU: 12.3%"), "CPU 行不符:\n{text}");
        // 短信只报最高温度（80.0），不逐个列出
        assert!(text.contains("温度: 80.0°C"), "温度行不符:\n{text}");
        assert!(!text.contains("48.2"), "短信不应列出全部传感器:\n{text}");
        // 行数应明显少于富文本
        assert!(text.lines().count() < report_with_memory().format_summary("x").lines().count());
    }

    #[test]
    fn hottest_sensor_picks_maximum() {
        let report = report_with_memory();
        let hottest = report.hottest_sensor();
        assert_eq!(hottest.map(|h| h.1), Some(80.0));
        assert_eq!(hottest.map(|h| h.0), Some("wlan"));
    }

    #[test]
    fn hottest_sensor_falls_back_to_zone_name() {
        let report = DeviceReport {
            thermal: vec![ThermalZone {
                zone: "thermal_zone3".to_string(),
                sensor_type: String::new(),
                temperature: 40.0,
            }],
            ..DeviceReport::default()
        };
        assert_eq!(report.hottest_sensor().map(|h| h.0), Some("thermal_zone3"));
    }

    #[test]
    fn primary_ipv4_prefers_public_over_private_and_link_local() {
        let iface = |name: &str, scope: &str, addr: &str| NetworkInterfaceInfo {
            name: name.to_string(),
            status: "up".to_string(),
            mac_address: None,
            mtu: 1500,
            ip_addresses: vec![IpAddress {
                address: addr.to_string(),
                prefix_len: 24,
                ip_type: "ipv4".to_string(),
                scope: scope.to_string(),
            }],
            rx_bytes: 0,
            tx_bytes: 0,
            rx_packets: 0,
            tx_packets: 0,
            rx_errors: 0,
            tx_errors: 0,
        };

        let report = DeviceReport {
            interfaces: vec![
                iface("lo", "loopback", "127.0.0.1"),
                iface("wlan0", "link-local", "169.254.1.5"),
                iface("usb0", "private", "192.168.1.50"),
            ],
            ..DeviceReport::default()
        };
        // 回环被排除，私网优先于链路本地
        assert_eq!(report.primary_ipv4().as_deref(), Some("192.168.1.50"));

        let with_public = DeviceReport {
            interfaces: vec![
                iface("usb0", "private", "192.168.1.50"),
                iface("wwan0", "public", "1.2.3.4"),
            ],
            ..DeviceReport::default()
        };
        assert_eq!(with_public.primary_ipv4().as_deref(), Some("1.2.3.4"));

        let down_only = DeviceReport {
            interfaces: vec![NetworkInterfaceInfo {
                status: "down".to_string(),
                ..iface("usb0", "private", "192.168.1.50")
            }],
            ..DeviceReport::default()
        };
        assert_eq!(down_only.primary_ipv4(), None);
    }

    #[test]
    fn signal_bars_thresholds() {
        assert_eq!(signal_bars(95).1, "极好");
        assert_eq!(signal_bars(80).1, "极好");
        assert_eq!(signal_bars(79).1, "良好");
        assert_eq!(signal_bars(60).1, "良好");
        assert_eq!(signal_bars(40).1, "一般");
        assert_eq!(signal_bars(20).1, "较弱");
        assert_eq!(signal_bars(0).1, "弱");
    }

    #[test]
    fn temperature_emoji_thresholds() {
        assert_eq!(temperature_emoji(80.0), "🔥");
        assert_eq!(temperature_emoji(75.0), "🔥");
        assert_eq!(temperature_emoji(74.9), "🌡️");
        assert_eq!(temperature_emoji(55.0), "🌡️");
        assert_eq!(temperature_emoji(54.9), "❄️");
    }

    #[test]
    fn translate_registration_covers_known_states() {
        assert_eq!(translate_registration("registered"), "已注册");
        assert_eq!(translate_registration("roaming"), "漫游中");
        assert_eq!(translate_registration("searching"), "搜网中");
        assert_eq!(translate_registration("denied"), "被拒绝");
        assert_eq!(translate_registration("unknown"), "未知");
        assert_eq!(translate_registration(""), "未知");
        // 未知枚举原样返回，避免把新状态吞成 "未知"
        assert_eq!(translate_registration("emergency"), "emergency");
    }

    #[test]
    fn format_bytes_uses_appropriate_unit() {
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0GB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5GB");
        assert_eq!(format_bytes(512 * 1024 * 1024), "512MB");
        assert_eq!(format_bytes(512 * 1024), "512KB");
        assert_eq!(format_bytes(0), "0KB");
    }

    #[test]
    fn scope_rank_orders_public_first() {
        assert!(scope_rank("public") < scope_rank("private"));
        assert!(scope_rank("private") < scope_rank("link-local"));
        assert!(scope_rank("link-local") < scope_rank("weird"));
    }

    #[test]
    fn storage_section_skips_pseudo_filesystems() {
        let report = DeviceReport {
            disks: vec![
                DiskInfo {
                    mount_point: "/proc".to_string(),
                    fs_type: "proc".to_string(),
                    total_bytes: 0,
                    used_bytes: 0,
                    available_bytes: 0,
                    used_percent: 0.0,
                },
                DiskInfo {
                    mount_point: "/".to_string(),
                    fs_type: "ext4".to_string(),
                    total_bytes: 2 * 1024 * 1024 * 1024,
                    used_bytes: 1024 * 1024 * 1024,
                    available_bytes: 1024 * 1024 * 1024,
                    used_percent: 50.0,
                },
            ],
            ..DeviceReport::default()
        };
        let text = report.format_summary("t");
        assert!(text.contains("/ 已用 50%"), "根分区应出现:\n{text}");
        assert!(!text.contains("/proc"), "伪文件系统不应出现:\n{text}");
    }

    #[test]
    fn storage_section_warns_when_root_nearly_full() {
        let report = DeviceReport {
            disks: vec![DiskInfo {
                mount_point: "/".to_string(),
                fs_type: "ext4".to_string(),
                total_bytes: 1024 * 1024 * 1024,
                used_bytes: 1000 * 1024 * 1024,
                available_bytes: 24 * 1024 * 1024,
                used_percent: 97.7,
            }],
            ..DeviceReport::default()
        };
        let text = report.format_summary("t");
        assert!(text.contains("根分区仅剩 24MB"), "满盘告警缺失:\n{text}");
    }
}
