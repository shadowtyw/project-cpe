/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-09 17:34:01
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:45:58
 * @FilePath: /udx710-backend/backend/src/config.rs
 * @Description: 
 * 
 * Copyright (c) 2025 by 1orz, All Rights Reserved. 
 */
//! 配置管理模块
//!
//! 使用 JSON 文件存储用户配置，支持热更新

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

const DEFAULT_LOADER_SCRIPT: &str = r#"#!/bin/sh
/home/root/ttyd/start.sh &
/home/root/udx710 -p 80 &
"#;
const LOADER_SCRIPT_PATH: &str = "/home/root/loader.sh";
const INIT_SCRIPT_PATH: &str = "/home/root/init.sh";
const INIT_SCRIPT_LOADER_COMMAND: &str = "sh /home/root/init.sh &";

/// Webhook 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_true")]
    pub forward_sms: bool,
    #[serde(default = "default_true")]
    pub forward_calls: bool,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub secret: String,  // 可选的签名密钥
    #[serde(default = "default_sms_template")]
    pub sms_template: String,  // 短信 payload 模板
    #[serde(default = "default_call_template")]
    pub call_template: String,  // 通话 payload 模板
}

/// 远程遥控推送配置（独立于 WebhookConfig）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteControlPushConfig {
    /// 是否启用远程遥控推送
    #[serde(default)]
    pub enabled: bool,
    /// Webhook URL
    #[serde(default)]
    pub webhook_url: String,
    /// 自定义请求头
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// 签名密钥
    #[serde(default)]
    pub secret: String,
    /// 推送 payload 模板，支持 {{变量}} 替换
    #[serde(default = "default_remote_control_push_template")]
    pub template: String,
    /// 是否推送短信遥控事件
    #[serde(default = "default_true")]
    pub forward_sms_control: bool,
    /// 是否推送通话遥控事件
    #[serde(default = "default_true")]
    pub forward_call_control: bool,
    /// 是否推送 MQTT 遥控事件
    #[serde(default = "default_true")]
    pub forward_mqtt_control: bool,
}

fn default_remote_control_push_template() -> String {
    r#"{
  "msgtype": "text",
  "text": {
    "content": "🔔 MQTT 远程控制通知\n━━━━━━━━━━━━━━━\n📡 事件: {{data_event}}\n⚙️ 指令: {{data_command}}\n💬 内容: {{data_message}}\n🕐 时间: {{timestamp}}"
  }
}"#
    .to_string()
}

/// 默认短信模板 (飞书机器人格式)
fn default_sms_template() -> String {
    r#"{
  "msg_type": "text",
  "content": {
    "text": "📱 短信通知\n发送方: {{phone_number}}\n内容: {{content}}\n时间: {{timestamp}}"
  }
}"#.to_string()
}

/// 默认通话模板 (飞书机器人格式)
fn default_call_template() -> String {
    r#"{
  "msg_type": "text",
  "content": {
    "text": "📞 来电通知\n号码: {{phone_number}}\n类型: {{direction}}\n时间: {{start_time}}\n时长: {{duration}}秒\n已接听: {{answered}}"
  }
}"#.to_string()
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            forward_sms: true,
            forward_calls: true,
            headers: HashMap::new(),
            secret: String::new(),
            sms_template: default_sms_template(),
            call_template: default_call_template(),
        }
    }
}

impl Default for RemoteControlPushConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            webhook_url: String::new(),
            headers: HashMap::new(),
            secret: String::new(),
            template: default_remote_control_push_template(),
            forward_sms_control: true,
            forward_call_control: true,
            forward_mqtt_control: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// 短信推送服务提供商
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmsPushProvider {
    Pushplus,
    Serverchan,
    Pushdeer,
    Bark,
    Ntfy,
}

impl Default for SmsPushProvider {
    fn default() -> Self {
        Self::Pushplus
    }
}

/// 短信推送配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsPushConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: SmsPushProvider,
    #[serde(default)]
    pub credential: String,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default = "default_sms_push_title_template")]
    pub title_template: String,
    #[serde(default = "default_sms_push_body_template")]
    pub body_template: String,
}

fn default_sms_push_title_template() -> String {
    "短信通知 · {{phone_number}}".to_string()
}

fn default_sms_push_body_template() -> String {
    "时间: {{timestamp}}\n号码: {{phone_number}}\n状态: {{status}}\n\n{{content}}".to_string()
}

impl Default for SmsPushConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: SmsPushProvider::Pushplus,
            credential: String::new(),
            server_url: String::new(),
            topic: String::new(),
            title_template: default_sms_push_title_template(),
            body_template: default_sms_push_body_template(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshConfig {
    #[serde(default = "default_refresh_interval_ms")]
    pub interval_ms: u64,
}

fn default_refresh_interval_ms() -> u64 {
    5_000
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            interval_ms: default_refresh_interval_ms(),
        }
    }
}

impl RefreshConfig {
    pub fn sanitize(mut self) -> Self {
        self.interval_ms = sanitize_refresh_interval_ms(self.interval_ms);
        self
    }

    pub fn heartbeat_timeout_ms(&self) -> u64 {
        let base = self.interval_ms.max(1_000);
        if self.interval_ms == 0 {
            30_000
        } else {
            (base.saturating_mul(4)).clamp(15_000, 120_000)
        }
    }

    pub fn active_watchdog_interval_ms(&self) -> u64 {
        if self.interval_ms == 0 {
            15_000
        } else {
            self.interval_ms.max(5_000)
        }
    }

    pub fn idle_watchdog_interval_ms(&self) -> u64 {
        self.active_watchdog_interval_ms()
            .saturating_mul(6)
            .max(60_000)
    }
}

fn sanitize_refresh_interval_ms(interval_ms: u64) -> u64 {
    match interval_ms {
        0 => 0,
        1..=999 => 1_000,
        value => value.min(60_000),
    }
}

/// 自动重启配置。所有策略默认关闭，避免升级后改变既有设备行为。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartConfig {
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default = "default_schedule_interval_days")]
    pub schedule_interval_days: u32,
    #[serde(default)]
    pub low_memory_enabled: bool,
    #[serde(default = "default_low_memory_threshold_percent")]
    pub low_memory_threshold_percent: u8,
}

fn default_schedule_interval_days() -> u32 {
    7
}

fn default_low_memory_threshold_percent() -> u8 {
    10
}

impl Default for RestartConfig {
    fn default() -> Self {
        Self {
            schedule_enabled: false,
            schedule_interval_days: default_schedule_interval_days(),
            low_memory_enabled: false,
            low_memory_threshold_percent: default_low_memory_threshold_percent(),
        }
    }
}

impl RestartConfig {
    pub fn sanitize(mut self) -> Self {
        self.schedule_interval_days = self.schedule_interval_days.clamp(1, 365);
        self.low_memory_threshold_percent = self.low_memory_threshold_percent.clamp(5, 50);
        self
    }
}

/// 断网自愈配置。默认关闭，避免升级后改变既有设备行为。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetHealthConfig {
    /// 是否启用基于外网探活的断网自愈
    #[serde(default)]
    pub enabled: bool,
    /// 探测周期（秒）
    #[serde(default = "default_net_health_interval_secs")]
    pub interval_secs: u64,
    /// 单个探测目标 ping 超时（秒）
    #[serde(default = "default_net_health_ping_timeout_secs")]
    pub ping_timeout_secs: u64,
    /// Level 1：连续失败多少次后重置数据连接
    #[serde(default = "default_net_health_l1_failures")]
    pub l1_failures: u32,
    /// Level 2：累计失败多少次后飞行模式复位基带
    #[serde(default = "default_net_health_l2_failures")]
    pub l2_failures: u32,
    /// Level 3：累计失败多少次后系统重启
    #[serde(default = "default_net_health_l3_failures")]
    pub l3_failures: u32,
    /// 每级动作执行后的静默期（秒），给基带重连留时间，避免动作叠加
    #[serde(default = "default_net_health_cooldown_secs")]
    pub cooldown_secs: u64,
}

fn default_net_health_interval_secs() -> u64 {
    60
}

fn default_net_health_ping_timeout_secs() -> u64 {
    2
}

fn default_net_health_l1_failures() -> u32 {
    3
}

fn default_net_health_l2_failures() -> u32 {
    6
}

fn default_net_health_l3_failures() -> u32 {
    10
}

fn default_net_health_cooldown_secs() -> u64 {
    30
}

impl Default for NetHealthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_net_health_interval_secs(),
            ping_timeout_secs: default_net_health_ping_timeout_secs(),
            l1_failures: default_net_health_l1_failures(),
            l2_failures: default_net_health_l2_failures(),
            l3_failures: default_net_health_l3_failures(),
            cooldown_secs: default_net_health_cooldown_secs(),
        }
    }
}

