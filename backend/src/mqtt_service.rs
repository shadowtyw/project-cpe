use crate::config::{ConfigManager, MqttConfig};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};
use zbus::Connection;

// ── 通知发送器 trait（由 main.rs 注入） ──────────────────────

/// MQTT 远程遥控通知回调：接收 JSON 字符串推送到 Webhook 和短信推送平台。
pub trait MqttNotifier: Send + Sync {
    fn notify(&self, json_payload: &str);
}

/// 全局通知器，由 main.rs 在启动时设置一次。
static NOTIFIER: std::sync::OnceLock<Arc<dyn MqttNotifier>> = std::sync::OnceLock::new();

pub fn set_notifier(n: Arc<dyn MqttNotifier>) {
    let _ = NOTIFIER.set(n);
}

fn notify(payload: &serde_json::Value) {
    if let Some(n) = NOTIFIER.get() {
        if let Ok(json) = serde_json::to_string(payload) {
            n.notify(&json);
        }
    }
}

fn send_mqtt_notification(event: &str, command: &str, message: &str) {
    let ts = chrono::Utc::now().to_rfc3339();
    let msg = serde_json::json!({
        "timestamp": ts,
        "type": "mqtt_control",
        "data": {
            "event": event,
            "command": command,
            "message": message,
        },
    });
    notify(&msg);
}

/// MQTT 运行时状态
#[derive(Debug, Clone, Serialize)]
pub struct MqttRuntimeState {
    pub connected: bool,
    pub current_broker: String,
    pub last_heartbeat: Option<String>,
    pub last_command: Option<String>,
    pub error_message: Option<String>,
    pub broker_index: usize,
}

impl Default for MqttRuntimeState {
    fn default() -> Self {
        Self {
            connected: false,
            current_broker: String::new(),
            last_heartbeat: None,
            last_command: None,
            error_message: None,
            broker_index: 0,
        }
    }
}

/// 全局运行时状态
static MQTT_STATE: Mutex<Option<MqttRuntimeState>> = Mutex::const_new(None);
static MQTT_ENABLED: AtomicBool = AtomicBool::new(false);

/// 已连接的 MQTT 客户端（用于发布消息，避免每次创建临时客户端）
static MQTT_CLIENT: Mutex<Option<(AsyncClient, String)>> = Mutex::const_new(None);

/// MQTT 指令
#[derive(Debug, Deserialize)]
struct MqttCommand {
    action: String,
    token: Option<String>,
}

/// 系统状态快照（发布到 topic_pub）
#[derive(Debug, Serialize)]
struct SystemStatus {
    timestamp: String,
    signal_strength: Option<serde_json::Value>,
    memory: Option<serde_json::Value>,
    uptime: Option<serde_json::Value>,
    thermal: serde_json::Value,
}

/// MQTT 服务
pub struct MqttService {
    config_manager: Arc<ConfigManager>,
    dbus_conn: Arc<Connection>,
    imei: String,
}

impl MqttService {
    pub fn new(config_manager: Arc<ConfigManager>, dbus_conn: Arc<Connection>, imei: String) -> Self {
        Self {
            config_manager,
            dbus_conn,
            imei,
        }
    }

    /// 启动 MQTT 服务主循环
    pub async fn run(&self) {
        info!("MQTT service started");

        loop {
            let config = self.config_manager.get_mqtt().sanitize();

            if !config.enabled {
                MQTT_ENABLED.store(false, Ordering::SeqCst);
                sleep(Duration::from_secs(5)).await;
                continue;
            }

            MQTT_ENABLED.store(true, Ordering::SeqCst);

            let err_str = match self.connect_and_run(&config).await {
                Ok(()) => None,
                Err(e) => {
                    error!(error = %e, "MQTT service error");
                    Some(e.to_string())
                }
            };
            if let Some(msg) = err_str {
                Self::update_state_static(|state| {
                    state.connected = false;
                    state.error_message = Some(msg);
                }).await;
            }

            // 重连间隔
            sleep(Duration::from_secs(5)).await;
        }
    }

