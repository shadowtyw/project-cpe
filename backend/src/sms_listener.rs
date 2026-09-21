/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-07 07:33:11
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:46:16
 * @FilePath: /udx710-backend/backend/src/sms_listener.rs
 * @Description: 
 * 
 * Copyright (c) 2025 by 1orz, All Rights Reserved. 
 */
//! SMS Listener Module
//!
//! Listens for incoming SMS via D-Bus signals and stores them in the database.
//!
//! Copyright (c) 2025 1orz
//! https://github.com/1orz/project-cpe

use crate::db::{Database, SmsMessage, CallRecord};
use crate::sms_push::SmsPushSender;
use crate::webhook::WebhookSender;
use std::sync::Arc;
use zbus::{Connection, MessageStream, Proxy};
use zbus::zvariant::OwnedValue;
use futures_util::StreamExt;

/// Start SMS listener with webhook, SMS push, and SMS remote control support.
pub async fn start_sms_listener(
    conn: Connection,
    db: Arc<Database>,
    webhook: Arc<WebhookSender>,
    sms_push: Arc<SmsPushSender>,
    config_manager: Arc<crate::config::ConfigManager>,
) -> zbus::Result<()> {
    // Subscribe to D-Bus signals via proxy
    let dbus_proxy = Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
    
    // Only listen to IncomingMessage signal (ofono auto-assembles long SMS)
    // Note: MessagePDU is not monitored to avoid duplicate SMS notifications
    let rule = "type='signal',sender='org.ofono',interface='org.ofono.MessageManager',member='IncomingMessage'";
    dbus_proxy.call::<_, _, ()>("AddMatch", &(rule,)).await?;
    
    // Create message stream
    let mut stream = MessageStream::from(&conn);
    
    // Listen for signals
    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(error)) => {
                tracing::warn!(error = %error, "D-Bus listener stopped after stream error");
                return Ok(());
            }
            None => {
                tracing::warn!("D-Bus listener stopped because the stream ended");
                return Ok(());
            }
        };
        
        // Check if it's a signal message
        if let Some(member) = msg.header().member() {
            if member.as_str() == "IncomingMessage" {
                // Parse IncomingMessage format (text format)
                if let Ok((content, props)) = msg.body().deserialize::<(String, std::collections::HashMap<String, OwnedValue>)>() {
                    // Extract sender from properties
                    let sender = props.get("Sender")
                        .and_then(|v| v.downcast_ref::<zbus::zvariant::Str>().ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".to_string());

                    // 短信遥控：命中指令且发送者在白名单时，执行动作并回复短信，
                    // 跳过 Webhook/推送转发，避免控制指令泄漏到第三方平台。
                    let is_command = crate::sms_control::handle_incoming_sms(
                        &conn,
                        &config_manager,
                        &sender,
                        &content,
                    )
                    .await;

                    // Store to database (always, for audit trail)
                    match db.insert_sms("incoming", &sender, &content, "received", None) {
                        Ok(id) => {
                            if is_command {
                                // 控制指令：仅入库，不转发 webhook / 推送
                                continue;
                            }

                            // Forward to webhook / SMS push
                            let sms = SmsMessage {
                                id,
                                direction: "incoming".to_string(),
                                phone_number: sender,
                                content,
                                timestamp: crate::utils::beijing_now_rfc3339(),
                                status: "received".to_string(),
                                pdu: None,
                            };
                            let webhook_clone = Arc::clone(&webhook);
                            let sms_push_clone = Arc::clone(&sms_push);

                            // 并行转发 webhook / 推送
                            tokio::spawn(async move {
                                let _ = webhook_clone.forward_sms(&sms).await;
                                let _ = sms_push_clone.forward_sms(&sms).await;
                            });
                        }
                        Err(error) => {
                            crate::log_entry!(warn, "sms", "Failed to store incoming SMS: {}", error);
                        }
                    }
                }
            }
        }
    }
}

/// 活跃通话追踪
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex as StdMutex};
use chrono::Utc;

/// 通话追踪信息
struct ActiveCall {
    db_id: i64,
    phone_number: String,
    direction: String,
    start_time: chrono::DateTime<Utc>,
    answered: bool,
}

/// 通话记录的最大存活时间：超过此时间未收到 CallRemoved 的条目视为孤儿，自动清理。
const ACTIVE_CALL_TTL_SECONDS: i64 = 1800; // 30 minutes

static ACTIVE_CALLS: LazyLock<StdMutex<HashMap<String, ActiveCall>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// 清理超过 TTL 的孤儿通话条目（CallRemoved 信号丢失时防止内存泄漏）。
fn cleanup_stale_calls() {
    let mut active_calls = ACTIVE_CALLS.lock().unwrap_or_else(|p| p.into_inner());
    let now = Utc::now();
    active_calls.retain(|path, call| {
        let age = (now - call.start_time).num_seconds();
        if age > ACTIVE_CALL_TTL_SECONDS {
            crate::log_entry!(warn, "call", "Stale active call cleaned up: path={} number={} age={}s",
                path, call.phone_number, age);
            false
        } else {
            true
        }
    });
}

/// 周期性清理孤儿通话条目。
///
/// `cleanup_stale_calls` 原本仅在收到 `CallAdded` 时触发，若设备长时间无新通话，
/// 因 `CallRemoved` 信号丢失而残留的条目会一直滞留内存。此独立循环保证即使
/// 通话信号流静默，孤儿条目也能在 TTL 后及时回收，杜绝内存长期泄漏。
pub async fn call_cleanup_loop() {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await; // 消耗首个立即触发，首次清理延迟一个周期
    loop {
        interval.tick().await;
        cleanup_stale_calls();
    }
}

