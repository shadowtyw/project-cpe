feat: 发布版出厂化清理 + 北京时间 + OTA 升级保留用户配置

## 本次改动总览

面向"最终发布 OTA 分发给他人"这一目标，做三件事：
1. **出厂化**：清理所有可能泄露个人信息的硬编码值，杜绝把运行态数据打进镜像。
2. **平滑升级**：保证刷入 OTA 只覆盖程序文件，不覆盖用户已有的配置与短信/通话数据。
3. **防崩溃**：旧配置缺字段时安全回落默认值，进程不崩、前端不白屏。

---

## 一、天气时刻统一为北京时间（UTC+8）

- 问题：设备未配置 `TZ`，`chrono::Local` 回落到 UTC，推送/日志时间比实际慢 8 小时。
- 新增 `backend/src/utils.rs` 的北京时间 helper：`now_beijing_rfc3339()` / `now_beijing_format()` / `to_beijing_rfc3339()`，固定 `+08:00`（无夏令时），不依赖 `Local`。
- 所有"给用户看"的时间戳（推送、短信、通话记录、日志、设备状态报告）全部切换；内部计时与差值计算（通话时长、流量时间戳、定时窗口）仍保留 UTC/Local，展示时再转。
- 修复推送里"🕐 xxx"与"时间: yyy"两条时间同时出现的重复显示。

## 二、隐私清理（出厂化）

- 删除远程遥控里的个人 MQTT Broker（`ssl://lafffe12.ala.cn-hangzhou.emqxsl.cn`）、连接认证用户名/密码、鉴权 token、白名单号码。
- 前端 `RemoteControl.tsx` 初始状态回落为公共 Broker（broker.emqx.io / broker-cn.emqx.io）；`ATConsole.tsx` IMEI 占位符改为示例值。
- 修正测试固件里的真实设备 IMEI `868659060480591` → 示例值 `123456789012345`。
- 确认无硬编码 WiFi 密码、无管理员密码/登录体系、无 URL 内嵌 token；`cbnet` APN 为公开运营商 MCC/MNC 查表，非个人数据。
- `.gitignore` 覆盖扩展：新增 `*.corrupt`（解析失败备份）、`net_health_state.json`、`userdata/**/mode*.cfg`，确保设备运行态数据绝不入库。

## 三、OTA 升级保留用户配置（打包→上传→安装三层确认）

- 打包：`pack-ota.sh` 只打 `meta.json + udx710 + www/`，顶层多任何一项即硬失败。
- 上传：后端只放行 `meta.json`、`udx710`、`www/` 三个顶层入口；拒绝路径穿越、符号链接、绝对路径。
- 安装：`ota.rs` 的 `install_update` rename 切换只作用于 `/home/root/udx710` 与 `/home/root/www`，唯一副作用是设备无启动脚本时补空 `init.sh` 占位，与 `/data/config.json`、`/data/data.db` 无关，二者永不读取/改写。
- 全链路复盘确认：`config.json` 与 `data.db` 只存在于设备 `/data` 分区，属私有运行态数据，源码与 OTA 产物均不包含。

## 四、兼容性与增量合并（防崩溃）

- 为 7 个缺失 `#[serde(default)]` 的字段补齐兜底：`WebhookConfig`（enabled/url/forward_sms/forward_calls）、`SmsPushConfig.enabled`、`ScheduleEntry`（time/action）。
- 解析失败兜底：`ConfigManager::new` 先备份原文件为 `config.json.corrupt`，再回落 `AppConfig::default()`，不再静默清空用户配置。
- 新增 8 个回归测试（含 `unknown_future_fields_are_ignored_not_rejected` 锁死禁止 `deny_unknown_fields`），测试数 105 → 全部通过。
- 前端确认无白屏路径：每个配置加载都有空值兜底 + 独立 try/catch；后端 `sanitize()` 保证返回非空 `nodes`/主题。

## 五、README 完善

- 修正"可用内存上限 100%"、公共 Broker 节点数描述等与实际不符处。
- 新增「升级保留用户配置」与「配置安全：解析失败兜底」小节，明确发布级承诺。
- 开源协议补充分发时的署名保留与 GPLv3 条款说明。

---

## 验证

- `cargo check` 通过，无警告。
- `cargo test`：105 passed; 0 failed。
- 隐私扫描：源码中无个人 broker 域名、无真实手机号、无真实 IMEI、无 webhook key/token。