    async fn connect_and_run(&self, config: &MqttConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let broker_list = &config.broker_list;
        let mut broker_index = 0;
        let mut consecutive_failures: u32 = 0;

        // 从当前 active_broker 开始
        if let Some(idx) = broker_list.iter().position(|b| b == &config.active_broker) {
            broker_index = idx;
        }

        loop {
            if broker_index >= broker_list.len() {
                broker_index = 0;
            }

            let broker = &broker_list[broker_index];
            let client_id = format!("udx710_{}", self.imei);

            info!("Connecting to MQTT broker: {} client_id={}", broker, client_id);

            Self::update_state_static(|state| {
                state.current_broker = broker.clone();
                state.broker_index = broker_index;
                state.connected = false;
                state.error_message = None;
            }).await;

            match self.connect_broker(broker, &client_id, config).await {
                Ok(()) => {
                    // 成功连接并正常运行直到断开，重置退避
                    consecutive_failures = 0;
                    info!(broker = %broker, "MQTT connection closed gracefully, trying next broker");
                    broker_index += 1;
                }
                Err(e) => {
                    let err_msg = format!("{}: {}", broker, e);
                    error!(broker = %broker, error = %err_msg, "Failed to connect to broker");
                    Self::update_state_static(|state| {
                        state.error_message = Some(err_msg);
                    }).await;

                    consecutive_failures += 1;
                    broker_index += 1;

                    // 所有节点都试过且全部失败 → 指数退避
                    if broker_index >= broker_list.len() {
                        let wait = min(Duration::from_secs(3)
                            .saturating_mul(2u32.saturating_pow(consecutive_failures.min(5))),
                            Duration::from_secs(60));
                        warn!(
                            successive_failures = consecutive_failures,
                            backoff_secs = wait.as_secs(),
                            "All MQTT brokers unreachable, backing off"
                        );
                        sleep(wait).await;
                    } else {
                        // 当前节点失败但还有剩余节点，快速尝试下一个
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }
    }

    async fn connect_broker(
        &self,
        broker: &str,
        client_id: &str,
        config: &MqttConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // 清除上一个连接的客户端，避免在新连接建立期间误用旧连接发布
        MQTT_CLIENT.lock().await.take();

        // 解析 ssl:// 前缀 → 剥掉前缀，启用 TLS
        let (host, use_tls) = if let Some(host) = broker.strip_prefix("ssl://") {
            (host, true)
        } else {
            (broker, config.tls)
        };

        let port = config.port;
        let mut mqttoptions = MqttOptions::new(client_id, host, port);
        mqttoptions.set_keep_alive(Duration::from_secs(60));
        mqttoptions.set_clean_session(true);

        // TLS
        if use_tls {
            mqttoptions.set_transport(Transport::tls_with_default_config());
            info!("MQTT TLS enabled for broker: {} (system CA certs)", host);
        }

        // 用户名密码认证
        if let Some(ref user) = config.username {
            let pass = config.password.as_deref().unwrap_or("");
            mqttoptions.set_credentials(user, pass);
            info!("MQTT credentials set user={}", user);
        }

        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

        // 订阅指令主题
        let topic_sub = config.topic_sub.replace("{imei}", &self.imei);
        client.subscribe(&topic_sub, QoS::AtLeastOnce).await?;
        info!(topic = %topic_sub, "Subscribed to command topic");

        // 计算发布主题
        let topic_pub = config.topic_pub.replace("{imei}", &self.imei);

        Self::update_state_static(|state| {
            state.connected = true;
            state.error_message = None;
        }).await;

        // 连接阶段：等待 ConnAck，超时则快速失败以切换下一个节点
        let connect_deadline = Duration::from_secs(8);
        let conn_ack = async {
            // 可能先收到其它包（如 SubAck），循环直到 ConnAck 或超时
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok::<(), String>(()),
                    Ok(_) => continue,
                    Err(e) => return Err(format!("{broker}: {e}")),
                }
            }
        };
        match timeout(connect_deadline, conn_ack).await {
            Ok(Ok(())) => {
                info!(broker = %broker, "MQTT connected");
                Self::update_state_static(|state| {
                    state.connected = true;
                }).await;
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(format!("connect timeout to {broker}").into()),
        }

        // 连接成功后存储客户端以供 publish 复用
        MQTT_CLIENT.lock().await.replace((client.clone(), topic_pub.clone()));

        // 发布初始上线状态
        send_mqtt_notification(
            "mqtt_connected",
            "connect",
            &format!("MQTT 已连接至 {}", broker),
        );
        {
            let dbus_clone = Arc::clone(&self.dbus_conn);
            tokio::spawn(async move {
                publish_status(&dbus_clone).await;
            });
        }

        // 事件循环：保持连接，处理下行指令 + 周期性心跳。
        //
        // 关键设计：
        // 1. 心跳定时器集成在事件循环内（tokio::select!），不再 spawn 独立 task。
        //    之前每次 connect_broker() 都会 spawn 心跳 task，断连重连后旧 task 未退出
        //    导致多个心跳 task 叠加，状态发布频率翻倍。
        // 2. 收到的 Publish 消息 spawn 到独立 task 处理，因为 handle_command 里的
        //    publish 需要 EventLoop 轮询来完成网络 I/O，不能在 poll 回调中 await。
        // 3. 过滤 topic_pub 上的自回环：如果 topic_pub == topic_sub，忽略自己发布的消息。
        let heartbeat_interval = Duration::from_secs(300);
        let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
        // 第一次 tick 立即触发，跳过它
        heartbeat_timer.tick().await;

        let dbus_conn = Arc::clone(&self.dbus_conn);
        let config_clone = config.clone();
        loop {
            tokio::select! {
                event = eventloop.poll() => {
                    match event {
                        Ok(Event::Incoming(Packet::Publish(publish))) => {
                            // 过滤自回环：忽略自己发布到 topic_pub 的状态消息
                            if publish.topic == topic_pub {
                                continue;
                            }
                            if publish.topic == topic_sub {
                                let payload = publish.payload;
                                let conn = Arc::clone(&dbus_conn);
                                let cfg = config_clone.clone();
                                tokio::spawn(async move {
                                    handle_command_spawned(&payload, &cfg, &conn).await;
                                });
                            }
                        }
                        Ok(Event::Incoming(Packet::Disconnect)) => {
                            warn!(broker = %broker, "MQTT disconnected by broker");
                            MQTT_CLIENT.lock().await.take();
                            break;
                        }
                        Ok(Event::Incoming(Packet::ConnAck(_))) => {
                            // 重连后的 ConnAck，忽略
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let err_msg = e.to_string();
                            error!(broker = %broker, error = %err_msg, "MQTT event loop error");
                            MQTT_CLIENT.lock().await.take();
                            Self::update_state_static(|state| {
                                state.connected = false;
                                state.error_message = Some(err_msg.clone());
                            }).await;
                            return Err(format!("{broker}: {err_msg}").into());
                        }
                    }
                }
                _ = heartbeat_timer.tick() => {
                    let dbus_clone = Arc::clone(&self.dbus_conn);
                    tokio::spawn(async move {
                        publish_status(&dbus_clone).await;
                    });
                }
            }
        }

        Ok(())
    }

    async fn update_state_static<F>(f: F)
    where
        F: FnOnce(&mut MqttRuntimeState),
    {
        let mut state_guard = MQTT_STATE.lock().await;
        let state = state_guard.get_or_insert_with(MqttRuntimeState::default);
        f(state);
    }
}

/// 在独立 task 中处理 MQTT 指令（由 EventLoop spawn 调用）。
///
/// 此函数不能 running 在与 EventLoop 相同的 task 中，否则 publish_status 里
/// client.publish() 会死锁（publish 依赖 EventLoop 轮询来完成网络 I/O）。
async fn handle_command_spawned(payload: &[u8], config: &MqttConfig, dbus_conn: &Connection) {
    let payload_str = match std::str::from_utf8(payload) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Invalid UTF-8 in MQTT payload");
            return;
        }
    };