impl NetHealthConfig {
    pub fn sanitize(mut self) -> Self {
        self.interval_secs = self.interval_secs.clamp(10, 3600);
        self.ping_timeout_secs = self.ping_timeout_secs.clamp(1, 10);
        self.l1_failures = self.l1_failures.clamp(1, 100);
        self.l2_failures = self.l2_failures.clamp(self.l1_failures, 100);
        self.l3_failures = self.l3_failures.clamp(self.l2_failures, 100);
        self.cooldown_secs = self.cooldown_secs.clamp(0, 600);
        self
    }
}

/// 定时计划执行的命令
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAction {
    // 飞行模式开/关（开=关闭射频省电）
    AirplaneOn,
    AirplaneOff,
    // 数据连接开/关
    DataOn,
    DataOff,
    // 射频模式
    RadioLte,
    RadioNr,
    RadioAuto,
    // 关闭射频（比飞行模式更彻底，Modem 下电）
    RadioOff,
    // 重启
    #[default]
    Reboot,
}

/// 单条定时计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// 触发时间（本地 24 小时制 HH:MM）
    ///
    /// 缺失时取空串，`ScheduleConfig::sanitize()` 会把它连同整条计划项一起丢弃
    /// （`valid_schedule_time` 校验不过），不会留下一条永远不触发的僵尸计划。
    #[serde(default)]
    pub time: String,
    /// 周几触发（0=周日 … 6=周六），空表示每天
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// 要执行的命令
    #[serde(default)]
    pub action: ScheduleAction,
}

/// 定时计划总配置。默认关闭，避免升级后改变既有设备行为。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScheduleConfig {
    #[serde(default)]
    pub entries: Vec<ScheduleEntry>,
    /// 时钟校准误差容忍窗口（分钟）
    #[serde(default = "default_schedule_tolerance_min")]
    pub tolerance_min: u8,
}

fn default_schedule_tolerance_min() -> u8 {
    1
}

/// 通话遥控时长编码命令：通话持续 N 秒映射到指定动作。
/// 使用 ±2 秒容差匹配，例如 duration_secs=5 可匹配 3–7 秒的通话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallControlDurationCommand {
    /// 匹配的通话时长（秒）；同一通话时长只匹配一条命令，重复的取第一条。
    #[serde(default = "default_duration_secs")]
    pub duration_secs: u64,
    /// 匹配后执行的动作
    #[serde(default = "default_duration_command_action")]
    pub action: ScheduleAction,
    /// 前端展示标签（可为空，为空时显示动作名称）
    #[serde(default)]
    pub label: String,
}

/// 通话远程控制配置（时长编码方案）。
///
/// ## 交互流程
///
/// 1. 白名单号码来电 → 自动接听，开始计时。
/// 2. 用户保持通话 N 秒后挂断 → 根据 duration_commands 匹配命令（±2s 容差）。
/// 3. 通过 Webhook + 短信推送通知"检测到遥控命令，10 秒内再次来电确认执行"。
/// 4. 同号码 10 秒内再次来电 → 确认并执行动作。
/// 5. 10 秒内无二次来电 → 推送"命令已取消"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallControlConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 白名单号码。来电号码经规范化（去空格/连字符）后，只要以任一白名单号码结尾即匹配，
    /// 从而兼容来电显示带 "+86" 或国家码前缀的情况。
    #[serde(default)]
    pub numbers: Vec<String>,
    /// 时长→命令映射表。同一 duration_secs 只保留第一条，推荐 5、10、15、20 秒依次递增。
    #[serde(default)]
    pub duration_commands: Vec<CallControlDurationCommand>,
}

fn default_duration_secs() -> u64 {
    10
}

fn default_duration_command_action() -> ScheduleAction {
    ScheduleAction::Reboot
}

impl Default for CallControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            numbers: Vec::new(),
            duration_commands: Vec::new(),
        }
    }
}

/// 短信远程指令控制。复用通话遥控的白名单（CallControlConfig.numbers），
/// 仅增加独立的启用开关。当白名单号码发来 #REBOOT# / #RECONNECT# / #STATUS#
/// 等指令时，设备执行对应动作并通过短信回复结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsControlConfig {
    /// 是否启用短信远程指令控制
    #[serde(default)]
    pub enabled: bool,
}

impl Default for SmsControlConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

impl SmsControlConfig {
    pub fn sanitize(self) -> Self {
        self
    }
}

/// 流量用量预警配置。默认关闭。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrafficAlertConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 单日流量阈值（字节），超过则触发通知
    #[serde(default)]
    pub daily_threshold_bytes: u64,
}

impl ScheduleConfig {
    pub fn sanitize(mut self) -> Self {
        self.tolerance_min = self.tolerance_min.clamp(1, 10);
        self.entries.retain(|entry| {
            // 丢弃格式错误的计划项：时间不是 HH:MM 或动作无效
            valid_schedule_time(&entry.time)
        });
        self
    }
}

/// 校验 HH:MM 格式
fn valid_schedule_time(time: &str) -> bool {
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    let hour: u8 = match parts[0].parse() {
        Ok(h) => h,
        Err(_) => return false,
    };
    let minute: u8 = match parts[1].parse() {
        Ok(m) => m,
        Err(_) => return false,
    };
    hour <= 23 && minute <= 59
}

impl CallControlConfig {
    pub fn sanitize(mut self) -> Self {
        // 号码去空白、去连字符，过滤空项并去重。
        let mut seen = std::collections::HashSet::new();
        self.numbers = self
            .numbers
            .into_iter()
            .map(|number| normalize_phone_number(&number))
            .filter(|number| !number.is_empty() && seen.insert(number.clone()))
            .collect();
        // 每条命令的 duration_secs 限定 [3, 30] 秒范围；去重（同 duration 只保留第一条）；最多 10 条。
        let mut seen_dur = std::collections::HashSet::new();
        for cmd in &mut self.duration_commands {
            cmd.duration_secs = cmd.duration_secs.clamp(3, 30);
        }
        self.duration_commands.retain(|cmd| seen_dur.insert(cmd.duration_secs));
        self.duration_commands.truncate(10);
        self
    }
}

impl TrafficAlertConfig {
    pub fn sanitize(self) -> Self {
        Self { ..self }
    }
}

/// 射频模式持久化。
///
/// ofono 的 `TechnologyPreference` 在某些 UDX710 固件上冷启动后可能恢复到默认值。
/// 将此字段写入 config.json，每次开机/重连后强制重套，确保"仅 4G"等设置不漂移。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadioModeConfig {
    /// "auto" | "lte" | "nr"，与 ofono TechnologyPreference 值一致
    #[serde(default = "default_radio_mode")]
    pub mode: String,
}

fn default_radio_mode() -> String {
    "auto".to_string()
}

impl Default for RadioModeConfig {
    fn default() -> Self {
        Self {
            mode: default_radio_mode(),
        }
    }
}

impl RadioModeConfig {
    pub fn sanitize(mut self) -> Self {
        if !matches!(self.mode.as_str(), "auto" | "lte" | "nr") {
            self.mode = "auto".to_string();
        }
        self
    }

    pub fn is_auto(&self) -> bool {
        self.mode == "auto"
    }
}

/// 频段锁定持久化（AT+SPLBAND）。
///
/// 虽然 SPLBAND 写入 modem NVRAM 通常断电不丢，但不同固件行为不一致，
/// 此处冗余保存到 config.json 作为兜底。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandLockConfig {
    #[serde(default)]
    pub lte_fdd_bands: Vec<u8>,
    #[serde(default)]
    pub lte_tdd_bands: Vec<u8>,
    #[serde(default)]
    pub nr_fdd_bands: Vec<u8>,
    #[serde(default)]
    pub nr_tdd_bands: Vec<u8>,
}

impl BandLockConfig {
    pub fn is_empty(&self) -> bool {
        self.lte_fdd_bands.is_empty()
            && self.lte_tdd_bands.is_empty()
            && self.nr_fdd_bands.is_empty()
            && self.nr_tdd_bands.is_empty()
    }
}

/// 小区锁定持久化（AT+SPFORCEFRQ）。
///
/// 小区锁是 RAM 态的，冷启动、网络去附着均会丢失。
/// 写入 config.json 确保每次开机/重连后自动重套。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CellLockConfig {
    #[serde(default)]
    pub lte_arfcn: Option<u32>,
    #[serde(default)]
    pub lte_pci: Option<u16>,
    #[serde(default)]
    pub nr_arfcn: Option<u32>,
    #[serde(default)]
    pub nr_pci: Option<u16>,
}

impl CellLockConfig {
    pub fn has_lte(&self) -> bool {
        self.lte_arfcn.is_some() && self.lte_pci.is_some()
    }

