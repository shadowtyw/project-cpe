/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-07 07:33:11
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:46:06
 * @FilePath: /udx710-backend/backend/src/iptables.rs
 * @Description: 
 * 
 * Copyright (c) 2025 by 1orz, All Rights Reserved. 
 */
//! iptables 操作模块
//!
//! 提供 iptables 规则检查和清空功能

use std::process::Command;
use tokio::task;

/// 清空所有 iptables 规则（包括 nat 和 mangle 表）
///
/// 执行更完整的清空操作，清空 filter、nat、mangle 表的所有规则
///
/// # Returns
/// * `Ok(())` - 成功清空规则
/// * `Err(String)` - 操作失败的错误信息
#[allow(dead_code)]
pub async fn flush_all_iptables() -> Result<(), String> {
    task::spawn_blocking(|| {
        let tables = ["filter", "nat", "mangle"];
        
        for table in &tables {
            let output = Command::new("iptables")
                .arg("-t")
                .arg(table)
                .arg("-F")
                .output()
                .map_err(|e| format!("Failed to execute iptables for table {}: {}", table, e))?;

            if !output.status.success() {
                // 如果表不存在或不支持，继续处理下一个表（某些表可能不存在）
                // 静默处理，不输出警告
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| format!("Task execution failed: {}", e))?
}