/// Start call status listener with call history recording and webhook support
pub async fn start_call_listener(
    conn: Connection,
    db: Arc<Database>,
    webhook: Arc<WebhookSender>,
    config_manager: Arc<crate::config::ConfigManager>,
) -> zbus::Result<()> {
    // Subscribe to D-Bus signals via proxy
    let dbus_proxy = Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
    
    // Add signal match rules - listen to VoiceCallManager signals
    let rule1 = "type='signal',sender='org.ofono',interface='org.ofono.VoiceCallManager'";
    dbus_proxy.call::<_, _, ()>("AddMatch", &(rule1,)).await?;
    
    // Also listen to VoiceCall property changes
    let rule2 = "type='signal',sender='org.ofono',interface='org.ofono.VoiceCall'";
    dbus_proxy.call::<_, _, ()>("AddMatch", &(rule2,)).await?;
    
    let mut stream = MessageStream::from(&conn);
    
    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(error)) => {
                tracing::warn!(error = %error, "D-Bus listener stopped after stream error");
                return Ok(());
            }
            None => {
                tracing::warn!("D-Bus listener stopped because the stream ended");
                return Ok(());
            }
        };
        
        // Process call-related signals
        if let Some(member) = msg.header().member() {
            let member_str = member.as_str();
            
            match member_str {
                "CallAdded" => {
                    // Parse CallAdded signal: (object_path, properties)
                    if let Ok((path, props)) = msg.body().deserialize::<(zbus::zvariant::ObjectPath, std::collections::HashMap<String, OwnedValue>)>() {
                        let path_str = path.to_string();
                        
                        // Extract phone number from LineIdentification property
                        let phone_number = props.get("LineIdentification")
                            .and_then(|v| v.downcast_ref::<zbus::zvariant::Str>().ok())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Unknown".to_string());
                        
                        // Extract call state
                        let state = props.get("State")
                            .and_then(|v| v.downcast_ref::<zbus::zvariant::Str>().ok())
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                        
                        // Determine direction based on state
                        let direction = if state == "incoming" || state == "alerting" {
                            "incoming"
                        } else {
                            "outgoing"
                        };
                        
                        // Insert call record into database
                        let answered = state == "active";
                        if let Ok(db_id) = db.insert_call(direction, &phone_number, answered) {
                            // 每次新通话到来时清理一次超时孤儿条目，防止 CallRemoved
                            // 信号丢失导致内存泄漏。
                            cleanup_stale_calls();
                            let mut active_calls = ACTIVE_CALLS.lock().unwrap_or_else(|p| p.into_inner());
                            active_calls.insert(path_str.clone(), ActiveCall {
                                db_id,
                                phone_number: phone_number.clone(),
                                direction: direction.to_string(),
                                start_time: Utc::now(),
                                answered,
                            });
                        }

                        // 通话遥控：来电命中白名单时自动接听并开始计时。
                        if direction == "incoming" {
                            let config = config_manager.get_call_control();
                            let _ = crate::call_control::on_incoming_call(
                                &conn,
                                &config,
                                &path_str,
                                &phone_number,
                            )
                            .await;
                        }
                    }
                }
                "CallRemoved" => {
                    // Parse CallRemoved signal: object_path
                    if let Ok(path) = msg.body().deserialize::<zbus::zvariant::ObjectPath>() {
                        let path_str = path.to_string();

                        // 通话遥控：根据通话时长匹配命令，进入 10 秒确认窗口。
                        let call_config = config_manager.get_call_control();
                        crate::call_control::on_call_removed(&conn, &call_config, &path_str).await;

                        let mut active_calls = ACTIVE_CALLS.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(call) = active_calls.remove(&path_str) {
                            // Calculate duration
                            let duration = (Utc::now() - call.start_time).num_seconds();
                            let end_time = crate::utils::beijing_now_rfc3339();

                            // Determine final direction
                            let final_direction = if !call.answered && call.direction == "incoming" {
                                // Missed call
                                let _ = db.mark_call_missed(call.db_id);
                                "missed".to_string()
                            } else {
                                let _ = db.update_call_end(call.db_id, duration, call.answered);
                                call.direction.clone()
                            };

                            // Forward to webhook（起始/结束时刻统一为北京时间，与入库值一致）
                            let call_record = CallRecord {
                                id: call.db_id,
                                direction: final_direction,
                                phone_number: call.phone_number,
                                duration,
                                start_time: call
                                    .start_time
                                    .with_timezone(&crate::utils::beijing_offset())
                                    .to_rfc3339(),
                                end_time: Some(end_time),
                                answered: call.answered,
                            };
                            let webhook_clone = Arc::clone(&webhook);
                            tokio::spawn(async move {
                                let _ = webhook_clone.forward_call(&call_record).await;
                            });
                        }
                    }
                }
                "PropertyChanged" => {
                    // Handle VoiceCall property changes (e.g., state changes to "active")
                    if let Ok((name, value)) = msg.body().deserialize::<(String, OwnedValue)>() {
                        if name == "State" {
                            if let Some(state) = value.downcast_ref::<zbus::zvariant::Str>().ok() {
                                let state_str = state.to_string();
                                
                                // Get call path from message
                                if let Some(path) = msg.header().path() {
                                    let path_str = path.to_string();
                                    
                                    // Update answered status if call becomes active
                                    if state_str == "active" {
                                        let mut active_calls = ACTIVE_CALLS.lock().unwrap_or_else(|p| p.into_inner());
                                        if let Some(call) = active_calls.get_mut(&path_str) {
                                            call.answered = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
