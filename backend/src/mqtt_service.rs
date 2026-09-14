use crate::config::{ConfigManager, MqttBrokerNode, MqttConfig};
use crate::device_report::DeviceReport;
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

/// MQTT 模块在前端日志里的独立模块名。
///
/// `LogBufferLayer` 默认从 `target` 取末段（`udx710::mqtt_service` → `mqtt_service`），
/// 但本模块的关键事件通过 `log_entry!` 显式写入 `"mqtt"`，使前端 MQTT 页可以只筛选
/// 这一类日志，与系统日志页的全量视图区分开。
const LOG_MODULE: &str = "mqtt";

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
    /// 配置里是否启用（关闭后 UI 应显示「已停用」而非「未连接」）
    pub enabled: bool,
    /// 是否已收到 ConnAck 并处于连接中
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
            enabled: false,
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
    ///
    /// 关键修复：关闭开关后必须**立即**清理连接状态。旧实现里 `connect_and_run`
    /// 是个永不返回的循环，一旦连上就再也走不回这里的 `!enabled` 分支，导致
    /// 用户在页面上关掉 MQTT 后，状态卡仍然显示「已连接」——直到进程重启。
    ///
    /// 现在 `connect_and_run` 与 `connect_broker` 都会周期性重读 `config.enabled`，
    /// 一旦被关闭就主动断开、清空 client 与 connected 状态并返回。
    pub async fn run(&self) {
        info!("MQTT service started");
        crate::log_entry!(info, LOG_MODULE, "MQTT 服务已启动");

        loop {
            let config = self.config_manager.get_mqtt().sanitize();

            // 配置态始终同步到全局，使 /api/health 与状态卡能区分「已停用」与「未连接」
            MQTT_ENABLED.store(config.enabled, Ordering::SeqCst);

            if !config.enabled {
                // 关闭时彻底清理，避免任何残留的「已连接」状态
                Self::teardown("配置已关闭").await;
                Self::update_state_static(|state| {
                    state.enabled = false;
                }).await;
                sleep(Duration::from_secs(5)).await;
                continue;
            }

            Self::update_state_static(|state| {
                state.enabled = true;
            }).await;

            // connect_and_run 会在「被关闭」或「所有节点失败」时返回。
            // 返回后回到循环顶部重读配置，因此关闭开关能在一轮内生效。
            self.connect_and_run(&config).await;

            // 重连间隔
            sleep(Duration::from_secs(5)).await;
        }
    }

    /// 清理连接：丢弃 client、清除 connected 与错误信息。
    ///
    /// 关闭开关、断开、出错三条路径共用，保证状态一致。
    async fn teardown(reason: &str) {
        MQTT_CLIENT.lock().await.take();
        Self::update_state_static(|state| {
            state.connected = false;
            state.error_message = None;
            state.current_broker = String::new();
        }).await;
        debug!("MQTT torn down: {reason}");
    }

    /// 遍历所有 broker 节点，连接并维持，直到被关闭或全部节点失败。
    async fn connect_and_run(&self, config: &MqttConfig) {
        let nodes = &config.nodes;
        let mut broker_index = 0usize;
        let mut consecutive_failures: u32 = 0;

        // 从 active_broker 对应的节点开始
        if let Some(idx) = nodes.iter().position(|n| n.display() == config.active_broker) {
            broker_index = idx;
        }

        loop {
            // 每轮重读 enabled：用户在连接期间关闭开关，这里能立刻退出
            if !self.config_manager.get_mqtt().enabled {
                Self::teardown("配置已关闭").await;
                crate::log_entry!(info, LOG_MODULE, "MQTT 已停用，断开连接");
                return;
            }

            if nodes.is_empty() {
                // sanitize 保证非空，这里是防御性分支
                return;
            }
            if broker_index >= nodes.len() {
                broker_index = 0;
            }

            let node = &nodes[broker_index];
            let endpoint = node.endpoint();
            let client_id = format!("udx710_{}", self.imei);

            debug!("Connecting to MQTT broker {endpoint} client_id={client_id}");
            crate::log_entry!(info, LOG_MODULE, "正在连接节点 {}", endpoint);

            Self::update_state_static(|state| {
                state.current_broker = endpoint.clone();
                state.broker_index = broker_index;
                state.connected = false;
                state.error_message = None;
            }).await;

            match self.connect_broker(node, &client_id, config).await {
                BrokerOutcome::Disabled => {
                    // 连接期间被关闭，teardown 已在 connect_broker 内完成
                    return;
                }
                BrokerOutcome::ConfigChanged => {
                    // 本函数持有的 config 已过期，退回 run() 重新加载后再连
                    return;
                }
                BrokerOutcome::Closed => {
                    // 正常运行后被 broker 断开，重置退避并尝试下一节点
                    consecutive_failures = 0;
                    debug!("MQTT connection to {endpoint} closed gracefully, trying next");
                    broker_index += 1;
                }
                BrokerOutcome::Failed(e) => {
                    let err_msg = format!("{endpoint}: {e}");
                    error!("Failed to connect to MQTT broker {err_msg}");
                    crate::log_entry!(error, LOG_MODULE, "节点连接失败：{}", err_msg);
                    Self::update_state_static(|state| {
                        state.error_message = Some(err_msg);
                    }).await;

                    consecutive_failures += 1;
                    broker_index += 1;

                    // 所有节点都试过且全部失败 → 指数退避
                    if broker_index >= nodes.len() {
                        let wait = min(
                            Duration::from_secs(3)
                                .saturating_mul(2u32.saturating_pow(consecutive_failures.min(5))),
                            Duration::from_secs(60),
                        );
                        warn!("All MQTT brokers unreachable, backing off {}s", wait.as_secs());
                        crate::log_entry!(
                            warn,
                            LOG_MODULE,
                            "全部节点不可达，{}秒后重试（连续失败 {} 次）",
                            wait.as_secs(),
                            consecutive_failures
                        );
                        // 退避期间也要能被「关闭」打断，分段 sleep
                        if self.sleep_responsive_to_disable(wait).await {
                            Self::teardown("配置已关闭").await;
                            return;
                        }
                    } else {
                        // 当前节点失败但还有剩余节点，快速尝试下一个
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }
    }

    /// 分段睡眠，期间若配置被关闭则提前返回 `true`。
    ///
    /// 指数退避最长 60s，若整段 sleep 会让「关闭开关」迟迟不生效。
    async fn sleep_responsive_to_disable(&self, total: Duration) -> bool {
        let mut remaining = total;
        let step = Duration::from_secs(1);
        while remaining > Duration::ZERO {
            let this = min(remaining, step);
            sleep(this).await;
            remaining = remaining.saturating_sub(this);
            if !self.config_manager.get_mqtt().enabled {
                return true;
            }
        }
        false
    }

    async fn connect_broker(
        &self,
        node: &MqttBrokerNode,
        client_id: &str,
        config: &MqttConfig,
    ) -> BrokerOutcome {
        // 清除上一个连接的客户端，避免在新连接建立期间误用旧连接发布
        MQTT_CLIENT.lock().await.take();

        // 每节点独立解析 host / TLS / 端口（不再共享全局 port）
        let (host, use_tls) = node.resolve();
        if host.is_empty() {
            return BrokerOutcome::Failed("空主机名".to_string());
        }
        let port = node.effective_port();
        let endpoint = node.endpoint();

        let mut mqttoptions = MqttOptions::new(client_id, host.clone(), port);
        mqttoptions.set_keep_alive(Duration::from_secs(60));
        mqttoptions.set_clean_session(true);

        // TLS
        if use_tls {
            mqttoptions.set_transport(Transport::tls_with_default_config());
            debug!("MQTT TLS enabled for {host} (system CA certs)");
        }

        // 用户名密码认证
        if let Some(ref user) = config.username {
            let pass = config.password.as_deref().unwrap_or("");
            mqttoptions.set_credentials(user, pass);
            debug!("MQTT credentials set for user {user}");
        }

        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

        // 订阅指令主题
        let topic_sub = config.topic_sub.replace("{imei}", &self.imei);
        if let Err(e) = client.subscribe(&topic_sub, QoS::AtLeastOnce).await {
            return BrokerOutcome::Failed(format!("subscribe {topic_sub}: {e}"));
        }

        // 计算发布主题
        let topic_pub = config.topic_pub.replace("{imei}", &self.imei);

        // 连接阶段：等待 ConnAck，超时则快速失败以切换下一个节点。
        // 关键修复：connected 只在收到 ConnAck 之后才置位，
        // 旧实现在 await ConnAck 之前就写 connected=true，会短暂显示假连接。
        let connect_deadline = Duration::from_secs(8);
        let conn_ack = async {
            // 可能先收到其它包（如 SubAck），循环直到 ConnAck 或超时
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok::<(), String>(()),
                    Ok(_) => continue,
                    Err(e) => return Err(format!("{endpoint}: {e}")),
                }
            }
        };
        match timeout(connect_deadline, conn_ack).await {
            Ok(Ok(())) => {
                info!("MQTT connected to {endpoint}");
                crate::log_entry!(info, LOG_MODULE, "已连接 {}", endpoint);
                Self::update_state_static(|state| {
                    state.connected = true;
                    state.error_message = None;
                }).await;
            }
            Ok(Err(e)) => return BrokerOutcome::Failed(e),
            Err(_) => return BrokerOutcome::Failed(format!("connect timeout to {endpoint}")),
        }

        info!("Subscribed to command topic {topic_sub}");
        crate::log_entry!(info, LOG_MODULE, "已订阅指令主题 {}", topic_sub);

        // 连接成功后存储客户端以供 publish 复用
        MQTT_CLIENT.lock().await.replace((client.clone(), topic_pub.clone()));

        // 发布初始上线状态
        send_mqtt_notification(
            "mqtt_connected",
            "connect",
            &format!("MQTT 已连接至 {}", endpoint),
        );
        {
            let dbus_clone = Arc::clone(&self.dbus_conn);
            tokio::spawn(async move {
                publish_status(&dbus_clone).await;
            });
        }

        // 事件循环：保持连接，处理下行指令 + 周期性心跳 + 关闭检测。
        //
        // 关键设计：
        // 1. 心跳定时器集成在事件循环内（tokio::select!），不再 spawn 独立 task。
        //    之前每次 connect_broker() 都会 spawn 心跳 task，断连重连后旧 task 未退出
        //    导致多个心跳 task 叠加，状态发布频率翻倍。
        // 2. 收到的 Publish 消息 spawn 到独立 task 处理，因为 handle_command 里的
        //    publish 需要 EventLoop 轮询来完成网络 I/O，不能在 poll 回调中 await。
        // 3. 过滤 topic_pub 上的自回环：如果 topic_pub == topic_sub，忽略自己发布的消息。
        // 4. disable_timer 周期性检查 enabled，使「连接中关闭开关」能在 5s 内断开。
        let heartbeat_interval = Duration::from_secs(300);
        let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
        // 第一次 tick 立即触发，跳过它
        heartbeat_timer.tick().await;
        let mut disable_timer = tokio::time::interval(Duration::from_secs(5));
        disable_timer.tick().await;
        // 本连接的配置指纹，用于检测「连接期间改了配置」
        let conn_sig = connection_signature(config);

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
                            warn!("MQTT disconnected by broker {endpoint}");
                            crate::log_entry!(warn, LOG_MODULE, "被节点断开：{}", endpoint);
                            Self::teardown("broker disconnect").await;
                            return BrokerOutcome::Closed;
                        }
                        Ok(Event::Incoming(Packet::ConnAck(_))) => {
                            // 重连后的 ConnAck，忽略
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let err_msg = e.to_string();
                            error!("MQTT event loop error on {endpoint}: {err_msg}");
                            crate::log_entry!(error, LOG_MODULE, "事件循环错误（{}）：{}", endpoint, err_msg);
                            MQTT_CLIENT.lock().await.take();
                            Self::update_state_static(|state| {
                                state.connected = false;
                                state.error_message = Some(err_msg);
                            }).await;
                            return BrokerOutcome::Failed(e.to_string());
                        }
                    }
                }
                _ = heartbeat_timer.tick() => {
                    let dbus_clone = Arc::clone(&self.dbus_conn);
                    tokio::spawn(async move {
                        publish_status(&dbus_clone).await;
                    });
                }
                _ = disable_timer.tick() => {
                    let current = self.config_manager.get_mqtt().sanitize();
                    // 连接期间被关闭 → 主动断开并清理状态
                    if !current.enabled {
                        info!("MQTT disabled while connected, disconnecting from {endpoint}");
                        crate::log_entry!(info, LOG_MODULE, "检测到已停用，主动断开 {}", endpoint);
                        // 尽量礼貌地发 Disconnect，再清理
                        let _ = client.disconnect().await;
                        Self::teardown("disabled while connected").await;
                        return BrokerOutcome::Disabled;
                    }
                    // 连接期间改了节点/端口/主题/TLS → 断开并用新配置重连，
                    // 让「保存配置后自动重连」的承诺在已连接状态下也成立。
                    // 必须返回 ConfigChanged 让上层重新加载配置：本函数持有的是旧 &config，
                    // 若按 Closed 处理，重连时用的仍是旧节点列表。
                    if connection_signature(&current) != conn_sig {
                        info!("MQTT config changed while connected, reconnecting from {endpoint}");
                        crate::log_entry!(info, LOG_MODULE, "检测到配置变更，重连 {}", endpoint);
                        let _ = client.disconnect().await;
                        Self::teardown("config changed while connected").await;
                        return BrokerOutcome::ConfigChanged;
                    }
                }
            }
        }
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

