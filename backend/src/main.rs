/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-11 17:44:29
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:46:08
 * @FilePath: /udx710-backend/backend/src/main.rs
 * @Description: 
 * 
 * Copyright (c) 2025 by 1orz, All Rights Reserved. 
 */
//! Project CPE - UDX710 5G/LTE Module Management Service
//!
//! A backend service for managing UDX710 5G/LTE modules via ofono D-Bus interface.
//! Built with Rust + Axum + zbus.
//!
//! Copyright (c) 2025 1orz
//! GitHub: https://github.com/1orz
//! Project: https://github.com/1orz/project-cpe
//!
//! Licensed under the MIT License.

use anyhow::Result;
use axum::{
    routing::get,
    routing::post,
    Router,
    response::{IntoResponse, Response},
    http::{StatusCode, Uri},
    extract::{DefaultBodyLimit, Request},
    middleware::{self, Next},
};
use clap::Parser;
use std::future::Future;
use std::sync::Arc;
use std::path::{Component, PathBuf};
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use zbus::Connection;

mod call_control;
mod config;
mod db;
mod dbus;
mod handlers;
mod iptables;
mod log_buffer;
mod models;
mod ota;
mod process_monitor;
mod restart;
mod schedule;
mod serial;
mod sms_push;
mod sms_listener;
mod state;
mod traffic;
mod usb_switch;
mod utils;
mod webhook;

use config::{ensure_loader_hooks_init, get_default_config_path, get_persistent_root_dir, ConfigManager};
use dbus::init_data_connection;
use handlers::*;
use db::Database;
use sms_push::SmsPushSender;
use state::{AppState, FrontendRuntime};
use webhook::WebhookSender;

/// 获取二进制文件同级目录下的 www 目录路径
fn get_www_dir() -> PathBuf {
    // 获取当前可执行文件的路径
    let exe_path = std::env::current_exe()
        .expect("Failed to get executable path");
    
    // 获取可执行文件所在目录
    let exe_dir = exe_path.parent()
        .expect("Failed to get executable directory");
    
    // 拼接 www 目录
    exe_dir.join("www")
}

/// Protect management APIs when UDX710_API_TOKEN is configured.
/// Keeping the token optional preserves compatibility for existing devices; production
/// deployments should always set it in the service environment.
async fn api_auth(
    expected_token: Option<String>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/api/")
        || path == "/api/health"
        || request.method() == axum::http::Method::OPTIONS
    {
        return next.run(request).await;
    }

    match expected_token {
        Some(token) => {
            let authorized = request
                .headers()
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.strip_prefix("Bearer ") == Some(token.as_str()));
            if authorized {
                next.run(request).await
            } else {
                (StatusCode::UNAUTHORIZED, "Missing or invalid API token").into_response()
            }
        }
        None => next.run(request).await,
    }
}

/// SPA fallback handler - 对于所有前端路由返回 index.html
async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path();
    if PathBuf::from(path)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return (StatusCode::BAD_REQUEST, "Invalid resource path").into_response();
    }

    if path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "API endpoint not found").into_response();
    }
    
    // 获取 www 目录的绝对路径
    let www_dir = get_www_dir();
    
    // 构建请求文件的完整路径
    let requested_path = if path == "/" { "/index.html" } else { path };
    let file_path = www_dir.join(requested_path.trim_start_matches('/'));
    
    // 如果文件存在，返回文件内容
    if let Ok(content) = tokio::fs::read(&file_path).await {
        // 根据文件扩展名设置正确的 Content-Type
        let content_type = match file_path
            .extension()
            .and_then(|ext| ext.to_str())
        {
            Some("html") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "application/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("svg") => "image/svg+xml",
            Some("ico") => "image/x-icon",
            _ => "application/octet-stream",
        };
        
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, content_type)],
            content
        ).into_response();
    }
    
    // 如果文件不存在，返回 index.html（SPA 路由）
    let index_path = www_dir.join("index.html");
    match tokio::fs::read(&index_path).await {
        Ok(content) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            content
        ).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            format!("index.html not found at {:?}. Please build the frontend first.", index_path)
        ).into_response(),
    }
}

