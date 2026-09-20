/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-10 10:09:22
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2026-04-18 20:15:00
 * @FilePath: /udx710-backend/backend/src/state.rs
 * @Description:
 *
 * Copyright (c) 2025 by 1orz, All Rights Reserved.
 */

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::extract::FromRef;
use zbus::Connection;

use crate::config::ConfigManager;
use crate::db::Database;
use crate::models::PingResult;
use crate::remote_control_push::RemoteControlPushSender;
use crate::sms_push::SmsPushSender;
use crate::webhook::WebhookSender;

pub struct FrontendRuntime {
    last_seen: RwLock<Option<Instant>>,
}

impl FrontendRuntime {
    pub fn new() -> Self {
        Self {
            last_seen: RwLock::new(None),
        }
    }

    pub fn mark_seen(&self) {
        *self
            .last_seen
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
    }

    pub fn is_recent(&self, timeout: Duration) -> bool {
        self.last_seen
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some_and(|last_seen| last_seen.elapsed() <= timeout)
    }
}

/// 连通性探测防抖状态：单次 ping 失败不立即上报为断网，
/// 保留上一次的成功值，仅连续 3 次失败后才判定为断开。
pub struct ConnectivityState {
    pub ipv4_failures: RwLock<u32>,
    pub ipv6_failures: RwLock<u32>,
    pub last_ipv4: RwLock<Option<PingResult>>,
    pub last_ipv6: RwLock<Option<PingResult>>,
}

impl ConnectivityState {
    const THRESHOLD: u32 = 3;

    pub fn new() -> Self {
        Self {
            ipv4_failures: RwLock::new(0),
            ipv6_failures: RwLock::new(0),
            last_ipv4: RwLock::new(None),
            last_ipv6: RwLock::new(None),
        }
    }

    /// 处理一次 IPv4 探测结果，返回应对外呈现的值
    pub fn apply_ipv4(&self, result: &PingResult) -> PingResult {
        let mut failures = self.ipv4_failures.write().unwrap_or_else(|p| p.into_inner());
        if result.success {
            *failures = 0;
            *self.last_ipv4.write().unwrap_or_else(|p| p.into_inner()) = Some(result.clone());
            result.clone()
        } else {
            *failures += 1;
            if *failures >= Self::THRESHOLD {
                result.clone()
            } else {
                self.last_ipv4
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone()
                    .unwrap_or_else(|| result.clone())
            }
        }
    }

    /// 处理一次 IPv6 探测结果，返回应对外呈现的值
    pub fn apply_ipv6(&self, result: &PingResult) -> PingResult {
        let mut failures = self.ipv6_failures.write().unwrap_or_else(|p| p.into_inner());
        if result.success {
            *failures = 0;
            *self.last_ipv6.write().unwrap_or_else(|p| p.into_inner()) = Some(result.clone());
            result.clone()
        } else {
            *failures += 1;
            if *failures >= Self::THRESHOLD {
                result.clone()
            } else {
                self.last_ipv6
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone()
                    .unwrap_or_else(|| result.clone())
            }
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub dbus_conn: Arc<Connection>,
    pub database: Arc<Database>,
    pub config_manager: Arc<ConfigManager>,
    pub webhook_sender: Arc<WebhookSender>,
    pub sms_push_sender: Arc<SmsPushSender>,
    pub remote_control_push_sender: Arc<RemoteControlPushSender>,
    pub frontend_runtime: Arc<FrontendRuntime>,
    pub connectivity_state: Arc<ConnectivityState>,
}

impl AppState {
    pub fn new(
        dbus_conn: Arc<Connection>,
        database: Arc<Database>,
        config_manager: Arc<ConfigManager>,
        webhook_sender: Arc<WebhookSender>,
        sms_push_sender: Arc<SmsPushSender>,
        remote_control_push_sender: Arc<RemoteControlPushSender>,
        frontend_runtime: Arc<FrontendRuntime>,
        connectivity_state: Arc<ConnectivityState>,
    ) -> Self {
        Self {
            dbus_conn,
            database,
            config_manager,
            webhook_sender,
            sms_push_sender,
            remote_control_push_sender,
            frontend_runtime,
            connectivity_state,
        }
    }
}

impl FromRef<AppState> for Arc<Connection> {
    fn from_ref(state: &AppState) -> Self {
        state.dbus_conn.clone()
    }
}

impl FromRef<AppState> for Arc<Database> {
    fn from_ref(state: &AppState) -> Self {
        state.database.clone()
    }
}

impl FromRef<AppState> for Arc<ConfigManager> {
    fn from_ref(state: &AppState) -> Self {
        state.config_manager.clone()
    }
}

impl FromRef<AppState> for Arc<WebhookSender> {
    fn from_ref(state: &AppState) -> Self {
        state.webhook_sender.clone()
    }
}

impl FromRef<AppState> for Arc<SmsPushSender> {
    fn from_ref(state: &AppState) -> Self {
        state.sms_push_sender.clone()
    }
}

impl FromRef<AppState> for Arc<RemoteControlPushSender> {
    fn from_ref(state: &AppState) -> Self {
        state.remote_control_push_sender.clone()
    }
}

impl FromRef<AppState> for Arc<FrontendRuntime> {
    fn from_ref(state: &AppState) -> Self {
        state.frontend_runtime.clone()
    }
}

impl FromRef<AppState> for Arc<ConnectivityState> {
    fn from_ref(state: &AppState) -> Self {
        state.connectivity_state.clone()
    }
}

impl FromRef<AppState> for (Arc<Connection>, Arc<Database>) {
    fn from_ref(state: &AppState) -> Self {
        (state.dbus_conn.clone(), state.database.clone())
    }
}