/// 计算「会影响当前连接」的配置指纹。
///
/// 只纳入连接期真正生效的字段：节点列表（host/port/tls）、激活节点、主题、TLS 相关
/// 凭据与 token。`enabled` 不在内——它由控制循环单独判断，语义不同。
///
/// 用字符串拼接而不是 derive(PartialEq) 比较整个 MqttConfig，是为了忽略那些
/// 不影响现有连接的字段（如旧版镜像字段），避免无意义的重连抖动。
fn connection_signature(config: &MqttConfig) -> String {
    let nodes: Vec<String> = config
        .nodes
        .iter()
        .map(|n| {
            let (host, tls) = n.resolve();
            format!("{host}:{}:{tls}", n.effective_port())
        })
        .collect();
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        nodes.join(","),
        config.active_broker,
        config.topic_sub,
        config.topic_pub,
        config.username.as_deref().unwrap_or(""),
        config.password.as_deref().unwrap_or(""),
        config.auth_token.as_deref().unwrap_or(""),
    )
}

/// `connect_broker` 的返回，区分各种结局，让上层决定是重试、换节点还是重载配置。
enum BrokerOutcome {
    /// 配置在连接期间被关闭 → 立即退出，不重连
    Disabled,
    /// 配置在连接期间被修改 → 退到 `run()` 重新加载配置后再连
    ConfigChanged,
    /// 已连接后被 broker 正常断开（应尝试下一节点，重置退避）
    Closed,
    /// 连接或事件循环出错
    Failed(String),
}