    debug!(payload = %payload_str, "Received MQTT command");

    let cmd: MqttCommand = match serde_json::from_str(payload_str) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to parse MQTT command");
            return;
        }
    };

    // Token 校验
    if let Some(ref expected_token) = config.auth_token {
        match &cmd.token {
            Some(token) if token == expected_token => {}
            _ => {
                error!("Invalid or missing token, ignoring command");
                return;
            }
        }
    }

    MqttService::update_state_static(|state| {
        state.last_command = Some(chrono::Utc::now().to_rfc3339());
    }).await;

    info!(action = %cmd.action, "Executing MQTT command");

    match cmd.action.as_str() {
        "reboot" => {
            send_mqtt_notification(
                "mqtt_command_executed",
                "reboot",
                "MQTT 远程遥控：收到重启指令，设备将在 10 秒后重启",
            );
            crate::restart::schedule_reboot("mqtt", 10);
        }
        "reconnect" => {
            send_mqtt_notification(
                "mqtt_command_executed",
                "reconnect",
                "MQTT 远程遥控：收到重置数据连接指令，正在执行…",
            );
            if let Err(e) = crate::dbus::set_data_connection(dbus_conn, false).await {
                error!(error = %e, "Failed to disconnect data");
            } else if let Err(e) = crate::dbus::set_data_connection(dbus_conn, true).await {
                error!(error = %e, "Failed to reconnect data");
            }
        }
        "status" => {
            // 先收集状态，再发一条含数据的推送（而不是两条分开的推送）
            let status = collect_system_status(dbus_conn).await;
            let summary = format_status_summary(&status);
            send_mqtt_notification(
                "mqtt_command_executed",
                "status",
                &summary,
            );
            // 同时发布完整 JSON 到 MQTT topic_pub 供外部系统消费
            publish_status(dbus_conn).await;
        }
        _ => {
            warn!(action = %cmd.action, "Unknown MQTT command");
        }
    }
}

