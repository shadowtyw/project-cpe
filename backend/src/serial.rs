/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-07 07:33:11
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:46:14
 * @FilePath: /udx710-backend/backend/src/serial.rs
 * @Description:
 *
 * Copyright (c) 2025 by 1orz, All Rights Reserved.
 */
//! DBus/AT Command Serialization Module
//!
//! This module provides a global lock to serialize all DBus and AT command operations
//! to prevent "org.ofono.Error.InProgress: Operation already in progress" errors.
//!
//! A 30-second timeout protects against ofono hangs: if any D-Bus call exceeds the
//! timeout, the process is aborted.  systemd restarts it automatically, recovering
//! from the hung state without manual intervention.

use std::future::Future;
use std::time::Duration;
use tokio::sync::Mutex;

/// Global mutex to serialize DBus/AT operations
static DBUS_LOCK: Mutex<()> = Mutex::const_new(());

/// Maximum time a single D-Bus call may hold the serial lock.
/// Exceeding this indicates ofono is unresponsive; the process aborts to recover.
const DBUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Execute a future while holding the global DBus lock
///
/// This ensures that only one DBus/AT operation can be in progress at a time,
/// preventing "Operation already in progress" errors from ofono.
///
/// If the operation exceeds `DBUS_TIMEOUT`, the process is aborted because a
/// hung D-Bus call would otherwise block every other operation on the global
/// serial lock indefinitely — causing request pile-up, thread-pool exhaustion,
/// and memory pressure.  systemd restarts the process automatically.
///
/// # Example
/// ```rust
/// let result = with_serial(async {
///     send_at_command(&conn, "AT+CGSN").await
/// }).await;
/// ```
pub async fn with_serial<T, F>(f: F) -> T
where
    F: Future<Output = T>,
{
    with_serial_timeout(DBUS_TIMEOUT, f).await
}

/// Execute a future while holding the global DBus lock, with a configurable timeout.
///
/// [`with_serial`] uses the default [`DBUS_TIMEOUT`]. Long-running but legitimate
/// operations — such as an operator scan (`org.ofono.NetworkRegistration.Scan`),
/// which can take up to ~120s — must pass a longer timeout here, or they would be
/// mistaken for a hung ofono call and abort the whole process.
pub async fn with_serial_timeout<T, F>(timeout: Duration, f: F) -> T
where
    F: Future<Output = T>,
{
    let _guard = DBUS_LOCK.lock().await;
    match tokio::time::timeout(timeout, f).await {
        Ok(result) => result,
        Err(_elapsed) => {
            tracing::error!(
                "D-Bus operation timed out after {}s — aborting process to recover",
                timeout.as_secs()
            );
            std::process::abort();
        }
    }
}