/// 在独立 task 中处理 MQTT 指令（由 EventLoop spawn 调用）。
///
/// 此函数不能 running 在与 EventLoop 相同的 task 中，否则 publish_status 里
/// client.publish() 会死锁（publish 依赖 EventLoop 轮询来完成网络 I/O）。
async fn handle_command_spawned(payload: &[u8], config: &MqttConfig, dbus_conn: &Connection) {
    let payload_str = match std::str::from_utf8(payload) {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid UTF-8 in MQTT payload: {e}");
            crate::log_entry!(error, LOG_MODULE, "指令载荷不是合法 UTF-8：{}", e);
            return;
        }
    };

    debug!("Received MQTT command payload: {payload_str}");

    let cmd: MqttCommand = match serde_json::from_str(payload_str) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to parse MQTT command: {e}");
            crate::log_entry!(error, LOG_MODULE, "指令 JSON 解析失败：{}", e);
            return;
        }
    };

    // Token 校验
    if let Some(ref expected_token) = config.auth_token {
        match &cmd.token {
            Some(token) if token == expected_token => {}
            _ => {
                error!("Invalid or missing token, ignoring MQTT command");
                crate::log_entry!(
                    warn,
                    LOG_MODULE,
                    "鉴权失败，已忽略指令 {}（token 缺失或不匹配）",
                    cmd.action
                );
                return;
            }
        }
    }

    MqttService::update_state_static(|state| {
        state.last_command = Some(chrono::Utc::now().to_rfc3339());
    }).await;

    info!("Executing MQTT command: {}", cmd.action);
    crate::log_entry!(info, LOG_MODULE, "收到指令：{}", cmd.action);

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
                error!("Failed to disconnect data: {e}");
                crate::log_entry!(error, LOG_MODULE, "断开数据连接失败：{}", e);
            } else if let Err(e) = crate::dbus::set_data_connection(dbus_conn, true).await {
                error!("Failed to reconnect data: {e}");
                crate::log_entry!(error, LOG_MODULE, "重连数据连接失败：{}", e);
            } else {
                crate::log_entry!(info, LOG_MODULE, "数据连接已重置");
            }
        }
        "status" => {
            // 采集一次完整报告，文本摘要走推送、完整 JSON 走 topic_pub，两者数据一致
            let report = DeviceReport::collect(dbus_conn).await;
            send_mqtt_notification(
                "mqtt_command_executed",
                "status",
                &report.format_summary("📊 设备状态报告"),
            );
            // 复用同一次采集，把完整 JSON 发布到 topic_pub 供外部系统消费
            publish_report(&report).await;
        }
        _ => {
            warn!("Unknown MQTT command: {}", cmd.action);
            crate::log_entry!(warn, LOG_MODULE, "未知指令：{}", cmd.action);
        }
    }
}