    pub fn has_nr(&self) -> bool {
        self.nr_arfcn.is_some() && self.nr_pci.is_some()
    }

    pub fn is_empty(&self) -> bool {
        !self.has_lte() && !self.has_nr()
    }
}

/// 规范化电话号码：去掉空白与连字符，统一用于来电号码匹配。
pub fn normalize_phone_number(number: &str) -> String {
    number
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '(' && *c != ')')
        .collect()
}

/// MQTT Broker 节点
///
/// 每个节点自带主机、端口与 TLS 开关，用户可在页面上逐条编辑，
/// 不再共享一个全局端口（公共 Broker 的 1883 与自建 EMQX 的 8883 常常并存）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MqttBrokerNode {
    /// 主机名或 IP，可带 `ssl://` / `tls://` 前缀（自动启用 TLS）
    #[serde(default)]
    pub host: String,
    /// 端口；未填写（0）时按是否 TLS 取 8883 / 1883
    #[serde(default)]
    pub port: u16,
    /// 是否使用 TLS 加密连接
    #[serde(default)]
    pub tls: bool,
}

impl MqttBrokerNode {
    pub fn new(host: impl Into<String>, port: u16, tls: bool) -> Self {
        Self {
            host: host.into(),
            port,
            tls,
        }
    }

    /// 明文/TLS 的默认端口
    pub const PLAIN_PORT: u16 = 1883;
    pub const TLS_PORT: u16 = 8883;

    /// 剥掉 `ssl://` / `tls://` / `mqtt://` 前缀，返回 (纯主机, 是否 TLS)。
    ///
    /// 前缀优先级高于 `tls` 字段：`mqtt://` 显式表示明文，`ssl://` / `tls://`
    /// 显式表示加密，无前缀时才回落到配置的 `tls` 值。这样用户直接粘贴
    /// `ssl://broker.example.com` 就能加密，不必再单独打开开关。
    ///
    /// rumqttc 只接受纯主机名，前缀必须由这里剥掉，否则 DNS 解析必然失败。
    pub fn resolve(&self) -> (String, bool) {
        let host = self.host.trim();
        let lower = host.to_ascii_lowercase();
        // 前缀都是 ASCII，长度固定；用长度切片以保留用户输入的原始大小写。
        let (prefix_len, forced_tls) = if lower.starts_with("ssl://") || lower.starts_with("tls://") {
            (6usize, Some(true))
        } else if lower.starts_with("mqtt://") {
            (7usize, Some(false))
        } else {
            (0usize, None)
        };
        let bare = host[prefix_len.min(host.len())..].trim();
        (bare.to_string(), forced_tls.unwrap_or(self.tls))
    }

    /// 实际连接端口：未填写时按 TLS 与否取默认值
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 {
            if self.resolve().1 {
                Self::TLS_PORT
            } else {
                Self::PLAIN_PORT
            }
        } else {
            self.port
        }
    }

    /// 用于展示与旧版配置镜像的写法：TLS 节点带 `ssl://` 前缀
    pub fn display(&self) -> String {
        let (host, tls) = self.resolve();
        if tls {
            format!("ssl://{host}")
        } else {
            host
        }
    }

    /// 连接用的完整地址（含端口），仅用于日志
    pub fn endpoint(&self) -> String {
        let (host, _) = self.resolve();
        format!("{host}:{}", self.effective_port())
    }
}

/// MQTT 远程控制配置
///
/// 用于国内蜂窝网络与纯数据物联卡的远程运维场景。
/// 支持多 Broker 节点容灾轮询，Client ID 动态拼接 IMEI 避免重名。
/// 支持 SSL/TLS 加密连接（节点级 `tls` 或 `ssl://` 前缀）及用户名/密码认证。
#[derive(Debug, Clone, Serialize)]
pub struct MqttConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Broker 节点列表（按优先级排序，失败时自动轮询）。
    ///
    /// 这是唯一的权威数据源；下面的 `broker_list` / `active_broker` / `port` / `tls`
    /// 是写给旧版本二进制读的镜像字段，读取时忽略。
    #[serde(default = "default_mqtt_nodes")]
    pub nodes: Vec<MqttBrokerNode>,
    /// 旧版：Broker 节点字符串列表（降级兼容镜像，勿直接读取）
    #[serde(default = "default_broker_list")]
    pub broker_list: Vec<String>,
    /// 旧版：当前使用的 Broker（降级兼容镜像，勿直接读取）
    #[serde(default)]
    pub active_broker: String,
    /// 旧版：全局 MQTT 端口（降级兼容镜像，勿直接读取）
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    /// 订阅主题（接收指令）
    #[serde(default = "default_topic_sub")]
    pub topic_sub: String,
    /// 发布主题（发送状态）
    #[serde(default = "default_topic_pub")]
    pub topic_pub: String,
    /// 指令鉴权 Token（防止公共 Broker 上被误触）
    #[serde(default)]
    pub auth_token: Option<String>,
    /// 旧版：全局 TLS 开关（降级兼容镜像，勿直接读取）
    #[serde(default)]
    pub tls: bool,
    /// MQTT 用户名（可选）
    #[serde(default)]
    pub username: Option<String>,
    /// MQTT 密码（可选）
    #[serde(default)]
    pub password: Option<String>,
    /// 连接前等待蜂窝数据连接就绪的秒数。`0` = 始终等待（默认）；`>0` = 最多等这么久，
    /// 超时后即使未联网也尝试连接（覆盖纯 WAN/直连等无蜂窝场景）。
    #[serde(default)]
    pub data_wait_timeout_secs: u64,
}

/// 节点数量上限：轮询一圈的耗时与节点数成正比，过多会让故障切换变得迟钝
const MAX_MQTT_NODES: usize = 16;

fn default_mqtt_nodes() -> Vec<MqttBrokerNode> {
    vec![
        MqttBrokerNode::new("broker.emqx.io", MqttBrokerNode::PLAIN_PORT, false),
        MqttBrokerNode::new("broker-cn.emqx.io", MqttBrokerNode::PLAIN_PORT, false),
        MqttBrokerNode::new("test.mosquitto.org", MqttBrokerNode::PLAIN_PORT, false),
    ]
}

fn default_broker_list() -> Vec<String> {
    vec![
        "broker.emqx.io".to_string(),
        "broker-cn.emqx.io".to_string(),
        "test.mosquitto.org".to_string(),
    ]
}

fn default_mqtt_port() -> u16 {
    MqttBrokerNode::PLAIN_PORT
}

fn default_topic_sub() -> String {
    "cpe/{imei}/cmd".to_string()
}

fn default_topic_pub() -> String {
    "cpe/{imei}/status".to_string()
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            nodes: default_mqtt_nodes(),
            broker_list: default_broker_list(),
            active_broker: String::new(),
            port: default_mqtt_port(),
            topic_sub: default_topic_sub(),
            topic_pub: default_topic_pub(),
            auth_token: None,
            tls: false,
            username: None,
            password: None,
            data_wait_timeout_secs: 0,
        }
    }
}

/// 手写 `Deserialize` 以兼容两种历史 schema：
///
/// * 旧版：`broker_list: Vec<String>` + 全局 `port` / `tls` / `active_broker`
/// * 新版：`nodes: Vec<MqttBrokerNode>`
///
/// 迁移必须放在反序列化层而不是 `sanitize()`，因为 `ConfigManager::new` 在解析失败时
/// 会回落到整个 `AppConfig::default()`——那会把用户所有配置清空。这里保证「只要 JSON
/// 合法就一定能读出可用配置」，绝不向上抛错。
impl<'de> Deserialize<'de> for MqttConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// 中间表示：字段名覆盖新旧两套 schema，缺失一律取 `None` / 默认，
        /// 以便区分「用户没写」与「用户写了空值」。
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Raw {
            enabled: bool,
            nodes: Option<Vec<MqttBrokerNode>>,
            broker_list: Vec<String>,
            active_broker: String,
            port: u16,
            tls: bool,
            topic_sub: String,
            topic_pub: String,
            auth_token: Option<String>,
            username: Option<String>,
            password: Option<String>,
            data_wait_timeout_secs: u64,
        }

        let raw = Raw::deserialize(deserializer)?;

        // 新版字段优先；只有完全没写 nodes 时才从旧版字段迁移
        let nodes = match raw.nodes {
            Some(nodes) if !nodes.is_empty() => nodes,
            _ => {
                let port = if raw.port == 0 {
                    MqttBrokerNode::PLAIN_PORT
                } else {
                    raw.port
                };
                let mut migrated: Vec<MqttBrokerNode> = raw
                    .broker_list
                    .iter()
                    .map(|host| MqttBrokerNode::new(host.trim(), port, raw.tls))
                    .collect();
                // active_broker 可能不在 broker_list 里（历史脏数据），按原语义插到最前
                let active = raw.active_broker.trim();
                if !active.is_empty()
                    && !migrated.iter().any(|n| n.host.trim().eq_ignore_ascii_case(active))
                {
                    migrated.insert(0, MqttBrokerNode::new(active, port, raw.tls));
                }
                migrated
            }
        };

        let mut config = MqttConfig {
            enabled: raw.enabled,
            nodes,
            broker_list: raw.broker_list,
            active_broker: raw.active_broker,
            port: raw.port,
            topic_sub: if raw.topic_sub.trim().is_empty() {
                default_topic_sub()
            } else {
                raw.topic_sub
            },
            topic_pub: if raw.topic_pub.trim().is_empty() {
                default_topic_pub()
            } else {
                raw.topic_pub
            },
            auth_token: raw.auth_token,
            tls: raw.tls,
            username: raw.username,
            password: raw.password,
            data_wait_timeout_secs: raw.data_wait_timeout_secs,
        };
        config = config.sanitize();
        Ok(config)
    }
}

