use crate::config::{ConfigManager, MqttConfig};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use zbus::Connection;

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
            let config = self.config_manager.get_mqtt();

            if !config.enabled {
                MQTT_ENABLED.store(false, Ordering::SeqCst);
                sleep(Duration::from_secs(5)).await;
                continue;
            }

            MQTT_ENABLED.store(true, Ordering::SeqCst);

            if let Err(e) = self.connect_and_run(&config).await {
                error!(error = %e, "MQTT service error");
                self.update_state(|state| {
                    state.connected = false;
                    state.error_message = Some(e.to_string());
                }).await;
            }

            // 重连间隔
            sleep(Duration::from_secs(5)).await;
        }
    }

    async fn connect_and_run(&self, config: &MqttConfig) -> Result<(), Box<dyn std::error::Error>> {
        let broker_list = &config.broker_list;
        let mut broker_index = 0;

        // 从当前 active_broker 开始（如果还在列表中的话）
        if let Some(idx) = broker_list.iter().position(|b| b == &config.active_broker) {
            broker_index = idx;
        }

        loop {
            if broker_index >= broker_list.len() {
                broker_index = 0;
                warn!("All brokers failed, retrying from start");
                sleep(Duration::from_secs(10)).await;
            }

            let broker = &broker_list[broker_index];
            let client_id = format!("udx710_{}", self.imei);

            info!(broker = %broker, client_id = %client_id, "Connecting to MQTT broker");

            self.update_state(|state| {
                state.current_broker = broker.clone();
                state.broker_index = broker_index;
                state.connected = false;
                state.error_message = None;
            }).await;

            match self.connect_broker(broker, &client_id, config).await {
                Ok(_) => {
                    info!(broker = %broker, "MQTT connection closed, trying next broker");
                    broker_index += 1;
                }
                Err(e) => {
                    error!(broker = %broker, error = %e, "Failed to connect to broker");
                    self.update_state(|state| {
                        state.error_message = Some(format!("{}: {}", broker, e));
                    }).await;
                    broker_index += 1;
                    sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }

    async fn connect_broker(
        &self,
        broker: &str,
        client_id: &str,
        config: &MqttConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut mqttoptions = MqttOptions::new(client_id, broker, config.port);
        mqttoptions.set_keep_alive(Duration::from_secs(60));
        mqttoptions.set_clean_session(true);

        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

        // 订阅指令主题
        let topic_sub = config.topic_sub.replace("{imei}", &self.imei);
        client.subscribe(&topic_sub, QoS::AtLeastOnce).await?;
        info!(topic = %topic_sub, "Subscribed to command topic");

        self.update_state(|state| {
            state.connected = true;
            state.error_message = None;
        }).await;

        // 事件循环
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    info!("MQTT connected to {}", broker);
                    self.update_state(|state| {
                        state.connected = true;
                    }).await;
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    if publish.topic == topic_sub {
                        self.handle_command(&publish.payload, config).await;
                    }
                }
                Ok(Event::Incoming(Packet::Disconnect)) => {
                    warn!("MQTT disconnected by broker");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "MQTT event loop error");
                    self.update_state(|state| {
                        state.connected = false;
                        state.error_message = Some(e.to_string());
                    }).await;
                    return Err(Box::new(e));
                }
            }
        }

        Ok(())
    }

    async fn handle_command(&self, payload: &[u8], config: &MqttConfig) {
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

        self.update_state(|state| {
            state.last_command = Some(chrono::Utc::now().to_rfc3339());
        }).await;

        info!(action = %cmd.action, "Executing MQTT command");

        match cmd.action.as_str() {
            "reboot" => {
                crate::restart::schedule_reboot("mqtt", 3);
            }
            "reconnect" => {
                if let Err(e) = crate::dbus::set_data_connection(&self.dbus_conn, false).await {
                    error!(error = %e, "Failed to disconnect data");
                } else if let Err(e) = crate::dbus::set_data_connection(&self.dbus_conn, true).await {
                    error!(error = %e, "Failed to reconnect data");
                }
            }
            "status" => {
                self.publish_status(config).await;
            }
            _ => {
                warn!(action = %cmd.action, "Unknown MQTT command");
            }
        }
    }

    async fn publish_status(&self, config: &MqttConfig) {
        let status = self.collect_system_status().await;
        let payload = match serde_json::to_string(&status) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to serialize status");
                return;
            }
        };

        let topic_pub = config.topic_pub.replace("{imei}", &self.imei);

        // 创建临时客户端发布消息
        let pub_client_id = format!("udx710_pub_{}", self.imei);
        let mut mqttoptions = MqttOptions::new(&pub_client_id, &config.active_broker, config.port);
        mqttoptions.set_keep_alive(Duration::from_secs(60));
        mqttoptions.set_clean_session(true);

        let (client, _eventloop) = AsyncClient::new(mqttoptions, 10);

        match client.publish(&topic_pub, QoS::AtLeastOnce, false, payload.as_bytes()).await {
            Ok(_) => {
                info!(topic = %topic_pub, "Published status");
                self.update_state(|state| {
                    state.last_heartbeat = Some(chrono::Utc::now().to_rfc3339());
                }).await;
            }
            Err(e) => {
                error!(error = %e, "Failed to publish status");
            }
        }
    }

    async fn collect_system_status(&self) -> SystemStatus {
        // 信号强度 (异步函数)
        let signal_strength = crate::dbus::get_signal_strength(&self.dbus_conn)
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

    async fn update_state<F>(&self, f: F)
    where
        F: FnOnce(&mut MqttRuntimeState),
    {
        let mut state_guard = MQTT_STATE.lock().await;
        let state = state_guard.get_or_insert_with(MqttRuntimeState::default);
        f(state);
    }
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