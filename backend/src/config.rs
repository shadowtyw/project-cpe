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
    pub enabled: bool,
    pub url: String,
    pub forward_sms: bool,
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
    pub time: String,
    /// 周几触发（0=周日 … 6=周六），空表示每天
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// 要执行的命令
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

/// 通话远程控制配置。默认关闭；启用后，指定号码来电自动接听，接通后
/// 保持通话达到 `hold_seconds` 秒即执行配置的动作（默认重启）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallControlConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 白名单号码。来电号码经规范化（去空格/连字符）后，只要以任一白名单号码结尾即匹配，
    /// 从而兼容来电显示带 "+86" 或国家码前缀的情况。
    #[serde(default)]
    pub numbers: Vec<String>,
    /// 接通后需要保持通话的秒数，达到后执行动作。
    #[serde(default = "default_call_hold_seconds")]
    pub hold_seconds: u64,
    /// 达到保持时长后执行的动作。
    #[serde(default = "default_call_action")]
    pub action: ScheduleAction,
}

fn default_call_hold_seconds() -> u64 {
    15
}

fn default_call_action() -> ScheduleAction {
    ScheduleAction::Reboot
}

impl Default for CallControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            numbers: Vec::new(),
            hold_seconds: default_call_hold_seconds(),
            action: default_call_action(),
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
        self.hold_seconds = self.hold_seconds.clamp(5, 3600);
        // 号码去空白、去连字符，过滤空项并去重。
        let mut seen = std::collections::HashSet::new();
        self.numbers = self
            .numbers
            .into_iter()
            .map(|number| normalize_phone_number(&number))
            .filter(|number| !number.is_empty() && seen.insert(number.clone()))
            .collect();
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
}


/// 配置管理器
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
                            ..cfg
                        },
                        Err(e) => {
                            warn!(error = %e, "Failed to parse config file, using defaults");
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
    
    /// 保存配置到文件（原子写入：先写临时文件，再 rename 替换）
    pub fn save(&self) -> Result<(), String> {
        let config = self.config.read().unwrap_or_else(|p| p.into_inner());
        let content = serde_json::to_string_pretty(&*config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // 确保目录存在
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        // 原子写入：先写临时文件，再 rename 替换。
        // 避免断电/进程崩溃导致配置文件被截断或损坏。
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

fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .map_err(|e| format!("Failed to read metadata for {}: {}", path.display(), e))?
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)
            .map_err(|e| format!("Failed to protect {}: {}", path.display(), e))?;
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

fn set_executable_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .map_err(|e| format!("Failed to read metadata for {}: {}", path.display(), e))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|e| format!("Failed to set permissions for {}: {}", path.display(), e))?;
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
        loader_contains_init_command,
        loader_contains_ota_command,
        remove_ota_command_from_loader,
        AppConfig,
        RestartConfig,
        INIT_SCRIPT_LOADER_COMMAND,
    };

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
}
