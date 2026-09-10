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

/// PDU decode result
#[allow(dead_code)]
pub struct PduDecodeResult {
    pub sender: String,
    pub content: String,
    pub is_multipart: bool,
    pub reference: u8,
    pub total_parts: u8,
    pub part_number: u8,
}

/// PDU decoder (improved version)
/// Supports concatenated SMS (with UDH) and regular SMS
#[allow(dead_code)]
pub fn decode_pdu_simple(pdu_hex: &str) -> Option<(String, String)> {
    let result = decode_pdu_full(pdu_hex)?;
    Some((result.sender, result.content))
}

/// Full PDU decode (includes multipart SMS info)
pub fn decode_pdu_full(pdu_hex: &str) -> Option<PduDecodeResult> {
    let pdu = pdu_hex.trim().to_uppercase();
    if pdu.len() < 20 {
        return None;
    }
    
    // 1. Skip SMSC (SMS center)
    let smsc_len = u8::from_str_radix(&pdu[0..2], 16).ok()? as usize;
    let mut pos = 2 + smsc_len * 2;
    
    if pos + 2 > pdu.len() {
        return None;
    }
    
    // 2. PDU type (first byte)
    let pdu_type = u8::from_str_radix(&pdu[pos..pos+2], 16).ok()?;
    let has_udh = (pdu_type & 0x40) != 0; // Check UDHI bit
    pos += 2;
    
    if pos + 2 > pdu.len() {
        return None;
    }
    
    // 3. Sender address length
    let sender_len = u8::from_str_radix(&pdu[pos..pos+2], 16).ok()? as usize;
    pos += 2;
    
    // 4. Sender type
    if pos + 2 > pdu.len() {
        return None;
    }
    pos += 2;
    
    // 5. Sender number (BCD encoded)
    let sender_digits_len = if sender_len % 2 == 0 { sender_len } else { sender_len + 1 };
    if pos + sender_digits_len > pdu.len() {
        return None;
    }
    
    let sender_hex = &pdu[pos..pos+sender_digits_len];
    let sender = decode_bcd_number(sender_hex);
    pos += sender_digits_len;
    
    // 6. PID (1 byte)
    if pos + 2 > pdu.len() {
        return None;
    }
    pos += 2;
    
    // 7. DCS (1 byte) - Data Coding Scheme
    if pos + 2 > pdu.len() {
        return None;
    }
    let dcs = u8::from_str_radix(&pdu[pos..pos+2], 16).ok()?;
    pos += 2;
    
    // 8. Timestamp (7 bytes = 14 hex chars)
    if pos + 14 > pdu.len() {
        return None;
    }
    pos += 14;
    
    // 9. User data length
    if pos + 2 > pdu.len() {
        return None;
    }
    let _ud_len = u8::from_str_radix(&pdu[pos..pos+2], 16).ok()? as usize;
    pos += 2;
    
    // 10. Process user data
    let mut is_multipart = false;
    let mut reference: u8 = 0;
    let mut total_parts: u8 = 1;
    let mut part_number: u8 = 1;
    
    // If UDH (User Data Header) exists, parse it first
    if has_udh && pos + 2 <= pdu.len() {
        let udh_len = u8::from_str_radix(&pdu[pos..pos+2], 16).ok()? as usize;
        pos += 2;
        
        // Parse UDH content
        if udh_len >= 5 && pos + udh_len * 2 <= pdu.len() {
            let udh_data = &pdu[pos..pos + udh_len * 2];
            
            // Check if concatenated SMS (IEI = 0x00)
            if udh_data.len() >= 10 && &udh_data[0..2] == "00" && &udh_data[2..4] == "03" {
                is_multipart = true;
                reference = u8::from_str_radix(&udh_data[4..6], 16).ok()?;
                total_parts = u8::from_str_radix(&udh_data[6..8], 16).ok()?;
                part_number = u8::from_str_radix(&udh_data[8..10], 16).ok()?;
            }
        }
        
        // Skip entire UDH
        pos += udh_len * 2;
    }
    
    // 11. Decode message content
    if pos >= pdu.len() {
        return None;
    }
    
    let user_data = &pdu[pos..];
    
    // Determine encoding based on DCS
    let content = if (dcs & 0x08) != 0 || dcs == 0x08 {
        // UCS2 encoding (UTF-16BE)
        decode_ucs2(user_data).ok()?
    } else if dcs == 0x00 || (dcs & 0xF0) == 0x00 {
        // GSM 7-bit encoding (simplified: try as ASCII)
        decode_ucs2(user_data).unwrap_or_else(|_| {
            // Fallback: treat as hex string
            user_data.to_string()
        })
    } else {
        // Other encodings, try UCS2
        decode_ucs2(user_data).ok()?
    };
    
    Some(PduDecodeResult {
        sender,
        content,
        is_multipart,
        reference,
        total_parts,
        part_number,
    })
}

/// Decode BCD encoded phone number
fn decode_bcd_number(hex: &str) -> String {
    let mut result = String::new();
    for i in (0..hex.len()).step_by(2) {
        if i + 1 < hex.len() {
            let second = &hex[i+1..i+2];
            let first = &hex[i..i+1];
            
            if second != "F" && second != "f" {
                result.push_str(second);
            }
            if first != "F" && first != "f" {
                result.push_str(first);
            }
        }
    }
    result
}

/// Decode UCS2 (UTF-16BE) encoding
fn decode_ucs2(hex: &str) -> Result<String, String> {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i+2], 16).ok())
        .collect();
    
    // UTF-16BE decode
    let utf16_values: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    
    String::from_utf16(&utf16_values)
        .map_err(|e| format!("UTF-16 decode error: {}", e))
}

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
                                timestamp: chrono::Utc::now().to_rfc3339(),
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
use std::sync::Mutex as StdMutex;
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

lazy_static::lazy_static! {
    static ref ACTIVE_CALLS: StdMutex<HashMap<String, ActiveCall>> = StdMutex::new(HashMap::new());
}

/// 清理超过 TTL 的孤儿通话条目（CallRemoved 信号丢失时防止内存泄漏）。
fn cleanup_stale_calls() {
    let mut active_calls = ACTIVE_CALLS.lock().unwrap_or_else(|p| p.into_inner());
    let now = Utc::now();
    active_calls.retain(|_, call| {
        let age = (now - call.start_time).num_seconds();
        if age > ACTIVE_CALL_TTL_SECONDS {
            crate::log_entry!(warn, "call", "Stale active call cleaned up: path={} number={} age={}s",
                "", call.phone_number, age);
            false
        } else {
            true
        }
    });
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
                            let end_time = Utc::now().to_rfc3339();
                            
                            // Determine final direction
                            let final_direction = if !call.answered && call.direction == "incoming" {
                                // Missed call
                                let _ = db.mark_call_missed(call.db_id);
                                "missed".to_string()
                            } else {
                                let _ = db.update_call_end(call.db_id, duration, call.answered);
                                call.direction.clone()
                            };
                            
                            // Forward to webhook
                            let call_record = CallRecord {
                                id: call.db_id,
                                direction: final_direction,
                                phone_number: call.phone_number,
                                duration,
                                start_time: call.start_time.to_rfc3339(),
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