/// 采集并发布系统状态到 MQTT（心跳路径用，独立函数不依赖 MqttService 实例）。
async fn publish_status(dbus_conn: &Connection) {
    let report = DeviceReport::collect(dbus_conn).await;
    publish_report(&report).await;
}

/// 把已采集好的报告发布到 topic_pub，并刷新心跳时间戳。
///
/// 与 [`publish_status`] 拆开，是为了让 `status` 指令复用「文本摘要 + JSON 发布」
/// 同一次采集结果，避免重复采样 CPU（sample_cpu_usage 有 200ms 间隔）。
async fn publish_report(report: &DeviceReport) {
    let payload = match serde_json::to_string(report) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to serialize status: {e}");
            crate::log_entry!(error, LOG_MODULE, "状态序列化失败：{}", e);
            return;
        }
    };

    // 复用当前已连接的客户端发布，避免用可能已过期的 active_broker 重新连一个新临时客户端
    let (client, topic_pub) = match MQTT_CLIENT.lock().await.clone() {
        Some(entry) => entry,
        None => {
            debug!("No active MQTT client for publish_status");
            return;
        }
    };

    match client.publish(&topic_pub, QoS::AtLeastOnce, false, payload.as_bytes()).await {
        Ok(_) => {
            debug!("Published status to {topic_pub}");
            crate::log_entry!(debug, LOG_MODULE, "已发布状态到 {}", topic_pub);
            MqttService::update_state_static(|state| {
                state.last_heartbeat = Some(chrono::Utc::now().to_rfc3339());
            }).await;
        }
        Err(e) => {
            error!("Failed to publish status to {topic_pub}: {e}");
            crate::log_entry!(error, LOG_MODULE, "发布状态到 {} 失败：{}", topic_pub, e);
        }
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

/// 检查 MQTT 是否已连接
pub async fn is_mqtt_connected() -> bool {
    let state_guard = MQTT_STATE.lock().await;
    state_guard.as_ref().map_or(false, |s| s.connected)
}

#[cfg(test)]
mod tests {
    use super::connection_signature;
    use crate::config::{MqttBrokerNode, MqttConfig};

    fn base_config() -> MqttConfig {
        MqttConfig {
            nodes: vec![MqttBrokerNode::new("a.example.com", 1883, false)],
            active_broker: "a.example.com".to_string(),
            topic_sub: "cpe/{imei}/cmd".to_string(),
            topic_pub: "cpe/{imei}/status".to_string(),
            ..MqttConfig::default()
        }
    }

    #[test]
    fn signature_is_stable_for_identical_config() {
        assert_eq!(connection_signature(&base_config()), connection_signature(&base_config()));
    }

    #[test]
    fn signature_ignores_enabled_flag() {
        // enabled 由控制循环单独判断，不应触发「配置变更重连」
        let mut disabled = base_config();
        disabled.enabled = false;
        assert_eq!(connection_signature(&base_config()), connection_signature(&disabled));
    }

    #[test]
    fn signature_detects_node_changes() {
        let mut changed = base_config();
        changed.nodes = vec![MqttBrokerNode::new("b.example.com", 1883, false)];
        assert_ne!(connection_signature(&base_config()), connection_signature(&changed));
    }

    #[test]
    fn signature_detects_port_change() {
        let mut changed = base_config();
        changed.nodes = vec![MqttBrokerNode::new("a.example.com", 8883, false)];
        assert_ne!(connection_signature(&base_config()), connection_signature(&changed));
    }

    #[test]
    fn signature_detects_tls_change() {
        let mut changed = base_config();
        changed.nodes = vec![MqttBrokerNode::new("a.example.com", 8883, true)];
        assert_ne!(connection_signature(&base_config()), connection_signature(&changed));
    }

    #[test]
    fn signature_treats_ssl_prefix_as_equivalent_to_tls_flag() {
        // 用户把 tls 开关改成 ssl:// 前缀写法，连接参数其实没变，不应触发重连
        let prefixed = MqttConfig {
            nodes: vec![MqttBrokerNode::new("ssl://a.example.com", 8883, false)],
            ..base_config()
        };
        let flagged = MqttConfig {
            nodes: vec![MqttBrokerNode::new("a.example.com", 8883, true)],
            ..base_config()
        };
        assert_eq!(connection_signature(&prefixed), connection_signature(&flagged));
    }

    #[test]
    fn signature_detects_topic_change() {
        let mut changed = base_config();
        changed.topic_sub = "other/{imei}/cmd".to_string();
        assert_ne!(connection_signature(&base_config()), connection_signature(&changed));
    }

    #[test]
    fn signature_detects_credential_change() {
        let mut changed = base_config();
        changed.username = Some("user".to_string());
        changed.password = Some("pass".to_string());
        assert_ne!(connection_signature(&base_config()), connection_signature(&changed));
    }

    #[test]
    fn signature_detects_token_change() {
        let mut changed = base_config();
        changed.auth_token = Some("secret-token".to_string());
        assert_ne!(connection_signature(&base_config()), connection_signature(&changed));
    }

    #[test]
    fn signature_detects_node_order_change() {
        // 节点顺序决定故障转移顺序，变了就该重连
        let two = MqttConfig {
            nodes: vec![
                MqttBrokerNode::new("a.example.com", 1883, false),
                MqttBrokerNode::new("b.example.com", 1883, false),
            ],
            ..base_config()
        };
        let swapped = MqttConfig {
            nodes: vec![
                MqttBrokerNode::new("b.example.com", 1883, false),
                MqttBrokerNode::new("a.example.com", 1883, false),
            ],
            ..base_config()
        };
        assert_ne!(connection_signature(&two), connection_signature(&swapped));
    }
}

