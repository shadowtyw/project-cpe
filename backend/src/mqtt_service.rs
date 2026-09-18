use crate::config::{ConfigManager, MqttBrokerNode, MqttConfig};
use crate::device_report::DeviceReport;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};
use zbus::Connection;

/// 外网连通性探测目标：阿里 DNS TCP 端口。
///
/// 选用 TCP 53 而非 ICMP ping：ICMP 可能需要 root / raw socket，
/// 而 TCP connect 无需特权且精确对应「能否建立外网 socket」的判断。
/// 阿里 DNS 223.5.5.5 是国内可达性最好的公共地址之一。
const PROBE_ADDR: &str = "223.5.5.5:53";

/// 单次探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 探测失败后的重试间隔。
const PROBE_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// 冷启动首轮"外网连通性已确认"标记。
///
/// 进程生命周期内仅生效一次：首次通过探测后置位，后续循环直接跳过。
static NETWORK_PROBE_PASSED: AtomicBool = AtomicBool::new(false);

/// MQTT 模块在前端日志里的独立模块名。
///
/// `LogBufferLayer` 默认从 `target` 取末段（`udx710::mqtt_service` → `mqtt_service`），
/// 但本模块的关键事件通过 `log_entry!` 显式写入 `"mqtt"`，使前端 MQTT 页可以只筛选
/// 这一类日志，与系统日志页的全量视图区分开。
const LOG_MODULE: &str = "mqtt";

/// MQTT 单包大小上限（收/发同值），单位字节。
///
/// 必须显式设置：rumqttc 的默认值是 10KB，而完整设备状态报告 JSON 约 10.3KB，
/// 刚好越线，导致发布状态时直接报错并断开重连（表现为「MQTT 无法连接」）。
const MAX_PACKET_SIZE: usize = 64 * 1024;

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
    let ts = crate::utils::beijing_now_rfc3339();
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
#[derive(Debug, Clone, Default, Serialize)]
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

/// 全局运行时状态
static MQTT_STATE: Mutex<Option<MqttRuntimeState>> = Mutex::const_new(None);
static MQTT_ENABLED: AtomicBool = AtomicBool::new(false);

/// 已连接的 MQTT 客户端（用于发布消息，避免每次创建临时客户端）
static MQTT_CLIENT: Mutex<Option<(AsyncClient, String)>> = Mutex::const_new(None);

/// 数据连接看门狗恢复连接后置位，用于中断退避 sleep 立即重连。
static DATA_CONNECTION_RESTORED: AtomicBool = AtomicBool::new(false);