impl MqttConfig {
    /// 归一化配置：清洗节点、补齐默认值，并同步旧版镜像字段。
    pub fn sanitize(mut self) -> Self {
        let mut cleaned: Vec<MqttBrokerNode> = Vec::with_capacity(self.nodes.len());
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for node in self.nodes.into_iter() {
            let host = node.host.trim();
            // 剥掉前缀后为空的节点（如只写了 "ssl://"）直接丢弃，避免 DNS 解析必然失败
            if node.resolve().0.is_empty() {
                continue;
            }
            // 去重：同一 host+port+tls 只保留第一条（用户粘贴重复节点时很常见）
            let key = format!("{}:{}:{}", node.resolve().0, node.effective_port(), node.resolve().1);
            if !seen.insert(key) {
                continue;
            }
            cleaned.push(MqttBrokerNode {
                host: host.to_string(),
                port: node.port,
                tls: node.tls,
            });
        }

        if cleaned.is_empty() {
            cleaned = default_mqtt_nodes();
        }
        cleaned.truncate(MAX_MQTT_NODES);
        self.nodes = cleaned;

        if self.topic_sub.trim().is_empty() {
            self.topic_sub = default_topic_sub();
        }
        if self.topic_pub.trim().is_empty() {
            self.topic_pub = default_topic_pub();
        }

        // 认证凭据：没有用户名时密码无意义（连接代码以 username 为开关）
        let username = self.username.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        self.username = username;
        if self.username.is_none() {
            self.password = None;
        }
        if let Some(ref mut token) = self.auth_token {
            let trimmed = token.trim().to_string();
            self.auth_token = if trimmed.is_empty() { None } else { Some(trimmed) };
        }

        self.sync_legacy_mirror();
        self
    }

    /// 把节点列表回写到旧版字段，使 OTA 回滚到旧二进制时配置仍然可读。
    ///
    /// 旧版只有一个全局端口，这里取首节点的端口/TLS 作为近似——回滚场景下
    /// 至少首选节点能正常连上，不至于因为端口不匹配而完全失联。
    fn sync_legacy_mirror(&mut self) {
        self.broker_list = self.nodes.iter().map(|n| n.display()).collect();

        // active_broker 决定连接时从哪个节点开始（页面上的 ★）。
        // 只要它仍匹配某个节点的 display()，就保留用户的选择；否则回落到首节点。
        let active_valid = self
            .nodes
            .iter()
            .any(|n| n.display() == self.active_broker);
        match self.nodes.first() {
            Some(first) => {
                if !active_valid {
                    self.active_broker = first.display();
                }
                // 端口/TLS 镜像取「激活节点」，回滚旧二进制时优先连用户指定的那个
                let active_node = self
                    .nodes
                    .iter()
                    .find(|n| n.display() == self.active_broker)
                    .unwrap_or(first);
                self.port = active_node.effective_port();
                self.tls = active_node.resolve().1;
            }
            None => {
                self.active_broker = String::new();
                self.port = default_mqtt_port();
                self.tls = false;
            }
        }
    }
}

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub sms_push: SmsPushConfig,
    #[serde(default)]
    pub refresh: RefreshConfig,
    #[serde(default)]
    pub restart: RestartConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub call_control: CallControlConfig,
    #[serde(default)]
    pub sms_control: SmsControlConfig,
    #[serde(default)]
    pub traffic_alert: TrafficAlertConfig,
    #[serde(default)]
    pub net_health: NetHealthConfig,
    #[serde(default)]
    pub radio_mode: RadioModeConfig,
    #[serde(default)]
    pub band_lock: BandLockConfig,
    #[serde(default)]
    pub cell_lock: CellLockConfig,
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub remote_control_push: RemoteControlPushConfig,
}


/// 配置管理器
///
/// 注意：当前每次 setter 都同步写盘（原子写）。未来可改为去抖动保存
/// 以减少 UBIFS 闪存写入次数，但需要在 ConfigManager 中引入 Arc<AtomicBool>
/// 共享状态并改用 Arc<ConfigManager> 接口，属于 P2 优化项。
pub struct ConfigManager {
    config: Arc<RwLock<AppConfig>>,
    config_path: PathBuf,
}

impl ConfigManager {
    /// 创建新的配置管理器
    pub fn new(config_path: PathBuf) -> Self {
        let config = if config_path.exists() {
            match fs::read_to_string(&config_path) {
                Ok(content) => {
                    match serde_json::from_str::<AppConfig>(&content) {
                        Ok(cfg) => AppConfig {
                            refresh: cfg.refresh.sanitize(),
                            restart: cfg.restart.sanitize(),
                            schedule: cfg.schedule.sanitize(),
                            call_control: cfg.call_control.sanitize(),
                            sms_control: cfg.sms_control.sanitize(),
                            traffic_alert: cfg.traffic_alert.sanitize(),
                            net_health: cfg.net_health.sanitize(),
                            radio_mode: cfg.radio_mode.sanitize(),
                            mqtt: cfg.mqtt.sanitize(),
                            ..cfg
                        },
                        Err(e) => {
                            // 解析失败时先备份原文件再回落默认值。
                            //
                            // 为什么必须备份：所有 setter 都会调 save() 写盘，用户在页面上
                            // 随手改一个开关，内存里的「默认值」就会覆盖磁盘上的原文件，
                            // 用户配置从此永久丢失。嵌入式 UBIFS 上掉电截断 JSON 并不罕见，
                            // 留一份 .corrupt 副本至少能人工抢救。
                            let backup_path = corrupt_backup_path(&config_path);
                            match fs::copy(&config_path, &backup_path) {
                                Ok(_) => warn!(
                                    error = %e,
                                    backup = %backup_path.display(),
                                    "Failed to parse config file, original backed up, using defaults"
                                ),
                                Err(copy_err) => warn!(
                                    error = %e,
                                    copy_error = %copy_err,
                                    "Failed to parse config file and could not back it up, using defaults"
                                ),
                            }
                            AppConfig::default()
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to read config file, using defaults");
                    AppConfig::default()
                }
            }
        } else {
            info!("No config file found, using defaults");
            AppConfig::default()
        };

        let manager = Self {
            config: Arc::new(RwLock::new(config)),
            config_path,
        };

        // 保存默认配置（如果文件不存在）
        if !manager.config_path.exists() {
            let _ = manager.save();
        }

        manager
    }
    
    /// 获取当前配置
    #[allow(dead_code)]
    pub fn get(&self) -> AppConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).clone()
    }
    
