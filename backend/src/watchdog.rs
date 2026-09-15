//! Linux 硬件看门狗 `/dev/watchdog` 喂狗心跳
//!
//! 设备部署在无人值守的 7×24 场景。若后端进程主线程死锁（例如某把 `tokio::sync::Mutex`
//! 永不释放、某个 `.await` 永远不返回）或内核发生 panic，软件层面的 `supervise()`
//! 无能为力——`panic = "abort"` 的 release 构建下连 panic 都直接杀进程，更不用说死锁。
//! Linux 内核提供的 `/dev/watchdog` 在停止喂狗一段时间（典型 15~60s）后会触发整机
//! 硬重启，是软件完全失控后的最后一道冷重启兜底。
//!
//! ## 关键设计
//! - **独立 OS 线程而非 tokio task**（`supervise()` 包裹的一切都是 tokio task）：若
//!   tokio runtime 因死锁 / 卡死而停止调度，`tokio::spawn` 的任务也不会再被轮转，
//!   喂狗随之停止，看门狗形同虚设。专用 `std::thread` 只要内核仍能调度它，就会持续
//!   喂狗，真正覆盖「主线程假死」这一最坏情况。
//! - **静默降级**：`/dev/watchdog` 不存在（容器、非标准内核、未加载看门狗驱动）时
//!   直接退出线程，绝不 panic、绝不写日志刷屏，也不影响主流程。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::Duration;

/// 看门狗设备路径。
const WATCHDOG_DEV: &str = "/dev/watchdog";
/// 喂狗间隔。必须远小于内核看门狗超时（常见 15~60s），留足余量。
const FEED_INTERVAL: Duration = Duration::from_secs(5);

/// 尝试启动硬件看门狗喂狗线程。
///
/// 返回 `true` 表示看门狗已接管并持续喂狗；`false` 表示当前环境没有 `/dev/watchdog`
/// （此时为静默降级，不构成错误）。
pub fn spawn_hardware_watchdog() -> bool {
    let mut fd = match OpenOptions::new().write(true).open(WATCHDOG_DEV) {
        Ok(fd) => fd,
        Err(_) => return false,
    };
    // 打开 fd 即代表接管看门狗：立即喂第一口，避免在首个间隔内就被内核判死。
    if feed(&mut fd).is_err() {
        return false;
    }

    std::thread::Builder::new()
        .name("hw-watchdog".to_string())
        .spawn(move || loop {
            std::thread::sleep(FEED_INTERVAL);
            if feed(&mut fd).is_err() {
                // 写入失败（设备被移除 / 内核重置看门狗）已无法继续喂狗；此刻无法
                // 安全恢复，退出线程让内核看门狗（若仍在计时）触发硬重启兜底。
                // 不做任何日志写入，避免在极端状态下再向磁盘/缓冲追加 IO。
                break;
            }
        })
        .is_ok()
}

/// 向看门狗写入一个字节完成一次喂狗（任意非魔数字节都有效）。
fn feed(fd: &mut File) -> std::io::Result<()> {
    fd.write_all(&[0u8])?;
    fd.flush()?;
    Ok(())
}