/// 发布系统状态到 MQTT（独立函数，不依赖 MqttService 实例）。
async fn publish_status(dbus_conn: &Connection) {
    let status = collect_system_status(dbus_conn).await;
    let payload = match serde_json::to_string(&status) {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "Failed to serialize status");
            return;
        }
    };

    // 复用当前已连接的客户端发布，避免用可能已过期的 active_broker 重新连一个新临时客户端
    let (client, topic_pub) = match MQTT_CLIENT.lock().await.clone() {
        Some(entry) => entry,
        None => {
            error!("No active MQTT client, cannot publish status");
            return;
        }
    };

    match client.publish(&topic_pub, QoS::AtLeastOnce, false, payload.as_bytes()).await {
        Ok(_) => {
            info!(topic = %topic_pub, "Published status");
            MqttService::update_state_static(|state| {
                state.last_heartbeat = Some(chrono::Utc::now().to_rfc3339());
            }).await;
        }
        Err(e) => {
            error!(error = %e, "Failed to publish status");
        }
    }
}

/// 收集系统状态快照。
async fn collect_system_status(dbus_conn: &Connection) -> SystemStatus {
    // 信号强度 (异步函数)
    let signal_strength = crate::dbus::get_signal_strength(dbus_conn)
        .await
        .ok()
        .and_then(|s| serde_json::to_value(s).ok());

    // 内存 (同步函数)
    let memory = crate::utils::read_memory_info()
        .ok()
        .and_then(|m| serde_json::to_value(m).ok());

    // 运行时长 (同步函数)
    let uptime = crate::utils::read_uptime()
        .ok()
        .and_then(|u| serde_json::to_value(u).ok());

    // 温度 (同步函数)
    let thermal = serde_json::to_value(crate::handlers::read_temperature_sensors()).unwrap_or_default();

    SystemStatus {
        timestamp: chrono::Utc::now().to_rfc3339(),
        signal_strength,
        memory,
        uptime,
        thermal,
    }
}

/// 将系统状态格式化为中文可读摘要（用于 WeCom 推送）
fn format_status_summary(status: &SystemStatus) -> String {
    let mut lines = Vec::new();
    lines.push("📊 设备状态报告".to_string());

    // 信号强度
    let sig = status.signal_strength.as_ref()
        .and_then(|v| v["strength"].as_i64());
    let sig_str = match sig {
        Some(s) if s >= 80 => format!("📶 信号: {}% █████ (极好)", s),
        Some(s) if s >= 60 => format!("📶 信号: {}% ████ (良好)", s),
        Some(s) if s >= 40 => format!("📶 信号: {}% ███ (一般)", s),
        Some(s) if s >= 20 => format!("📶 信号: {}% ██ (较弱)", s),
        Some(s) => format!("📶 信号: {}% █ (弱)", s),
        None => "📶 信号: 未知".to_string(),
    };
    lines.push(sig_str);

    // 内存
    if let Some(mem) = &status.memory {
        let total = mem["total_bytes"].as_u64().unwrap_or(0);
        let avail = mem["available_bytes"].as_u64().unwrap_or(0);
        let used = total.saturating_sub(avail);
        let pct = mem["used_percent"].as_f64().unwrap_or(0.0);
        if total > 0 {
            let total_mb = total / 1024 / 1024;
            let used_mb = used / 1024 / 1024;
            lines.push(format!(
                "💾 内存: {:.0}% (已用 {:.0}MB / 总计 {:.0}MB)",
                pct, used_mb, total_mb
            ));
        }
    }

    // 温度
    if let Some(arr) = status.thermal.as_array() {
        for tz in arr {
            if let (Some(t), Some(name)) = (
                tz["temperature"].as_f64(),
                tz["sensor_type"].as_str(),
            ) {
                let emoji = if t >= 75.0 { "🔥" } else if t >= 55.0 { "🌡️" } else { "❄️" };
                lines.push(format!("{} {}: {:.1}°C", emoji, name, t));
            }
        }
    }

    // 运行时长
    if let Some(up) = &status.uptime {
        if let Some(secs) = up.as_array().and_then(|a| a.first()?.as_u64()) {
            let days = secs / 86400;
            let hours = (secs % 86400) / 3600;
            let mins = (secs % 3600) / 60;
            if days > 0 {
                lines.push(format!("⏱️ 已运行: {}天{}小时{}分钟", days, hours, mins));
            } else {
                lines.push(format!("⏱️ 已运行: {}小时{}分钟", hours, mins));
            }
        }
    }

    lines.push(format!("🕐 {}", status.timestamp));
    lines.join("\n")
}

/// 获取 MQTT 运行时状态
pub async fn get_mqtt_status() -> MqttRuntimeState {
    let state_guard = MQTT_STATE.lock().await;
    state_guard.clone().unwrap_or_default()
}

/// 检查 MQTT 是否启用
pub fn is_mqtt_enabled() -> bool {
    MQTT_ENABLED.load(Ordering::SeqCst)
}

/// 检查 MQTT 是否已连接
pub async fn is_mqtt_connected() -> bool {
    let state_guard = MQTT_STATE.lock().await;
    state_guard.as_ref().map_or(false, |s| s.connected)
}

