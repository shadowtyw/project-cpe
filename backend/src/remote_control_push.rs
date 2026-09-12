//! 远程遥控推送模块
//!
//! 独立于普通 Webhook 配置，专门为短信遥控、通话遥控、MQTT 遥控提供推送通知。
//! 支持独立的启用开关、URL、请求头和签名密钥。

use crate::config::{ConfigManager, RemoteControlPushConfig};
use chrono::Utc;
use reqwest::Client;
use std::sync::Arc;

/// 远程遥控推送发送器
pub struct RemoteControlPushSender {
    client: Client,
    config_manager: Arc<ConfigManager>,
}

impl RemoteControlPushSender {
    /// 创建新的远程遥控推送发送器
    pub fn new(config_manager: Arc<ConfigManager>) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
            config_manager,
        }
    }

    /// 获取当前远程遥控推送配置
    fn get_config(&self) -> RemoteControlPushConfig {
        self.config_manager.get_remote_control_push()
    }

    /// 发送短信遥控事件推送
    pub async fn forward_sms_control(&self, payload: &str) -> Result<(), String> {
        let config = self.get_config();

        if !config.enabled || !config.forward_sms_control || config.webhook_url.is_empty() {
            return Ok(());
        }

        let rendered = render_remote_control_template(&config.template, payload);
        self.send_webhook_raw(&config, &rendered).await
    }

    /// 发送通话遥控事件推送
    pub async fn forward_call_control(&self, payload: &str) -> Result<(), String> {
        let config = self.get_config();

        if !config.enabled || !config.forward_call_control || config.webhook_url.is_empty() {
            return Ok(());
        }

        let rendered = render_remote_control_template(&config.template, payload);
        self.send_webhook_raw(&config, &rendered).await
    }

    /// 发送 MQTT 遥控事件推送
    pub async fn forward_mqtt_control(&self, payload: &str) -> Result<(), String> {
        let config = self.get_config();

        if !config.enabled || !config.forward_mqtt_control || config.webhook_url.is_empty() {
            return Ok(());
        }

        let rendered = render_remote_control_template(&config.template, payload);
        self.send_webhook_raw(&config, &rendered).await
    }

    /// 发送原始 JSON payload 到 Webhook
    async fn send_webhook_raw(&self, config: &RemoteControlPushConfig, payload: &str) -> Result<(), String> {
        let mut request = self.client.post(&config.webhook_url);

        // 添加自定义请求头
        for (key, value) in &config.headers {
            request = request.header(key, value);
        }

        // 添加 Content-Type
        request = request.header("Content-Type", "application/json");

        // 如果有密钥，添加签名头
        if !config.secret.is_empty() {
            let signature = compute_hmac(&config.secret, payload);
            request = request.header("X-Remote-Control-Signature", signature);
        }

        // 发送请求
        let response = request
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| format!("Failed to send remote control webhook: {}", e))?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(format!("Remote control webhook returned error status {}: {}", status, body))
        }
    }

    /// 测试远程遥控推送（发送测试消息，走模板渲染路径）
    pub async fn test_push(&self) -> Result<String, String> {
        let config = self.get_config();

        if config.webhook_url.is_empty() {
            return Err("Webhook URL is not configured".to_string());
        }

        let test_payload = serde_json::json!({
            "timestamp": Utc::now().to_rfc3339(),
            "type": "remote_control_test",
            "data": {
                "event": "test",
                "message": "远程遥控推送测试成功"
            }
        });

        let payload_str = serde_json::to_string(&test_payload)
            .map_err(|e| format!("Failed to serialize test payload: {}", e))?;

        // 走模板渲染路径，与 forward_* 方法保持一致
        let rendered = render_remote_control_template(&config.template, &payload_str);
        self.send_webhook_raw(&config, &rendered).await?;

        Ok("Remote control push test successful".to_string())
    }
}

/// 计算 HMAC-SHA256 签名
fn compute_hmac(secret: &str, data: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take a key of any size");
    mac.update(data.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// 渲染遥控推送模板，替换 {{变量名}} 占位符。
///
/// payload 是原始 JSON 字符串，内含 `timestamp`、`type`、`data` 等字段。
/// 模板中可引用这些字段的任意嵌套路径，如 `{{data.message}}`、`{{data.command}}`。
fn render_remote_control_template(template: &str, payload: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return payload.to_string(),
    };

    let mut result = template.to_string();

    // 提取顶层和 data 下一级字段
    if let Some(ts) = value["timestamp"].as_str() {
        result = result.replace("{{timestamp}}", ts);
    }
    if let Some(t) = value["type"].as_str() {
        result = result.replace("{{type}}", t);
    }

    // 展开 data 下的所有一级字段
    if let Some(data) = value.get("data") {
        if let Some(obj) = data.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{{data_{}}}}}", key);
                let text = match val {
                    serde_json::Value::String(s) => escape_json_string(s),
                    other => escape_json_string(&other.to_string()),
                };
                result = result.replace(&placeholder, &text);
            }
        }
        // data_message: 优先取 data.message，否则取整个 data 的 JSON 字符串
        let data_message = data["message"]
            .as_str()
            .unwrap_or("");
        result = result.replace("{{data_message}}", &escape_json_string(data_message));
    }

    result
}

/// 转义 JSON 字符串中的特殊字符
fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
