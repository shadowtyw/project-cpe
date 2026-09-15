feat: v3.7.1 极限瘦身 + OTA 防变砖 + with_serial 读路径补齐 + 断网重启熔断 + 硬件看门狗

## 本次改动总览

面向「彻底脱敏、防止升级覆盖、极限瘦身减负、7×24 长稳自愈」四条主线，对代码、配置与构建脚本做全局排查与加固。版本号统一升至 `3.7.1`（`VERSION` / `backend/Cargo.toml` / `frontend/package.json` 三处一致）。

---

## 一、隐私与出厂配置彻底脱敏（防泄密）

- 已确认并清理：远程遥控个人 MQTT Broker（`ssl://lafffe12.ala.cn-hangzhou.emqxsl.cn`）、连接鉴权用户名/密码、Webhook 签名 key、白名单号码等硬编码凭据全部移除。
- `config.rs`：`MqttConfig` 默认 `enabled=false`，节点列表回落为公共节点（broker.emqx.io / broker-cn.emqx.io / test.mosquitto.org）；`WebhookConfig`、`SmsPushConfig` 的 Secret/Key/URL 默认全部为空字符串。
- IMEI 一律经 `dbus.rs` 的 D-Bus 动态读取，**代码中不再保留任何测试机真实 IMEI 常量作为兜底**；取不到 IMEI 时回落到占位值 `"unknown"`。
- `.gitignore` 覆盖 `/data/config.json`、`/data/data.db`、`/home/root/data.db`、临时文件、`net_health_state.json`、`*.corrupt`，设备运行态数据永不入库、永不打包。

## 二、OTA 安全隔离与旧配置平滑兼容（防覆盖、防变砖）

- `pack-ota.sh`：OTA tar.gz 顶层只允许 `meta.json`、`udx710`、`www/` 三项，多一项即硬失败；本地运行产生的 `/data/config.json`、`/data/data.db`、`/home/root/data.db`、临时文件一律不进镜像。
- `ota.rs` 内存安全三道闸：
  - 解包总字节上限 `200MB → 64MB`（旧值超过整机 197MB 物理内存，畸形归档可撑满 tmpfs 触发 OOM 变砖）；
  - 解包前用 `statvfs` 检查 `/tmp` 至少剩 32MB，不足直接拒绝（`tmp_free_bytes()`；`statvfs` 不可用时按不阻断降级）；
  - 解包前用 `tar -tvzf` 未压缩大小做 gzip 炸弹前置拦截（`parse_tar_listing_size`，字段窗口收窄到 4 以免误读纯数字文件名）。
- 上传/解压/临时目录全部落在 `/tmp`（tmpfs），不写物理根分区；`install_update` 只 rename 切换 `/home/root/udx710` 与 `/home/root/www`，**绝不覆盖 `/data/config.json`、`/data/data.db`**。
- 新增配置字段全部带 `#[serde(default)]` + `sanitize()` 兜底，旧配置任何字段缺失都不解析失败；`deny_unknown_fields` 显式禁止，前向兼容。

## 三、极限体积压缩与运行占用控制（目标 ~2MB）

- `backend/Cargo.toml [profile.release]`：`opt-level="z"` + `lto=true` + `codegen-units=1` + `panic="abort"` + `strip=true`；依赖评审确认无冗余重型 crate（新增仅复用既有 `libc` 做 `statvfs`）。
- `scripts/build.sh`：UPX `--best` → `--ultra-brute --lzma`，并新增 8MB 体积闸门（超限打印告警，正常应落在 2~3MB）。
- `frontend/vite.config.ts`：`sourcemap:false`（生产严禁生成 sourcemap）；顶层 `esbuild.drop: ['console','debugger']` + `legalComments:'none'`；`manualChunks` 拆分 react/mui/query 三组 vendor，`www/` 目标 < 1.5MB。
- 前端图标全部 Named Import（杜绝全量打包图标库）；移除两个未使用依赖 `@mui/x-data-grid`、`swr`（`@mui/x-charts` 保留，用于 Dashboard 折线图）。

## 四、模组并发安全（with_serial 读路径补齐，防 abort）

- 审计确认 `with_serial` 全局串行锁不可重入、30s 超时 `abort()`；据此构建调用嵌套关系图。
- 补齐 8 个「只读查询」的串行包裹：`get_all_apn_contexts`、`get_data_connection_status`、`get_roaming_status`、`get_registration_status`、`get_sim_info_data`、`get_network_info_data`、`get_device_info_data`、`get_airplane_mode`。
- 刻意**不加锁**的辅助函数（避免自死锁 abort）：`find_internet_context`（被锁持有者调用）、`get_qos_info_data`（内部调 `send_at_command`，已加锁）、`ofono_ready`（探测总线守护进程，非 ofono RIL 通道）。
- `device_report.rs` 的 `tokio::join!` 并发写法保留——串行锁会让它们实际排队执行，无害。

## 五、存储寿命与长稳自愈闭环

- **断网重启熔断**（`net_health.rs`）：新增 `disconnect_reboots` 时间戳滑动窗口 + `breaker_until` 休眠截止。1 小时内「因断网触发的重启」累计 3 次即熔断，进入 30 分钟强制休眠；无卡/欠费/盲区环境下不再无限循环重启烧闪存。状态随 `write_state` 跨进程持久化。
- **硬件看门狗**（新增 `watchdog.rs`）：启动时尝试打开 `/dev/watchdog`，每 5 秒喂一次；喂狗跑在**独立 OS 线程**而非 tokio task，即使 tokio runtime 死锁或内核 panic，仍能触发整机冷重启兜底；`/dev/watchdog` 不存在时静默降级，不 panic、不刷日志。
- **OOM 防护**（`config.rs` 的 `DEFAULT_LOADER_SCRIPT`）：启动后台进程后立即 `echo -900 > /proc/$PID/oom_score_adj`，把核心服务钉在低 OOM 优先级，OOM 时优先牺牲其它进程而非核心后台；写负值失败静默忽略。
- 日志仅落内存环形缓冲（`log_buffer.rs` 2000 条，不写磁盘）；高频指标（温度/CPU/网速/信号）内存采集，不写 SQLite；`db.rs` 保持 WAL + `synchronous=NORMAL` + `cache_size=-1024`（约 1MB 页缓存）。
- `net_health.rs` 探活仅 `registered`/`roaming` 且 SIM 就绪时计失败，`searching` 阶段不计入；启动 30s 宽限 + 每级 cooldown。

---

## 验证

- `cargo check` 通过，无警告。
- `cargo test` 通过（config 兼容性回归测试等）。
- 体积门：后端 `opt-level="z"` + LTO + UPX `--ultra-brute` 预期 2~3MB；前端 `sourcemap=false` + console/debugger 剥离 + vendor 拆分。
- 隐私扫描：源码无个人 broker 域名、无真实 IMEI、无 Webhook key/鉴权 token。
- OTA 三层复核：打包只含 `meta.json + udx710 + www/`；上传只放行这三项；安装只替换 `/home/root/udx710` 与 `/home/root/www`，绝不触碰 `/data/config.json`、`/data/data.db`。