/// 由数据连接看门狗在恢复后调用，唤醒退避中的 MQTT 重连循环。
pub fn notify_data_connection_restored() {
    DATA_CONNECTION_RESTORED.store(true, Ordering::SeqCst);
}

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

            // ── 外网连通性探测门禁 ────
            //
            // v3.7.5：用轻量 TCP connect 验证蜂窝路由是否真正打通，
            // 替代 v3.7.4 的固定 90s 盲等。固定延时无法适应不同基带
            // 注网速度的差异——有时 90s 不够，有时 45s 就已就绪。
            //
            // 策略：首轮循环依次执行 TCP 探测（1.5s 超时），失败则
            // 静默休眠 5s 再试，不发起任何 MQTT 建连、不产生 error 日志。
            // 一旦探测通过标记即永久置位，后续循环（含用户手动关→开）跳过。
            if !NETWORK_PROBE_PASSED.load(Ordering::SeqCst) {
                self.wait_for_network_probe().await;
                // 等待期间被用户关闭 MQTT → 回到外层循环顶部 teardown
                if !self.config_manager.get_mqtt().enabled {
                    continue;
                }
                // 探测成功；以 info 级别告知用户连通时间点
                crate::log_entry!(
                    info,
                    LOG_MODULE,
                    "外网连通性确认，开始 MQTT 建连"
                );
            }

            // connect_and_run 会在「被关闭」或「所有节点失败」时返回。
            // 返回后回到循环顶部重读配置，因此关闭开关能在一轮内生效。
            self.connect_and_run(&config).await;

            // 重连间隔
            sleep(Duration::from_secs(5)).await;
        }
    }

    /// 外网连通性探测等待循环。
    ///
    /// 每 5s 发起一次轻量 TCP connect 到阿里 DNS 53 端口（1.5s 超时），
    /// 通网即返回并置位 `NETWORK_PROBE_PASSED`。期间仅打 debug 级别日志，
    /// 不产生 error、不发起 MQTT 建连，将开机脉冲发热降到最低。
    async fn wait_for_network_probe(&self) {
        let mut first = true;
        loop {
            if !self.config_manager.get_mqtt().enabled {
                return;
            }
            if first {
                crate::log_entry!(info, LOG_MODULE, "等待外网连通（探测 {}）…", PROBE_ADDR);
                first = false;
            }
            if probe_network().await {
                NETWORK_PROBE_PASSED.store(true, Ordering::SeqCst);
                return;
            }
            debug!("Network probe failed, retrying in {}s", PROBE_RETRY_INTERVAL.as_secs());
            // 分段 sleep：允许用户在此期间关闭 MQTT
            let mut remaining = PROBE_RETRY_INTERVAL;
            let tick = Duration::from_secs(1);
            while remaining > Duration::ZERO {
                let this = min(remaining, tick);
                sleep(this).await;
                remaining = remaining.saturating_sub(this);
                if !self.config_manager.get_mqtt().enabled {
                    return;
                }
            }
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
        // 会话中断的独立退避计数：与 consecutive_failures 分开，
        // 因为「连上过又掉线」不代表节点不可达，退避上限也应更短（30s vs 60s）。
        // 每次成功进入事件循环后归零，避免长期运行时退避一路涨到上限。
        let mut session_retries: u32 = 0;

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

            let session_started = Instant::now();
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
                BrokerOutcome::SessionLost(e) => {
                    // 连上过又掉线：节点是可达的，原地重连同一个节点即可。
                    // 这里既不改 broker_index（不轮换到别的节点），
                    // 也不累加 consecutive_failures（不是「全部节点不可达」），
                    // 只做一个短的、仍受上限约束的退避，避免掉线时疯狂重连。
                    // 掉线常见原因：broker 侧踢掉旧 client_id、网络抖动、
                    // 或发布超限被 broker 断开。
                    consecutive_failures = 0;
                    // 会话如果撑过 60s，说明链路本身是好的，这次掉线按「偶发」处理：
                    // 退避计数归零，下次仍从 3s 起。否则长期运行的设备每掉一次线
                    // 就把退避推高一档，最终固定在 30s，重连越来越慢。
                    if session_started.elapsed() >= Duration::from_secs(60) {
                        session_retries = 0;
                    }
                    let wait = min(
                        Duration::from_secs(3)
                            .saturating_mul(2u32.saturating_pow(session_retries.min(3))),
                        Duration::from_secs(10),
                    );
                    session_retries += 1;
                    warn!("MQTT session lost on {endpoint}: {e}; reconnecting in {}s", wait.as_secs());
                    crate::log_entry!(
                        warn,
                        LOG_MODULE,
                        "会话中断，{}秒后重连同一节点：{}",
                        wait.as_secs(),
                        endpoint
                    );
                    if self.sleep_responsive_to_disable(wait).await {
                        Self::teardown("配置已关闭").await;
                        return;
                    }
                    // 不递增 broker_index：下一轮循环会重连同一个节点
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

                    // 所有节点都试过且全部失败 → 指数退避（上限 10s）
                    if broker_index >= nodes.len() {
                        let wait = min(
                            Duration::from_secs(3)
                                .saturating_mul(2u32.saturating_pow(consecutive_failures.min(3))),
                            Duration::from_secs(10),
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
    /// 同时监听 `DATA_CONNECTION_RESTORED` 信号：数据连接恢复时清除标志
    /// 并提前返回 `false`，让上层立即重试建连而不走 teardown。
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
            if DATA_CONNECTION_RESTORED.swap(false, Ordering::SeqCst) {
                info!("MQTT: data connection restored, waking early from backoff");
                crate::log_entry!(info, LOG_MODULE, "数据连接已恢复，提前结束退避");
                return false;
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

        // ── DNS 预解析（spawn_blocking + 独立超时） ─────────────────
        //
        // **v3.7.1 核心修复**：在 aarch64-unknown-linux-musl 目标上，
        // rumqttc 内部的 TCP 建连调用 getaddrinfo()——这是 libc 的**同步阻塞**
        // DNS 解析，会直接卡死 tokio 工作线程。tokio::time::timeout 无法取消
        // 已阻塞的 OS 线程，于是每次建连重试都白白堵住一个 worker 长达 60s
        // （内核 TCP SYN 重传上限），快速耗尽 tokio 线程池，导致整个 MQTT
        // 子系统"假死"——直到手动开关重建 Task 时 DNS 已缓存才瞬间成功。
        //
        // 这里把 DNS 解析放到专用阻塞线程上，配一个 4s 独立超时：
        // 超时后我们已知 DNS 不可达，直接走退避重试，不占用 async 工作线程。
        // TLS 节点跳过预解析——证书校验需要原始域名。
        let connect_host = if use_tls {
            host.clone()
        } else {
            let host_for_dns = host.clone();
            let dns_result = timeout(Duration::from_secs(4), tokio::task::spawn_blocking(move || {
                format!("{host_for_dns}:{port}").to_socket_addrs().ok()
            }))
            .await;
            match dns_result {
                Ok(Ok(Some(addrs))) => {
                    // 取第一个可路由地址
                    if let Some(addr) = addrs
                        .filter(|a| a.is_ipv4())  // 优先 IPv4（嵌入式 CPE 场景）
                        .next()
                        .or_else(|| {
                            format!("{host}:{port}").to_socket_addrs().ok()
                                .and_then(|mut a| a.next())
                        })
                    {
                        debug!("MQTT DNS resolved {host} -> {}", addr.ip());
                        addr.ip().to_string()
                    } else {
                        debug!("MQTT DNS: no routable address for {host}, using hostname");
                        host.clone()
                    }
                }
                _ => {
                    // DNS 预解析超时或错误 → 直接失败，绝不 fallback 到 hostname。
                    // 若传 hostname 给 rumqttc，它内部又会触发阻塞 getaddrinfo()，
                    // 卡死 tokio worker 线程 60s，timeout 无法打断。
                    debug!("MQTT DNS timeout or error for {host}, failing fast");
                    return BrokerOutcome::Failed(format!("DNS resolution failed for {host}"));
                }
            }
        };

        // ── 建连阶段：4 秒硬超时（全局 Runtime） ────────────────
        //
        // DNS 已在上一步完成预解析，此 async block 内不再发生阻塞调用，
        // tokio::time::timeout 可以正确取消超时的 TCP SYN 挂起。
        let topic_sub = config.topic_sub.replace("{imei}", &self.imei);
        let topic_pub = config.topic_pub.replace("{imei}", &self.imei);

        let mut mqttoptions = MqttOptions::new(client_id, connect_host, port);
        mqttoptions.set_keep_alive(Duration::from_secs(60));
        mqttoptions.set_clean_session(true);
        mqttoptions.set_max_packet_size(MAX_PACKET_SIZE, MAX_PACKET_SIZE);
        if use_tls {
            mqttoptions.set_transport(Transport::tls_with_default_config());
            debug!("MQTT TLS enabled for {host} (system CA certs)");
        }
        if let Some(ref user) = config.username {
            let pass = config.password.as_deref().unwrap_or("");
            mqttoptions.set_credentials(user, pass);
            debug!("MQTT credentials set for user {user}");
        }

        let connect_deadline = Duration::from_secs(4);
        let conn_result = timeout(connect_deadline, async {
            let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);
            client
                .subscribe(&topic_sub, QoS::AtLeastOnce)
                .await
                .map_err(|e| format!("subscribe {topic_sub}: {e}"))?;
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => break,
                    Ok(_) => continue,
                    Err(e) => return Err(format!("{endpoint}: {e}")),
                }
            }
            Ok::<_, String>((client, eventloop))
        })
        .await;

        let (client, mut eventloop) = match conn_result {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return BrokerOutcome::Failed(e),
            Err(_) => {
                return BrokerOutcome::Failed(format!("connect timeout to {endpoint}"));
            }
        };

        // ── 连接成功 ──────────────────────────────────────────
        // 关键修复：connected 只在收到 ConnAck 之后才置位，
        // 旧实现在 await ConnAck 之前就写 connected=true，会短暂显示假连接。
        info!("MQTT connected to {endpoint}");
        crate::log_entry!(info, LOG_MODULE, "已连接 {}", endpoint);
        Self::update_state_static(|state| {
            state.connected = true;
            state.error_message = None;
        })
        .await;

        info!("Subscribed to command topic {topic_sub}");
        crate::log_entry!(info, LOG_MODULE, "已订阅指令主题 {}", topic_sub);

        // 连接成功后存储客户端以供 publish 复用
        MQTT_CLIENT
            .lock()
            .await
            .replace((client.clone(), topic_pub.clone()));

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
                            crate::log_entry!(error, LOG_MODULE, "连接中断（{}）：{}", endpoint, err_msg);
                            MQTT_CLIENT.lock().await.take();
                            // 克隆进闭包：err_msg 还要作为 SessionLost 的载荷返回
                            let state_err = err_msg.clone();
                            Self::update_state_static(|state| {
                                state.connected = false;
                                state.error_message = Some(state_err);
                            }).await;
                            // 注意：走到这里说明 ConnAck 早已收到、订阅也已成功，
                            // 这是「会话中断」而不是「节点不可达」。必须用独立变体，
                            // 否则上层会按连接失败累加退避，日志也会误报
                            // 「节点连接失败 / 全部节点不可达」（节点其实是通的）。
                            return BrokerOutcome::SessionLost(err_msg);
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

/// 外网连通性轻量探测：对阿里 DNS 53 端口发起 TCP connect。
///
/// 不涉及 DNS 解析（直接用 IP）、不涉及 TLS 握手、不产生任何日志输出。
/// 1.5s 超时足以覆盖最差的蜂窝链路建立场景。
/// 返回 `true` 表示外网路由已就绪，`false` 表示仍不可达。
async fn probe_network() -> bool {
    timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(PROBE_ADDR))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
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
    /// 连接阶段失败：从没收到 ConnAck，节点可能确实不可达 → 轮换到下一节点并累加退避
    Failed(String),
    /// 会话中断：ConnAck 早已收到、订阅成功，是运行期掉线而非「节点不可达」。
    /// 必须与 `Failed` 区分——两者混用会让上层把一个明明连得上的节点
    /// 报成「节点连接失败 / 全部节点不可达」，并无谓地轮换到别的节点。
    SessionLost(String),
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
/// 同一次采集结果，避免重复采样 CPU（sample_cpu_usage 有 2.0s 间隔）。
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
    state_guard.as_ref().is_some_and(|s| s.connected)
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

    /// 手写 `Default` 换成 `#[derive(Default)]` 后的等价性保障：
    /// 初始状态必须是「未启用 + 未连接 + 无 broker」，
    /// 否则 UI 会在进程刚起来时显示一个假的已连接状态。
    #[test]
    fn runtime_state_default_is_fully_disconnected() {
        use super::MqttRuntimeState;

        let s = MqttRuntimeState::default();
        assert!(!s.enabled, "默认不应显示为已启用");
        assert!(!s.connected, "默认不应显示为已连接");
        assert!(s.current_broker.is_empty(), "默认不应有 broker");
        assert!(s.error_message.is_none());
        assert!(s.last_heartbeat.is_none());
        assert!(s.last_command.is_none());
        assert_eq!(s.broker_index, 0);
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

    /// 回归测试：rumqttc 默认单包上限 10KB，而完整状态报告 JSON 约 10.3KB，
    /// 会刚好越线导致发布失败 + 断连重连（现场表现为「MQTT 无法连接」）。
    /// 这里断言 MAX_PACKET_SIZE 明确大于实测报告体积，且留足余量。
    ///
    /// 用真实采集器的序列化结果而非硬编码字节数，这样报告结构以后变大时
    /// 测试会跟着变严，不会退化成一句永远为真的断言。
    #[test]
    fn max_packet_size_exceeds_realistic_report() {
        use super::MAX_PACKET_SIZE;
        use crate::device_report::DeviceReport;
        use crate::models::{
            AirplaneModeResponse, CpuLoadInfo, DeviceInfoResponse, DiskInfo, IpAddress,
            MemoryInfo, NetworkInfoResponse, NetworkInterfaceInfo, ServingCell, SystemInfo,
            ThermalZone,
        };

        // 构造一份与现场同量级的报告。体积主要来自两块：
        // 1) 可选块全部填充（device/network/serving_cell/airplane/memory/cpu_load/system）
        //    ——真机上这些都有值，空报告只有 2.5KB，远不足以复现越线
        // 2) 9 个温度传感器（与实测日志的 *-thmzone 数量一致）+ 多网卡多磁盘
        let report = DeviceReport {
            timestamp: "2026-09-14T09:24:02.051140786+00:00".to_string(),
            app_version: "3.7.2".to_string(),
            git_commit: "95c1a2d".to_string(),
            device: Some(DeviceInfoResponse {
                imei: "868659060480591".to_string(),
                manufacturer: "Fake Modem Manufacturer".to_string(),
                model: "Fake Modem Model".to_string(),
                revision: Some("UDX710_V1.0.0_B05".to_string()),
                online: true,
                powered: true,
            }),
            network: Some(NetworkInfoResponse {
                operator_name: "China Telecom".to_string(),
                registration_status: "registered".to_string(),
                technology_preference: "NR 5G/LTE auto".to_string(),
                signal_strength: 41,
                mcc: Some("460".to_string()),
                mnc: Some("11".to_string()),
            }),
            serving_cell: Some(ServingCell {
                tech: "nr".to_string(),
                cell_id: 0,
                tac: 13_607_681,
            }),
            signal_strength: Some(41),
            data_connected: Some(true),
            airplane: Some(AirplaneModeResponse {
                enabled: false,
                ..AirplaneModeResponse::default()
            }),
            memory: Some(MemoryInfo {
                total_bytes: 206_569_472,
                available_bytes: 105_906_176,
                available_percent: 51.0,
                free_bytes: 74_973_184,
                cached_bytes: 32_505_856,
                buffers_bytes: 1_048_576,
                reclaimable_bytes: 31_457_280,
                buff_cache_bytes: 32_505_856,
                shared_bytes: 4_194_304,
                process_non_reclaimable_used_bytes: 68_157_440,
                available_estimated: false,
                available_source: "kernel".to_string(),
                ..MemoryInfo::default()
            }),
            cpu_usage_percent: Some(42.9),
            cpu_load: Some(CpuLoadInfo {
                load_1min: 2.30,
                load_5min: 1.13,
                load_15min: 0.44,
                core_count: 2,
                load_percent: 115.0,
            }),
            thermal: (0..9)
                .map(|i| ThermalZone {
                    zone: format!("thermal_zone{i}"),
                    sensor_type: format!("sensor{i}-thmzone"),
                    temperature: 41.2,
                })
                .collect(),
            uptime_seconds: Some(131),
            disks: (0..6)
                .map(|i| DiskInfo {
                    mount_point: format!("/mnt/data{i}"),
                    fs_type: "ubifs".to_string(),
                    total_bytes: 1_000_000_000,
                    used_bytes: 470_000_000,
                    available_bytes: 530_000_000,
                    used_percent: 47.0,
                })
                .collect(),
            interfaces: (0..8)
                .map(|i| NetworkInterfaceInfo {
                    name: format!("eth{i}"),
                    status: "up".to_string(),
                    mac_address: Some("aa:bb:cc:dd:ee:ff".to_string()),
                    mtu: 1500,
                    // 每块网卡带 IPv4 + IPv6，贴近真机（IPv6 通常有多个地址）
                    ip_addresses: vec![
                        IpAddress {
                            address: "10.132.240.44".to_string(),
                            prefix_len: 24,
                            ip_type: "ipv4".to_string(),
                            scope: "private".to_string(),
                        },
                        IpAddress {
                            address: "fe80::a8bb:ccff:fedd:eeff".to_string(),
                            prefix_len: 64,
                            ip_type: "ipv6".to_string(),
                            scope: "link-local".to_string(),
                        },
                        IpAddress {
                            address: "2409:8934:1234:5678::1".to_string(),
                            prefix_len: 64,
                            ip_type: "ipv6".to_string(),
                            scope: "public".to_string(),
                        },
                    ],
                    rx_bytes: 123_456_789,
                    tx_bytes: 987_654_321,
                    rx_packets: 100_000,
                    tx_packets: 90_000,
                    rx_errors: 0,
                    tx_errors: 0,
                })
                .collect(),
            system: Some(SystemInfo {
                sysname: "Linux".to_string(),
                nodename: "udx710-cpe".to_string(),
                release: "4.14.98".to_string(),
                version: "#1 SMP PREEMPT aarch64".to_string(),
                machine: "aarch64".to_string(),
                domainname: String::new(),
                full_info: "Linux udx710-cpe 4.14.98 #1 SMP PREEMPT aarch64 GNU/Linux".to_string(),
            }),
        };

        let payload = serde_json::to_string(&report).expect("报告应可序列化");
        let size = payload.len();

        // ── 断言 1：修复本身成立 ───────────────────────────────
        // 必须明显超过 rumqttc 的 10KB 默认上限，否则等于没修。
        const RUMQTTC_DEFAULT_LIMIT: usize = 10 * 1024;
        assert!(
            MAX_PACKET_SIZE > RUMQTTC_DEFAULT_LIMIT,
            "MAX_PACKET_SIZE={MAX_PACKET_SIZE} 未超过 rumqttc 默认的 {RUMQTTC_DEFAULT_LIMIT}B，修复无效"
        );

        // ── 断言 2：对现场实测体积留有足够余量 ─────────────────
        // 用户现场日志里的失败包体为 10306 / 10315 / 10324 字节（真机网卡、
        // 磁盘、传感器数量都比本测试构造的多）。直接拿这个实测最大值做基准，
        // 而不是试图在测试里精确复刻真机的接口列表——复刻不出来，硬凑数量
        // 只会得到一个「看起来像」但守不住任何东西的假基准。
        // 要求至少 4 倍余量：现场 10.3KB × 4 ≈ 41KB < 64KB，成立；
        // 若将来有人把上限改回 16KB，这条会立刻失败。
        const FIELD_OBSERVED_MAX_PACKET: usize = 10_324;
        assert!(
            MAX_PACKET_SIZE >= FIELD_OBSERVED_MAX_PACKET * 4,
            "MAX_PACKET_SIZE={MAX_PACKET_SIZE}B 相对现场实测最大包体 \
             {FIELD_OBSERVED_MAX_PACKET}B 余量不足 4 倍，报告再加字段就会越线"
        );

        // ── 断言 3：本测试构造的报告确实是个「非平凡」的载荷 ──
        // 防止 fixture 哪天被改瘦到几百字节，导致上面两条断言空转。
        // 6KB 是构造版报告的实际量级，留 5KB 阈值给字段增减的波动空间。
        assert!(
            size > 5 * 1024,
            "构造的报告只有 {size}B，太小了——它必须是一份填满了可选块、\
             传感器、磁盘与网卡的完整报告，否则断言 1/2 就失去了参照物"
        );
        // 构造版报告必须放得下（它比真机的瘦，这是必然的，但也要显式守住）
        assert!(
            size < MAX_PACKET_SIZE,
            "报告体积 {size}B 超过上限 {MAX_PACKET_SIZE}B"
        );
    }
}