    /// 获取 Webhook 配置
    pub fn get_webhook(&self) -> WebhookConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).webhook.clone()
    }
    
    pub fn get_webhook_for_response(&self) -> WebhookConfig {
        let mut webhook = self.get_webhook();
        webhook.secret.clear();
        webhook
    }

    /// Empty secret means "keep the existing secret" so a masked GET response
    /// can be saved again without accidentally erasing credentials.
    pub fn set_webhook(&self, mut webhook: WebhookConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            if webhook.secret.is_empty() {
                webhook.secret = config.webhook.secret.clone();
            }
            config.webhook = webhook;
        }
        self.save()
    }

    pub fn get_sms_push(&self) -> SmsPushConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).sms_push.clone()
    }

    pub fn get_sms_push_for_response(&self) -> SmsPushConfig {
        let mut sms_push = self.get_sms_push();
        sms_push.credential.clear();
        sms_push
    }

    pub fn set_sms_push(&self, mut sms_push: SmsPushConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            if sms_push.credential.is_empty() {
                sms_push.credential = config.sms_push.credential.clone();
            }
            config.sms_push = sms_push;
        }
        self.save()
    }

    pub fn get_refresh(&self) -> RefreshConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).refresh.clone().sanitize()
    }

    pub fn set_refresh(&self, refresh: RefreshConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.refresh = refresh.sanitize();
        }
        self.save()
    }

    pub fn get_restart(&self) -> RestartConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).restart.clone().sanitize()
    }

    pub fn set_restart(&self, restart: RestartConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.restart = restart.sanitize();
        }
        self.save()
    }

    pub fn get_schedule(&self) -> ScheduleConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).schedule.clone()
    }

    pub fn set_schedule(&self, schedule: ScheduleConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.schedule = schedule.sanitize();
        }
        self.save()
    }

    pub fn get_call_control(&self) -> CallControlConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).call_control.clone()
    }

    pub fn set_call_control(&self, call_control: CallControlConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.call_control = call_control.sanitize();
        }
        self.save()
    }

    pub fn get_sms_control(&self) -> SmsControlConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).sms_control.clone()
    }

    pub fn set_sms_control(&self, sms_control: SmsControlConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.sms_control = sms_control.sanitize();
        }
        self.save()
    }

    /// 获取用于短信遥控的白名单号码（复用通话遥控的白名单）。
    pub fn get_sms_control_whitelist(&self) -> Vec<String> {
        self.get_call_control().numbers
    }

    pub fn get_traffic_alert(&self) -> TrafficAlertConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).traffic_alert.clone()
    }

    pub fn set_traffic_alert(&self, traffic_alert: TrafficAlertConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.traffic_alert = traffic_alert.sanitize();
        }
        self.save()
    }

    pub fn get_net_health(&self) -> NetHealthConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).net_health.clone().sanitize()
    }

    pub fn set_net_health(&self, net_health: NetHealthConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.net_health = net_health.sanitize();
        }
        self.save()
    }

    pub fn get_radio_mode_cfg(&self) -> RadioModeConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).radio_mode.clone().sanitize()
    }

    pub fn set_radio_mode_cfg(&self, mode: &str) -> Result<(), String> {
        let cfg = RadioModeConfig { mode: mode.to_string() }.sanitize();
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.radio_mode = cfg;
        }
        self.save()
    }

    pub fn get_band_lock(&self) -> BandLockConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).band_lock.clone()
    }

    pub fn set_band_lock(&self, band_lock: BandLockConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.band_lock = band_lock;
        }
        self.save()
    }

    pub fn get_cell_lock(&self) -> CellLockConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).cell_lock.clone()
    }

    pub fn set_cell_lock(&self, cell_lock: CellLockConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.cell_lock = cell_lock;
        }
        self.save()
    }

    pub fn get_mqtt(&self) -> MqttConfig {
        self.config.read().unwrap_or_else(|p| p.into_inner()).mqtt.clone()
    }

    pub fn set_mqtt(&self, mqtt: MqttConfig) -> Result<(), String> {
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            config.mqtt = mqtt.sanitize();
        }
        self.save()
    }

    pub fn get_remote_control_push(&self) -> RemoteControlPushConfig {
        let config = self.config.read().unwrap_or_else(|p| p.into_inner()).remote_control_push.clone();
        // 修复旧模板：若保存的模板是旧格式（含 msg_type 而非 msgtype），自动替换为企业微信格式并持久化
        if config.template.contains("msg_type") {
            let new_template = default_remote_control_push_template();
            warn!("Remote control push template was old format (msg_type), auto-upgrading to wecom msgtype format");
            drop(config);
            // 短暂升级为写锁，修复模板
            {
                let mut app = self.config.write().unwrap_or_else(|p| p.into_inner());
                if app.remote_control_push.template.contains("msg_type") {
                    app.remote_control_push.template = new_template;
                }
            }
            let _ = self.save();
            return self.config.read().unwrap_or_else(|p| p.into_inner()).remote_control_push.clone();
        }
        config
    }

    pub fn set_remote_control_push(&self, config: RemoteControlPushConfig) -> Result<(), String> {
        {
            let mut app_config = self.config.write().unwrap_or_else(|p| p.into_inner());
            app_config.remote_control_push = config;
        }
        self.save()
    }

    #[allow(dead_code)]
    pub fn set(&self, config: AppConfig) -> Result<(), String> {
        {
            let mut current = self.config.write().unwrap_or_else(|p| p.into_inner());
            *current = AppConfig {
                refresh: config.refresh.sanitize(),
                restart: config.restart.sanitize(),
                ..config
            };
        }
        self.save()
    }
    
    /// 保存配置到文件（原子写入：先写临时文件，再 rename 替换）。
    ///
    /// RwLock 读锁在序列化完成后立即释放，后续磁盘 I/O 不持有锁，
    /// 避免阻塞其他 setter 的写锁获取。
    pub fn save(&self) -> Result<(), String> {
        let content = {
            let config = self.config.read().unwrap_or_else(|p| p.into_inner());
            serde_json::to_string_pretty(&*config)
                .map_err(|e| format!("Failed to serialize config: {}", e))?
        }; // 读锁在此释放

        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        let tmp_path = self.config_path.with_extension("json.tmp");
        fs::write(&tmp_path, &content)
            .map_err(|e| format!("Failed to write config file: {}", e))?;
        set_private_file_permissions(&tmp_path)?;
        fs::rename(&tmp_path, &self.config_path)
            .map_err(|e| {
                let _ = fs::remove_file(&tmp_path);
                format!("Failed to publish config file: {}", e)
            })?;

        Ok(())
    }
    
    /// 重新加载配置
    #[allow(dead_code)]
    pub fn reload(&self) -> Result<(), String> {
        if !self.config_path.exists() {
            return Err("Config file does not exist".to_string());
        }
        
        let content = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        
        let new_config: AppConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config file: {}", e))?;
        
        {
            let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
            *config = AppConfig {
                refresh: new_config.refresh.sanitize(),
                restart: new_config.restart.sanitize(),
                ..new_config
            };
        }
        
        Ok(())
    }
}

/// 解析失败时的备份路径：`config.json` → `config.json.corrupt`。
///
/// 用追加后缀而不是替换扩展名（`config.corrupt`），是为了让备份名里仍带 `.json`，
/// 人工抢救时一眼能看出原始格式，也便于编辑器直接语法高亮。
fn corrupt_backup_path(config_path: &Path) -> PathBuf {
    let mut backup = config_path.as_os_str().to_os_string();
    backup.push(".corrupt");
    PathBuf::from(backup)
}

/// 获取默认配置文件路径
pub fn get_persistent_root_dir() -> PathBuf {
    let device_root = PathBuf::from("/data");
    if device_root.exists() {
        return device_root;
    }

    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_default_config_path() -> PathBuf {
    get_persistent_root_dir().join("config.json")
}

fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(_path)
            .map_err(|e| format!("Failed to read metadata for {}: {}", _path.display(), e))?
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(_path, permissions)
            .map_err(|e| format!("Failed to protect {}: {}", _path.display(), e))?;
    }
    Ok(())
}

fn normalize_newlines(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn is_ota_hook_line(line: &str) -> bool {
    let trimmed = line.trim();

    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }

    trimmed == "sh /home/root/ota.sh &"
        || trimmed == "/home/root/ota.sh"
        || trimmed == "/home/root/ota.sh &"
        || trimmed.starts_with("sh /home/root/ota.sh")
}

fn is_init_hook_line(line: &str) -> bool {
    let trimmed = line.trim();

    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }

    trimmed == INIT_SCRIPT_LOADER_COMMAND
        || trimmed == INIT_SCRIPT_PATH
        || trimmed == format!("{} &", INIT_SCRIPT_PATH)
        || trimmed.starts_with(&format!("sh {}", INIT_SCRIPT_PATH))
}

fn loader_contains_ota_command(content: &str) -> bool {
    content.lines().any(is_ota_hook_line)
}

fn loader_contains_init_command(content: &str) -> bool {
    content.lines().any(is_init_hook_line)
}

fn remove_ota_command_from_loader(content: &str) -> String {
    let normalized = normalize_newlines(content);
    let mut filtered_lines: Vec<&str> = normalized
        .lines()
        .filter(|line| !is_ota_hook_line(line))
        .collect();

    while filtered_lines.last().is_some_and(|line| line.trim().is_empty()) {
        filtered_lines.pop();
    }

    if filtered_lines.is_empty() {
        return String::new();
    }

    format!("{}\n", filtered_lines.join("\n"))
}