/// UDX710 Backend Service - UDX710 5G/LTE 模块管理服务
#[derive(Parser, Debug)]
#[command(name = "udx710")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 监听端口 (默认: 3000)
    #[arg(short, long, default_value = "3000", env = "PORT")]
    port: u16,

    /// 监听地址 (默认: 0.0.0.0)
    #[arg(short = 'H', long, default_value = "0.0.0.0", env = "HOST")]
    host: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化 tracing 日志框架
    // 通过 RUST_LOG 环境变量控制日志级别，默认为 info
    // fmt layer 输出到终端（进程 stdout），LogBufferLayer 转发到内存环形缓冲，
    // 使“系统日志”页面能看到与进程真实输出一致的运行日志。
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(log_buffer::LogBufferLayer)
        .init();

    // 解析命令行参数
    let args = Args::parse();
    let bind_addr = format!("{}:{}", args.host, args.port);

    // 启动日志写入内存缓冲，确保"系统日志"页面在进程启动后立即有内容可看
    log_entry!(info, "app", "CPE backend v{} starting (commit {})", env!("APP_VERSION"), env!("GIT_COMMIT"));

    // Connect to system D-Bus
    let dbus_conn = Arc::new(Connection::system().await?);
    log_entry!(info, "app", "D-Bus system connection established");
    
    // 创建 SMS 数据库（存储在可执行文件同级目录）
    let db_path = get_persistent_root_dir().join("data.db");
    if !db_path.exists() {
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        {
            let legacy_db_path = exe_dir.join("data.db");
            if legacy_db_path != db_path && legacy_db_path.exists() {
                if let Some(parent) = db_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::copy(&legacy_db_path, &db_path) {
                    Ok(_) => info!(from = ?legacy_db_path, to = ?db_path, "Migrated legacy data.db"),
                    Err(error) => warn!(error = %error, from = ?legacy_db_path, to = ?db_path, "Failed to migrate legacy data.db"),
                }
            }
        }
    }
    log_entry!(info, "app", "Database initialized at {:?}", db_path);
    let app_db = Arc::new(Database::new(db_path)?);
    
    // 初始化配置管理器
    let config_path = get_default_config_path();
    info!(path = ?config_path, "Loading config");
    let config_manager = Arc::new(ConfigManager::new(config_path));

    if let Err(err) = ensure_loader_hooks_init() {
        warn!(error = %err, "Failed to ensure loader bootstrap");
    }
    // 启动时检查并恢复中断的 OTA 安装（进程崩溃/断电导致的中断状态）。
    ota::recover_interrupted_ota();
    // route_test.sh adds the rule after its five-second boot delay. Remove only this
    // known USB-to-cellular DROP rule twice after startup; do not flush firewall tables.
    usb_switch::remove_usb_uplink_drop_rules();
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;
        usb_switch::remove_usb_uplink_drop_rules();
        tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;
        usb_switch::remove_usb_uplink_drop_rules();
    });
    
    // 初始化 Webhook 发送器
    let webhook_sender = Arc::new(WebhookSender::new(Arc::clone(&config_manager)));
    let sms_push_sender = Arc::new(SmsPushSender::new(Arc::clone(&config_manager)));
    let frontend_runtime = Arc::new(FrontendRuntime::new());
    
    // 启动 SMS 监听线程
    {
        let db_clone = Arc::clone(&app_db);
        let webhook_clone = Arc::clone(&webhook_sender);
        let sms_push_clone = Arc::clone(&sms_push_sender);
        tokio::spawn(async move {
            supervise("sms_listener", move || {
                let db_clone = Arc::clone(&db_clone);
                let webhook_clone = Arc::clone(&webhook_clone);
                let sms_push_clone = Arc::clone(&sms_push_clone);
                async move {
                    loop {
                        match Connection::system().await {
                            Ok(conn) => {
                                if let Err(error) = sms_listener::start_sms_listener(
                                    conn,
                                    Arc::clone(&db_clone),
                                    Arc::clone(&webhook_clone),
                                    Arc::clone(&sms_push_clone),
                                )
                                .await
                                {
                                    warn!(error = %error, "SMS listener stopped");
                                }
                            }
                            Err(error) => warn!(error = %error, "Failed to connect SMS listener to system D-Bus"),
                        }
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            })
            .await;
        });
    }

    // 启动电话监听线程（包括通话记录存储）
    {
        let db_clone = Arc::clone(&app_db);
        let webhook_clone = Arc::clone(&webhook_sender);
        let config_manager_clone = Arc::clone(&config_manager);
        tokio::spawn(async move {
            supervise("call_listener", move || {
                let db_clone = Arc::clone(&db_clone);
                let webhook_clone = Arc::clone(&webhook_clone);
                let config_manager_clone = Arc::clone(&config_manager_clone);
                async move {
                    loop {
                        match Connection::system().await {
                            Ok(conn) => {
                                if let Err(error) = sms_listener::start_call_listener(
                                    conn,
                                    Arc::clone(&db_clone),
                                    Arc::clone(&webhook_clone),
                                    Arc::clone(&config_manager_clone),
                                )
                                .await
                                {
                                    warn!(error = %error, "Call listener stopped");
                                }
                            }
                            Err(error) => warn!(error = %error, "Failed to connect call listener to system D-Bus"),
                        }
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            })
            .await;
        });
    }

    // 通话遥控轮询：周期检查白名单来电接通计时是否到期，到期执行动作。
    {
        let conn_clone = Arc::clone(&dbus_conn);
        let config_manager_clone = Arc::clone(&config_manager);
        tokio::spawn(async move {
            supervise("call_control_poll", move || {
                let conn_clone = Arc::clone(&conn_clone);
                let config_manager_clone = Arc::clone(&config_manager_clone);
                async move {
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        let config = config_manager_clone.get_call_control();
                        if config.enabled {
                            crate::call_control::poll(&conn_clone, &config).await;
                        }
                    }
                }
            })
            .await;
        });
    }
    
    // 自动初始化数据连接
    {
        let conn_clone = Arc::clone(&dbus_conn);
        tokio::spawn(async move {
            // 等待 2 秒让 modem 完全初始化
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            let result = init_data_connection(&conn_clone).await;
            tracing::info!("Auto-connect completed: {}", result);
        });
    }
    
    // 启动数据连接 Watchdog（每 15 秒检查一次）
    {
        let conn_clone = Arc::clone(&dbus_conn);
        let config_manager = Arc::clone(&config_manager);
        let frontend_runtime = Arc::clone(&frontend_runtime);
        tokio::spawn(async move {
            supervise("data_connection_watchdog", move || {
                let conn_clone = Arc::clone(&conn_clone);
                let config_manager = Arc::clone(&config_manager);
                let frontend_runtime = Arc::clone(&frontend_runtime);
                async move {
                    // 初始延迟 5 秒，等待系统稳定
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    tracing::info!("Watchdog started");
                    log_entry!(info, "app", "Data connection watchdog started");
                    dbus::data_connection_watchdog(conn_clone, config_manager, frontend_runtime).await;
                }
            })
            .await;
        });
    }

    // 自动重启策略默认关闭；任务只在服务启动时创建一次。
    {
        let config_manager = Arc::clone(&config_manager);
        tokio::spawn(async move {
            supervise("restart_watchdog", move || {
                let config_manager = Arc::clone(&config_manager);
                async move {
                    restart::restart_watchdog(config_manager).await;
                }
            })
            .await;
        });
    }

    // 定时计划 watchdog（默认无计划项，不改变既有行为）
    {
        let conn_clone = Arc::clone(&dbus_conn);
        let config_manager = Arc::clone(&config_manager);
        tokio::spawn(async move {
            supervise("schedule_watchdog", move || {
                let conn_clone = Arc::clone(&conn_clone);
                let config_manager = Arc::clone(&config_manager);
                async move {
                    schedule::schedule_watchdog(conn_clone, config_manager).await;
                }
            })
            .await;
        });
    }

    // 流量统计与用量预警 watchdog（低频采样，晚 60 秒启动避免与启动扫描抢占）
    {
        let db_clone = Arc::clone(&app_db);
        let config_manager = Arc::clone(&config_manager);
        tokio::spawn(async move {
            supervise("traffic_watchdog", move || {
                let db_clone = Arc::clone(&db_clone);
                let config_manager = Arc::clone(&config_manager);
                async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                    traffic::traffic_watchdog(db_clone, config_manager).await;
                }
            })
            .await;
        });
    }

    // 数据库定期清理：短信/通话记录只保留最近 N 条。设备 flash 容量有限，
    // 若不清理，data.db 会无限增长直至占满 /data 分区，导致服务无法写库。
    {
        let db_clone = Arc::clone(&app_db);
        tokio::spawn(async move {
            supervise("db_cleanup", move || {
                let db_clone = Arc::clone(&db_clone);
                async move {
                    // 延迟启动，避免与启动阶段的读库扫描抢占。
                    tokio::time::sleep(tokio::time::Duration::from_secs(120)).await;
                    loop {
                        const SMS_KEEP: i64 = 2000;
                        const CALL_KEEP: i64 = 1000;
                        if let Err(e) = db_clone.cleanup_old_sms(SMS_KEEP) {
                            crate::log_entry!(warn, "db", "SMS history cleanup failed: {}", e);
                        }
                        if let Err(e) = db_clone.cleanup_old_calls(CALL_KEEP) {
                            crate::log_entry!(warn, "db", "Call history cleanup failed: {}", e);
                        }
                        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                    }
                }
            })
            .await;
        });
    }

    // Static frontend and API share one origin in production. Cross-origin access
    // is disabled unless the deployment explicitly adds an origin policy.
    let cors = CorsLayer::new();

    let api_token = std::env::var("UDX710_API_TOKEN").ok().filter(|token| !token.trim().is_empty());

    // 创建统一的应用状态
    let app_state = AppState::new(
        dbus_conn,
        app_db,
        config_manager,
        webhook_sender,
        sms_push_sender,
        frontend_runtime,
    );

    // Build routes - 使用统一的 AppState
    let app = Router::new()
        // ========== AT 指令接口 ==========
        .route("/api/at", post(post_at_command).options(options_handler))
        // ========== 设备信息接口 ==========
        .route("/api/device", get(get_device_info).options(options_handler))
        .route("/api/device/imeisv", get(get_imeisv_handler).options(options_handler))
        // ========== SIM 卡接口 ==========
        .route("/api/sim", get(get_sim_info).options(options_handler))
        .route("/api/sim/slot", get(get_sim_slot_handler).options(options_handler))
        .route("/api/sim/slot/switch", post(switch_sim_slot_handler).options(options_handler))
        // ========== 网络接口 ==========
        .route("/api/network", get(get_network_info).options(options_handler))
        .route("/api/network/interfaces", get(get_network_interfaces_info).options(options_handler))
        .route("/api/network/signal-strength", get(get_signal_strength_handler).options(options_handler))
        .route("/api/network/nitz", get(get_nitz_handler).options(options_handler))
        .route("/api/network/operators", get(get_operators_handler).options(options_handler))
        .route("/api/network/operators/scan", get(scan_operators_handler).options(options_handler))
        .route("/api/network/register-manual", post(register_operator_manual_handler).options(options_handler))
        .route("/api/network/register-auto", post(register_operator_auto_handler).options(options_handler))
        // ========== 小区信息接口 ==========
        .route("/api/cells", get(get_cells).options(options_handler))
        .route("/api/location/cell-info", get(get_cell_location_info).options(options_handler))
        // ========== QoS 接口 ==========
        .route("/api/qos", get(get_qos_info).options(options_handler))
        // ========== 数据连接接口 ==========
        .route("/api/data", get(get_data_status).post(set_data_status).options(options_handler))
        .route("/api/roaming", get(get_roaming_status_handler).post(set_roaming_status_handler).options(options_handler))
        .route("/api/airplane-mode", get(get_airplane_mode_handler).post(set_airplane_mode_handler).options(options_handler))
        // ========== 射频模式接口 ==========
        .route("/api/radio-mode", get(get_radio_mode_handler).post(set_radio_mode_handler).options(options_handler))
        .route("/api/band-lock", get(get_band_lock_handler).post(set_band_lock_handler).options(options_handler))
        .route("/api/cell-lock", get(get_cell_lock_handler).post(set_cell_lock_handler).options(options_handler))
        .route("/api/cell-lock/unlock-all", post(unlock_all_cells_handler).options(options_handler))
        // ========== APN 管理接口 ==========
        .route("/api/apn", get(get_apn_list_handler).post(set_apn_handler).options(options_handler))
        // ========== 电话功能接口 ==========
        .route("/api/calls", get(get_calls_handler).options(options_handler))
        .route("/api/call/dial", post(dial_call_handler).options(options_handler))
        .route("/api/call/hangup", post(hangup_call_handler).options(options_handler))
        .route("/api/call/hangup-all", post(hangup_all_calls_handler).options(options_handler))
        .route("/api/call/answer", post(answer_call_handler).options(options_handler))
        .route("/api/call/volume", get(get_call_volume_handler).post(set_call_volume_handler).options(options_handler))
        .route("/api/call/forwarding", get(get_call_forwarding_handler).post(set_call_forwarding_handler).options(options_handler))
        .route("/api/call/settings", get(get_call_settings_handler).post(set_call_settings_handler).options(options_handler))
        .route("/api/call/history", get(get_call_history_handler).options(options_handler))
        .route("/api/call/history/{id}", axum::routing::delete(delete_call_history_handler).options(options_handler))
        .route("/api/call/history/clear", post(clear_call_history_handler).options(options_handler))
        // ========== 短信功能接口 ==========
        .route("/api/sms/send", post(send_sms_handler).options(options_handler))
        .route("/api/sms/list", get(get_sms_list_handler).options(options_handler))
        .route("/api/sms/conversation", get(get_sms_conversation_handler).options(options_handler))
        .route("/api/sms/stats", get(get_sms_stats_handler).options(options_handler))
        .route("/api/sms/clear", post(clear_sms_handler).options(options_handler))
        // ========== IMS/VoLTE 接口 ==========
        .route("/api/ims/status", get(get_ims_status_handler).options(options_handler))
        .route("/api/voicemail/status", get(get_voicemail_status_handler).options(options_handler))
        // ========== USB 模式接口 ==========
        .route("/api/usb-mode", get(get_usb_mode).post(set_usb_mode).options(options_handler))
        .route("/api/usb-advance", post(set_usb_mode_advanced).options(options_handler))
        // ========== 系统接口 ==========
        .route("/api/stats", get(get_system_stats).options(options_handler))
        .route("/api/system/memory-processes", get(get_memory_processes).options(options_handler))
        .route("/api/stats/cpu", get(get_cpu_info).options(options_handler))
        .route("/api/connectivity", get(get_connectivity_check).options(options_handler))
        .route("/api/system/reboot", post(system_reboot).options(options_handler))
        .route("/api/restart/config", get(get_restart_config_handler).post(set_restart_config_handler).options(options_handler))
        .route("/api/health", get(health_check))
        // ========== init.sh 管理接口 ==========
        .route("/api/init-script", get(get_init_script_handler).post(set_init_script_handler).options(options_handler))
        // ========== Webhook 配置接口 ==========
        .route("/api/webhook/config", get(get_webhook_config_handler).post(set_webhook_config_handler).options(options_handler))
        .route("/api/webhook/test", post(test_webhook_handler).options(options_handler))
        // ========== 短信推送配置接口 ==========
        .route("/api/sms-push/config", get(get_sms_push_config_handler).post(set_sms_push_config_handler).options(options_handler))
        .route("/api/sms-push/test", post(test_sms_push_handler).options(options_handler))
        .route("/api/refresh/config", get(get_refresh_config_handler).post(set_refresh_config_handler).options(options_handler))
        .route("/api/refresh/heartbeat", post(frontend_refresh_heartbeat_handler).options(options_handler))
        // ========== OTA 更新接口 ==========
        .route("/api/ota/status", get(get_ota_status_handler).options(options_handler))
        .route("/api/ota/upload", post(upload_ota_handler).options(options_handler)
            .layer(DefaultBodyLimit::max(50 * 1024 * 1024))) // 50MB 限制
        .route("/api/ota/apply", post(apply_ota_handler).options(options_handler))
        .route("/api/ota/rollback", post(rollback_ota_handler).options(options_handler))
        .route("/api/ota/cancel", post(cancel_ota_handler).options(options_handler))
        // ========== 运行日志接口 ==========
        .route("/api/logs", get(get_logs_handler).options(options_handler))
        .route("/api/logs/clear", post(clear_logs_handler).options(options_handler))
        // ========== 一键诊断接口 ==========
        .route("/api/diag/report", get(get_diagnostic_report).options(options_handler))
        // ========== 配置备份/恢复接口 ==========
        .route("/api/config/backup/export", get(export_config_handler).options(options_handler))
        .route("/api/config/backup/import", post(import_config_handler).options(options_handler))
        // ========== 流量统计接口 ==========
        .route("/api/traffic/stats", get(get_traffic_stats_handler).options(options_handler))
        .route("/api/traffic/alert", post(set_traffic_alert_handler).options(options_handler))
        // ========== 定时计划接口 ==========
        .route("/api/schedule/config", get(get_schedule_config_handler).post(set_schedule_config_handler).options(options_handler))
        // ========== 通话遥控接口 ==========
        .route("/api/call-control/config", get(get_call_control_config_handler).post(set_call_control_config_handler).options(options_handler))
        .route("/api/call-control/status", get(get_call_control_status_handler).options(options_handler))
        // ========== 统一状态和中间件 ==========
        .with_state(app_state)
        .layer(middleware::from_fn(move |request, next| api_auth(api_token.clone(), request, next)))
        .layer(cors)
        .fallback(spa_fallback);

    // Start server - 显示版权信息
    info!(
        version = env!("APP_VERSION"),
        branch = env!("GIT_BRANCH"),
        commit = env!("GIT_COMMIT"),
        "Project CPE - UDX710 5G/LTE Module Management"
    );
    info!("Copyright (c) 2025 1orz - https://github.com/1orz/project-cpe");

    // 绑定端口，如果被占用则轮询等待（最多 30 秒）
    let listener = bind_with_retry(&bind_addr, 30).await?;
    log_entry!(info, "app", "Server listening on {}", bind_addr);
    // 使用优雅关闭
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// 监督一个长期运行的后台任务：panic 或意外返回时自动重启。
///
/// 之前所有后台 watchdog / 监听器的 `JoinHandle` 都被丢弃，一旦内部 panic，该任务就
/// 永久退出，对应功能（数据连接恢复、自动重启、短信接收等）静默失效直到整机重启。
/// 由 `supervise` 包裹后，panic 会被拦截记录，并在短暂延迟后重新拉起任务。
async fn supervise<F, Fut>(name: &'static str, task: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut restarts = 0u32;
    loop {
        let handle = tokio::spawn(task());
        match handle.await {
            Ok(()) => {
                tracing::warn!(name, restarts, "background task returned unexpectedly; restarting");
            }
            Err(error) => {
                tracing::error!(name, restarts, error = %error, "background task panicked; restarting");
            }
        }
        restarts = restarts.saturating_add(1);
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

/// 绑定端口，如果被占用则轮询等待
async fn bind_with_retry(addr: &str, max_retries: u32) -> Result<tokio::net::TcpListener> {
    use std::time::Duration;
    
    for i in 0..max_retries {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) => {
                if i == 0 {
                    warn!(addr = %addr, "Port busy, waiting for release...");
                }
                if i + 1 < max_retries {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                } else {
                    return Err(anyhow::anyhow!("Failed to bind to {}: {}", addr, e));
                }
            }
        }
    }
    unreachable!()
}

/// 监听 Ctrl+C 和 SIGTERM 信号，用于优雅关闭
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
