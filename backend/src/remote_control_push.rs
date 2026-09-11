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

        self.send_webhook_raw(&config, payload).await
    }

    /// 发送通话遥控事件推送
    pub async fn forward_call_control(&self, payload: &str) -> Result<(), String> {
        let config = self.get_config();

        if !config.enabled || !config.forward_call_control || config.webhook_url.is_empty() {
            return Ok(());
        }

        self.send_webhook_raw(&config, payload).await
    }

    /// 发送 MQTT 遥控事件推送
    pub async fn forward_mqtt_control(&self, payload: &str) -> Result<(), String> {
        let config = self.get_config();

        if !config.enabled || !config.forward_mqtt_control || config.webhook_url.is_empty() {
            return Ok(());
        }

        self.send_webhook_raw(&config, payload).await
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

    /// 测试远程遥控推送（发送测试消息）
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

        self.send_webhook_raw(&config, &payload_str).await?;

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