fn append_init_command_to_loader(content: &str) -> String {
    let normalized = normalize_newlines(content);

    if loader_contains_init_command(&normalized) {
        return format!("{}\n", normalized.trim_end_matches('\n'));
    }

    let base = if normalized.trim().is_empty() {
        DEFAULT_LOADER_SCRIPT.trim_end_matches('\n').to_string()
    } else {
        normalized.trim_end_matches('\n').to_string()
    };

    format!("{}\n{}\n", base, INIT_SCRIPT_LOADER_COMMAND)
}

fn loader_uses_ab_bootstrap(content: &str) -> bool {
    content.contains("UDX710 OTA bootstrap")
        || content.contains("OTA_STATE_FILE=\"/home/root/ota/state.env\"")
}

fn loader_is_plain_legacy_bootstrap(content: &str) -> bool {
    let script_lines: Vec<&str> = content
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#') || *line == "#!/bin/sh")
        .collect();

    if script_lines.len() < 3 {
        return false;
    }

    if script_lines[0] != "#!/bin/sh" {
        return false;
    }

    if script_lines[1] != "/home/root/ttyd/start.sh &"
        || script_lines[2] != "/home/root/udx710 -p 80 &"
    {
        return false;
    }

    script_lines[3..]
        .iter()
        .all(|line| *line == INIT_SCRIPT_LOADER_COMMAND)
}

fn set_executable_permissions(_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(_path)
            .map_err(|e| format!("Failed to read metadata for {}: {}", _path.display(), e))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(_path, permissions)
            .map_err(|e| format!("Failed to set permissions for {}: {}", _path.display(), e))?;
    }

    Ok(())
}

pub fn ensure_loader_hooks_init() -> Result<(), String> {
    let loader_path = PathBuf::from(LOADER_SCRIPT_PATH);
    let current_content = if loader_path.exists() {
        fs::read_to_string(&loader_path)
            .map_err(|e| format!("Failed to read loader.sh: {}", e))?
    } else {
        String::new()
    };

    let stripped_content = remove_ota_command_from_loader(&current_content);
    let missing_backend_command = !stripped_content
        .lines()
        .any(|line| line.trim() == "/home/root/udx710 -p 80 &");

    let base_content = if loader_uses_ab_bootstrap(&current_content)
        || loader_contains_ota_command(&current_content)
        || missing_backend_command
    {
        DEFAULT_LOADER_SCRIPT.to_string()
    } else if current_content.trim().is_empty()
        || loader_is_plain_legacy_bootstrap(&current_content)
    {
        DEFAULT_LOADER_SCRIPT.to_string()
    } else {
        stripped_content
    };

    let updated_content = append_init_command_to_loader(&base_content);

    if let Some(parent) = loader_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create loader.sh directory: {}", e))?;
    }

    fs::write(&loader_path, updated_content)
        .map_err(|e| format!("Failed to write loader.sh: {}", e))?;
    set_executable_permissions(&loader_path)?;

    // The loader always calls init.sh. Create a harmless executable placeholder when
    // a device has never saved an init script, avoiding a noisy boot-time shell error.
    let init_path = PathBuf::from(INIT_SCRIPT_PATH);
    if !init_path.exists() {
        fs::write(&init_path, "#!/bin/sh\n")
            .map_err(|e| format!("Failed to create default init.sh: {}", e))?;
        set_executable_permissions(&init_path)?;
    }

    let _ = fs::remove_file("/home/root/ota.sh");

    Ok(())
}

pub fn get_init_script() -> Result<crate::models::InitScriptResponse, String> {
    let loader_content = if Path::new(LOADER_SCRIPT_PATH).exists() {
        fs::read_to_string(LOADER_SCRIPT_PATH)
            .map_err(|e| format!("Failed to read loader.sh: {}", e))?
    } else {
        DEFAULT_LOADER_SCRIPT.to_string()
    };

    let script = match fs::read_to_string(INIT_SCRIPT_PATH) {
        Ok(content) => normalize_newlines(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Failed to read init.sh: {}", e)),
    };

    Ok(crate::models::InitScriptResponse {
        script,
        init_path: INIT_SCRIPT_PATH.to_string(),
        loader_path: LOADER_SCRIPT_PATH.to_string(),
        loader_hooked: loader_contains_init_command(&loader_content),
    })
}

pub fn set_init_script(script: String) -> Result<crate::models::InitScriptResponse, String> {
    let init_path = PathBuf::from(INIT_SCRIPT_PATH);
    if let Some(parent) = init_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create init.sh directory: {}", e))?;
    }

    fs::write(&init_path, normalize_newlines(&script))
        .map_err(|e| format!("Failed to write init.sh: {}", e))?;
    set_executable_permissions(&init_path)?;

    ensure_loader_hooks_init()?;

    get_init_script()
}

#[cfg(test)]
mod tests {
    use super::{
        append_init_command_to_loader,
        corrupt_backup_path,
        default_mqtt_nodes,
        default_topic_pub,
        default_topic_sub,
        loader_contains_init_command,
        loader_contains_ota_command,
        remove_ota_command_from_loader,
        AppConfig,
        MqttBrokerNode,
        MqttConfig,
        RestartConfig,
        ScheduleAction,
        INIT_SCRIPT_LOADER_COMMAND,
        MAX_MQTT_NODES,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn append_init_command_once_for_new_loader() {
        let loader = "#!/bin/sh\n/home/root/ttyd/start.sh &\n/home/root/udx710 -p 80 &\n";
        let updated = append_init_command_to_loader(loader);

        assert!(updated.contains(INIT_SCRIPT_LOADER_COMMAND));
        assert_eq!(updated.matches(INIT_SCRIPT_LOADER_COMMAND).count(), 1);
    }

    #[test]
    fn append_init_command_is_idempotent() {
        let loader = format!(
            "#!/bin/sh\n/home/root/ttyd/start.sh &\n/home/root/udx710 -p 80 &\n{}\n",
            INIT_SCRIPT_LOADER_COMMAND
        );
        let updated = append_init_command_to_loader(&loader);

        assert_eq!(updated.matches(INIT_SCRIPT_LOADER_COMMAND).count(), 1);
    }

    #[test]
    fn loader_detects_init_command() {
        let loader = format!("#!/bin/sh\n{}\n", INIT_SCRIPT_LOADER_COMMAND);
        assert!(loader_contains_init_command(&loader));
    }

    #[test]
    fn loader_ignores_commented_init_command() {
        let loader = format!("#!/bin/sh\n# {}\n", INIT_SCRIPT_LOADER_COMMAND);
        assert!(!loader_contains_init_command(&loader));
    }

    #[test]
    fn remove_ota_command_from_loader_strips_old_hook() {
        let loader = "#!/bin/sh\n/home/root/ttyd/start.sh &\nsh /home/root/ota.sh &\n/home/root/udx710 -p 80 &\n";
        let updated = remove_ota_command_from_loader(loader);

        assert!(!loader_contains_ota_command(&updated));
        assert!(updated.contains("/home/root/udx710 -p 80 &"));
    }

    #[test]
    fn restart_config_sanitizes_values() {
        let sanitized = RestartConfig {
            schedule_enabled: true,
            schedule_interval_days: 0,
            low_memory_enabled: true,
            low_memory_threshold_percent: 100,
        }
        .sanitize();

        assert_eq!(sanitized.schedule_interval_days, 1);
        assert_eq!(sanitized.low_memory_threshold_percent, 50);
    }

    #[test]
    fn legacy_config_defaults_restart_policies_to_disabled() {
        let config: AppConfig = serde_json::from_str(r#"{"refresh":{"interval_ms":5000}}"#).unwrap();

        assert!(!config.restart.schedule_enabled);
        assert!(!config.restart.low_memory_enabled);
    }

    // === 旧配置缺字段时必须能解析（升级兼容）===
    //
    // 这一组测试守护的是一条会导致用户配置永久丢失的路径：
    // 旧 config.json 缺某个字段 → serde 报 missing field → ConfigManager::new
    // 回落 AppConfig::default() → 用户在页面改任一开关 → setter 调 save()
    // → 磁盘上的原配置被默认值覆盖。
    //
    // 因此任何新增到 AppConfig 及其嵌套结构体的字段，都必须带 #[serde(default)]。

    #[test]
    fn partial_webhook_config_parses_without_missing_field_error() {
        // 早期版本写出的 webhook 段可能只有 headers/secret，没有 enabled/url 等
        let config: AppConfig = serde_json::from_str(
            r#"{"webhook":{"headers":{"X-Token":"t"},"secret":"s"}}"#,
        )
        .expect("部分 webhook 配置必须能解析，否则升级会清空用户配置");

        assert_eq!(config.webhook.secret, "s");
        assert_eq!(config.webhook.headers.get("X-Token").map(String::as_str), Some("t"));
        // 缺失字段取 Default 值：开关关闭、转发开启
        assert!(!config.webhook.enabled);
        assert!(config.webhook.url.is_empty());
        assert!(config.webhook.forward_sms);
        assert!(config.webhook.forward_calls);
    }

    #[test]
    fn empty_webhook_object_parses() {
        // 极端情况：webhook 段存在但完全是空对象
        let config: AppConfig = serde_json::from_str(r#"{"webhook":{}}"#)
            .expect("空 webhook 对象必须能解析");
        assert!(!config.webhook.enabled);
        assert!(config.webhook.forward_sms);
    }

    #[test]
    fn empty_sms_push_object_parses() {
        let config: AppConfig = serde_json::from_str(r#"{"sms_push":{}}"#)
            .expect("空 sms_push 对象必须能解析");
        assert!(!config.sms_push.enabled);
        assert!(config.sms_push.credential.is_empty());
        assert!(config.sms_push.server_url.is_empty());
    }

    #[test]
    fn schedule_entry_with_only_enabled_parses() {
        // 旧计划项可能只写了 enabled，缺 time/action
        let config: AppConfig = serde_json::from_str(
            r#"{"schedule":{"entries":[{"enabled":true}]}}"#,
        )
        .expect("缺 time/action 的计划项必须能解析");

        assert_eq!(config.schedule.entries.len(), 1);
        assert!(config.schedule.entries[0].time.is_empty());
        // action 落到枚举 Default（Reboot）
        assert_eq!(config.schedule.entries[0].action, ScheduleAction::Reboot);
    }

    #[test]
    fn schedule_sanitize_drops_entry_with_empty_time() {
        // 兜底链条的第二环：缺 time 的计划项虽能解析，但必须被 sanitize 丢弃，
        // 否则会留下一条「HH:MM 永远匹配不上」的僵尸计划。
        let config: AppConfig = serde_json::from_str(
            r#"{"schedule":{"entries":[{"enabled":true},{"enabled":true,"time":"08:30","action":"reboot"}]}}"#,
        )
        .expect("解析不应失败");

        let sanitized = config.schedule.sanitize();
        assert_eq!(sanitized.entries.len(), 1, "空 time 的计划项应被丢弃");
        assert_eq!(sanitized.entries[0].time, "08:30");
    }

    #[test]
    fn full_app_config_parses_from_empty_object() {
        // 所有 14 个顶层段都缺失时也必须成功——这是「旧版本完全不认识新段」的场景
        let config: AppConfig = serde_json::from_str("{}").expect("空对象必须能解析");
        assert!(!config.mqtt.enabled);
        assert!(!config.restart.schedule_enabled);
        assert!(!config.net_health.enabled);
        assert!(config.call_control.numbers.is_empty());
    }

    #[test]
    fn unknown_future_fields_are_ignored_not_rejected() {
        // 向前兼容：更新的版本写出的字段，旧二进制读取时应忽略而非报错。
        // 若有人加上 #[serde(deny_unknown_fields)]，此测试会失败。
        let config: AppConfig = serde_json::from_str(
            r#"{"some_feature_from_v9":{"nested":true},"webhook":{"enabled":true}}"#,
        )
        .expect("未知字段必须被忽略");
        assert!(config.webhook.enabled);
    }

    #[test]
    fn corrupt_backup_path_appends_suffix_keeping_json_extension() {
        // 备份名要保留 .json，人工抢救时能直接看出原格式
        let backup = corrupt_backup_path(Path::new("/data/config.json"));
        assert_eq!(backup, PathBuf::from("/data/config.json.corrupt"));

        // 相对路径与无扩展名同样成立
        assert_eq!(
            corrupt_backup_path(Path::new("config.json")),
            PathBuf::from("config.json.corrupt")
        );
    }

    // === MQTT 节点 schema 迁移 ===

    #[test]
    fn mqtt_migrates_legacy_broker_list_to_nodes() {
        // 旧版 config.json：只有 broker_list + 全局 port/tls/active_broker，没有 nodes
        let legacy = r#"{
            "enabled": true,
            "broker_list": ["broker.emqx.io", "ssl://custom.example.com"],
            "active_broker": "ssl://custom.example.com",
            "port": 8883,
            "tls": true,
            "topic_sub": "cpe/{imei}/cmd",
            "topic_pub": "cpe/{imei}/status"
        }"#;
        let cfg: MqttConfig = serde_json::from_str(legacy).unwrap();

        assert!(cfg.enabled);
        // 两个旧节点都迁移过来了，且各自继承全局 port/tls
        assert_eq!(cfg.nodes.len(), 2);
        assert_eq!(cfg.nodes[0].host, "broker.emqx.io");
        assert_eq!(cfg.nodes[0].port, 8883);
        assert!(cfg.nodes[0].tls);
        assert_eq!(cfg.nodes[1].host, "ssl://custom.example.com");
        // active_broker 仍指向迁移后的同一节点（sanitize 不应丢掉用户选择）
        assert_eq!(cfg.active_broker, "ssl://custom.example.com");
    }

    #[test]
    fn mqtt_active_broker_inserted_when_missing_from_legacy_list() {
        // 历史脏数据：active_broker 不在 broker_list 里
        let legacy = r#"{
            "broker_list": ["broker.emqx.io"],
            "active_broker": "orphan.example.com",
            "port": 1883
        }"#;
        let cfg: MqttConfig = serde_json::from_str(legacy).unwrap();

        // 旧语义：把 active_broker 插到列表最前面，不丢失
        assert!(cfg.nodes.iter().any(|n| n.host == "orphan.example.com"));
        assert!(cfg.nodes.iter().any(|n| n.host == "broker.emqx.io"));
        assert_eq!(cfg.active_broker, "orphan.example.com");
    }

    #[test]
    fn mqtt_new_nodes_schema_deserializes_directly() {
        let json = r#"{
            "enabled": false,
            "nodes": [
                {"host": "a.example.com", "port": 1883, "tls": false},
                {"host": "ssl://b.example.com", "port": 8883, "tls": true}
            ],
            "topic_sub": "cmd/{imei}",
            "topic_pub": "status/{imei}"
        }"#;
        let cfg: MqttConfig = serde_json::from_str(json).unwrap();

        assert_eq!(cfg.nodes.len(), 2);
        assert_eq!(cfg.nodes[0].host, "a.example.com");
        assert_eq!(cfg.nodes[1].port, 8883);
        // 旧字段镜像被 sanitize 自动回写，供 OTA 回滚到旧二进制时读取
        assert_eq!(cfg.broker_list.len(), 2);
        assert!(cfg.broker_list[1].starts_with("ssl://"));
    }

    #[test]
    fn mqtt_nodes_schema_wins_over_legacy_fields() {
        // 同时存在 nodes 与 broker_list 时，以 nodes 为权威
        let json = r#"{
            "nodes": [{"host": "new.example.com", "port": 2883, "tls": false}],
            "broker_list": ["old.example.com"],
            "port": 1883
        }"#;
        let cfg: MqttConfig = serde_json::from_str(json).unwrap();

        assert_eq!(cfg.nodes.len(), 1);
        assert_eq!(cfg.nodes[0].host, "new.example.com");
        assert_eq!(cfg.nodes[0].port, 2883);
        // broker_list 镜像被回写为 nodes 的内容，不再是旧值
        assert_eq!(cfg.broker_list, vec!["new.example.com".to_string()]);
    }

    #[test]
    fn mqtt_empty_nodes_falls_back_to_defaults() {
        let json = r#"{"enabled": true, "nodes": []}"#;
        let cfg: MqttConfig = serde_json::from_str(json).unwrap();

        // sanitize 保证节点列表非空，否则连接循环会空转
        assert!(!cfg.nodes.is_empty());
        assert_eq!(cfg.nodes.len(), default_mqtt_nodes().len());
    }

    #[test]
    fn mqtt_data_wait_timeout_defaults_to_zero() {
        // 缺少字段时默认 0 = 始终等待数据连接（最安全），不会因缺字段而误降级直连
        let cfg: MqttConfig = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert_eq!(cfg.data_wait_timeout_secs, 0);

        let cfg: MqttConfig = serde_json::from_str(r#"{"enabled": true, "data_wait_timeout_secs": 300}"#).unwrap();
        assert_eq!(cfg.data_wait_timeout_secs, 300);
    }

    #[test]
    fn mqtt_sanitize_dedupes_and_drops_empty_hosts() {
        let cfg = MqttConfig {
            nodes: vec![
                MqttBrokerNode::new("a.example.com", 1883, false),
                MqttBrokerNode::new("a.example.com", 1883, false), // 完全重复
                MqttBrokerNode::new("   ", 1883, false),          // 空主机
                MqttBrokerNode::new("ssl://", 8883, true),        // 只有前缀
                MqttBrokerNode::new("b.example.com", 8883, true),
            ],
            ..MqttConfig::default()
        }
        .sanitize();

        assert_eq!(cfg.nodes.len(), 2, "应只剩 a 与 b: {:?}", cfg.nodes);
        assert_eq!(cfg.nodes[0].host, "a.example.com");
        assert_eq!(cfg.nodes[1].host, "b.example.com");
    }

    #[test]
    fn mqtt_sanitize_distinguishes_same_host_different_port() {
        // 同主机不同端口是两个不同节点（明文 1883 与 TLS 8883 并存很常见）
        let cfg = MqttConfig {
            nodes: vec![
                MqttBrokerNode::new("a.example.com", 1883, false),
                MqttBrokerNode::new("a.example.com", 8883, true),
            ],
            ..MqttConfig::default()
        }
        .sanitize();

        assert_eq!(cfg.nodes.len(), 2);
    }

    #[test]
    fn mqtt_sanitize_caps_node_count() {
        let many: Vec<MqttBrokerNode> = (0..(MAX_MQTT_NODES + 10))
            .map(|i| MqttBrokerNode::new(format!("b{i}.example.com"), 1883, false))
            .collect();
        let cfg = MqttConfig { nodes: many, ..MqttConfig::default() }.sanitize();

        assert_eq!(cfg.nodes.len(), MAX_MQTT_NODES);
    }

    #[test]
    fn mqtt_sanitize_preserves_valid_active_broker() {
        // 用户把 ★ 设在第二个节点上，sanitize 不应把它挪回第一个
        let cfg = MqttConfig {
            nodes: vec![
                MqttBrokerNode::new("a.example.com", 1883, false),
                MqttBrokerNode::new("b.example.com", 1883, false),
            ],
            active_broker: "b.example.com".to_string(),
            ..MqttConfig::default()
        }
        .sanitize();

        assert_eq!(cfg.active_broker, "b.example.com");
        // 端口/TLS 镜像也应跟随激活节点
        assert_eq!(cfg.port, 1883);
    }

    #[test]
    fn mqtt_sanitize_resets_active_broker_when_stale() {
        // active_broker 指向已被删除的节点时回落到首节点
        let cfg = MqttConfig {
            nodes: vec![MqttBrokerNode::new("a.example.com", 1883, false)],
            active_broker: "deleted.example.com".to_string(),
            ..MqttConfig::default()
        }
        .sanitize();

        assert_eq!(cfg.active_broker, "a.example.com");
    }

    #[test]
    fn mqtt_sanitize_drops_password_without_username() {
        // 没有用户名时密码无意义（连接代码以 username 为开关），避免误存
        let cfg = MqttConfig {
            username: None,
            password: Some("orphan-pass".to_string()),
            ..MqttConfig::default()
        }
        .sanitize();

        assert!(cfg.username.is_none());
        assert!(cfg.password.is_none());
    }

    #[test]
    fn mqtt_sanitize_trims_blank_credentials_to_none() {
        let cfg = MqttConfig {
            username: Some("   ".to_string()),
            password: Some("secret".to_string()),
            auth_token: Some("  tok  ".to_string()),
            ..MqttConfig::default()
        }
        .sanitize();

        // 空白用户名视为未填，连带清掉密码
        assert!(cfg.username.is_none());
        assert!(cfg.password.is_none());
        // token 只去首尾空白，内容保留
        assert_eq!(cfg.auth_token.as_deref(), Some("tok"));
    }

    #[test]
    fn mqtt_sanitize_restores_empty_topics() {
        let cfg = MqttConfig {
            topic_sub: "   ".to_string(),
            topic_pub: String::new(),
            ..MqttConfig::default()
        }
        .sanitize();

        // 空主题会让订阅/发布静默失效，必须回落到默认值
        assert_eq!(cfg.topic_sub, default_topic_sub());
        assert_eq!(cfg.topic_pub, default_topic_pub());
    }

    #[test]
    fn mqtt_broker_node_resolve_strips_prefixes() {
        // rumqttc 只接受纯主机名，前缀必须剥掉，否则 DNS 解析必然失败
        let (host, tls) = MqttBrokerNode::new("ssl://a.example.com", 8883, false).resolve();
        assert_eq!(host, "a.example.com");
        assert!(tls, "ssl:// 前缀应强制启用 TLS");

        let (host, tls) = MqttBrokerNode::new("tls://b.example.com", 8883, false).resolve();
        assert_eq!(host, "b.example.com");
        assert!(tls);

        // mqtt:// 显式表示明文，即使 tls 字段为 true
        let (host, tls) = MqttBrokerNode::new("mqtt://c.example.com", 1883, true).resolve();
        assert_eq!(host, "c.example.com");
        assert!(!tls, "mqtt:// 前缀应强制明文");

        // 无前缀时回落到 tls 字段
        let (_, tls) = MqttBrokerNode::new("d.example.com", 1883, true).resolve();
        assert!(tls);
    }

    #[test]
    fn mqtt_broker_node_resolve_is_case_insensitive_on_prefix() {
        // 用户可能粘贴大写前缀
        let (host, tls) = MqttBrokerNode::new("SSL://MixedCase.Example.COM", 8883, false).resolve();
        assert_eq!(host, "MixedCase.Example.COM", "应保留主机原始大小写");
        assert!(tls);
    }

    #[test]
    fn mqtt_broker_node_effective_port_defaults_by_tls() {
        // port=0 表示「未填写」，按 TLS 与否取默认端口
        assert_eq!(MqttBrokerNode::new("a.com", 0, false).effective_port(), 1883);
        assert_eq!(MqttBrokerNode::new("a.com", 0, true).effective_port(), 8883);
        // ssl:// 前缀同样决定默认端口
        assert_eq!(MqttBrokerNode::new("ssl://a.com", 0, false).effective_port(), 8883);
        // 显式端口优先
        assert_eq!(MqttBrokerNode::new("ssl://a.com", 1234, false).effective_port(), 1234);
    }

    #[test]
    fn mqtt_broker_node_display_roundtrips_tls() {
        // display() 的结果要能被 resolve() 正确读回，否则 ★ 激活项匹配会失效
        let tls_node = MqttBrokerNode::new("ssl://a.example.com", 8883, true);
        assert_eq!(tls_node.display(), "ssl://a.example.com");
        assert_eq!(tls_node.resolve().0, "a.example.com");

        let plain_node = MqttBrokerNode::new("b.example.com", 1883, false);
        assert_eq!(plain_node.display(), "b.example.com");
    }

    #[test]
    fn mqtt_malformed_node_does_not_fail_parse() {
        // 关键安全性质：单个节点字段缺失不应让整个 MqttConfig 解析失败。
        // ConfigManager 在解析失败时会回落到整个 AppConfig::default()，
        // 那会清空用户所有配置——所以这里必须容错。
        let json = r#"{
            "nodes": [{"host": "a.example.com"}, {"port": 1883}],
            "enabled": true
        }"#;
        let cfg: MqttConfig = serde_json::from_str(json).unwrap();

        // 缺 port/tls 的节点取默认；缺 host 的节点被 sanitize 丢弃
        assert!(cfg.enabled);
        assert_eq!(cfg.nodes.len(), 1);
        assert_eq!(cfg.nodes[0].host, "a.example.com");
        assert_eq!(cfg.nodes[0].port, 0, "未填端口应为 0（连接时取默认）");
    }

    #[test]
    fn mqtt_unknown_fields_are_ignored() {
        // 未来版本新增字段时，旧二进制读新配置不应失败（反之亦然）
        let json = r#"{
            "enabled": true,
            "nodes": [{"host": "a.example.com", "port": 1883, "tls": false}],
            "some_future_field": {"nested": true},
            "another_unknown": 42
        }"#;
        let cfg: MqttConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.nodes.len(), 1);
    }

    #[test]
    fn mqtt_full_app_config_survives_partial_mqtt_section() {
        // 端到端验证：config.json 里 mqtt 段残缺时，其它段（如 webhook）不受影响。
        // 这是「解析失败会清空全部配置」风险的实际防线。
        let json = r#"{
            "webhook": {"enabled": true, "url": "https://example.com/hook", "forward_sms": false, "forward_calls": false},
            "mqtt": {"enabled": true, "broker_list": ["legacy.example.com"], "port": 2883}
        }"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();

        // webhook 段完好
        assert!(cfg.webhook.enabled);
        assert_eq!(cfg.webhook.url, "https://example.com/hook");
        // mqtt 段从旧 schema 迁移成功
        assert!(cfg.mqtt.enabled);
        assert_eq!(cfg.mqtt.nodes.len(), 1);
        assert_eq!(cfg.mqtt.nodes[0].host, "legacy.example.com");
        assert_eq!(cfg.mqtt.nodes[0].port, 2883);
    }
}
