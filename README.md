# UDX710 后台管理系统

面向成品 5G CPE / 通讯壳的 Web 管理系统。后端采用 Rust、Axum 与 zbus，通过 ofono D-Bus 管理 5G/LTE 调制解调器；前端提供网络、短信、电话、频段、小区、USB、OTA、Webhook 和系统状态管理界面。

> 当前版本：`3.4.5`  
> 授权协议：[GNU GPLv3](LICENSE)

## 免责声明与兼容性

本项目仅供技术交流和学习使用。调制解调器控制、AT 指令、USB 模式、频段/小区锁定、OTA 与重启都可能中断网络或改变设备状态；使用者应自行备份并承担操作后果。

目前已测试的设备为：

- 华为 5G 通讯壳 P50 / P60 / Mate 系列

其他设备的固件、ofono、USB gadget、内核、启动脚本和路由器兼容性可能不同。请先在备用设备或可恢复环境中测试，保留原始 OTA 包，并确保可通过 ADB、串口或原厂恢复方式救援。

## 本次安全性与稳定性更新

以下内容描述相对旧版本的主要变化。

### OTA：从直接替换改为受校验、可恢复的更新流程

| 旧行为 | 当前行为 |
|---|---|
| OTA 包校验主要依赖包内 MD5 和架构文本 | 校验 tar.gz 包结构、路径安全、文件类型、条目数、解压大小、MD5、实际 aarch64 ELF 架构、版本格式与 `min_version` |
| 可接受 ZIP / 提示和实际格式不一致 | 只接受 GitHub Actions 或 `scripts/pack-ota.sh` 生成的 `.tar.gz` 包 |
| 安装中断后可能留下半更新状态 | 先写入临时路径，再切换 live 文件；安装失败时恢复当前二进制和前端 |
| 成功升级后没有可用的本机回滚入口 | 每次成功 OTA 后保留上一版本快照，可从页面或 `/api/ota/rollback` 恢复 |
| 只能默认更新、无法清晰处理旧包 | 新版本直接应用；同版本重装或低版本恢复包必须在页面明确确认，API 需传 `allow_downgrade: true` |
| OTA 上传/安装会在异步请求中直接做阻塞 I/O | 上传解包和安装使用后台阻塞任务，避免长时间占用 HTTP 异步 worker |

OTA 包结构保持不变，仍为：

```text
meta.json
udx710
www/
```

OTA 只替换：

```text
/home/root/udx710
/home/root/www
```

不会替换或写入：

```text
/usr/sbin/connmand
/etc/route_test.sh
/mnt/data/mode.cfg
/data/config.json
/data/data.db
```

因此，如果设备通过修改 `connmand`、`route_test.sh` 或其他系统配置使用了自定义管理网段，例如 `192.168.67.1`，本项目 OTA **不会将其改回默认 IP**。生产前端使用相对 `/api` 地址，直接访问 `http://设备IP` 时，页面会继续请求同一地址的 API。

> **首次升级提示**：旧后端没有本机回滚快照能力。从不支持快照的旧版本首次升级到 `3.4.1` 或更高版本前，务必自行下载并保留旧版 GitHub OTA 包。若新版本仍能打开后台，可上传旧包、确认“降级恢复”并回退；若新二进制完全无法启动，Web 回滚 API 无法访问，应使用保留的 OTA 包和设备恢复手段处理。

### 运行时安全：从开放管理面改为可选 Token 认证

| 旧行为 | 当前行为 |
|---|---|
| 任意可访问设备管理地址的客户端可调用所有 API | 支持可选 Bearer Token 认证 |
| Webhook secret、推送凭据等配置可完整读回 | GET 配置响应会脱敏，空密钥保存时保留原有密钥 |
| 静态文件请求没有明确拒绝父目录路径 | SPA fallback 拒绝 `..` 路径并拒绝未知 `/api/*` fallback |
| API 业务错误大量以 HTTP 200 返回 | OTA 等关键接口使用更明确的 HTTP 状态码 |

认证默认关闭，保持旧设备兼容。若需要启用，在设备启动环境中设置：

```sh
export UDX710_API_TOKEN='请生成一段足够随机的Token'
/home/root/udx710 -p 80 &
```

然后在网页 **系统配置 → 管理 API 认证** 中输入相同 Token。浏览器只将 Token 保存于本地 `localStorage`，不会通过配置页面写入设备。启用后，除 `GET /api/health` 与 CORS 预检外的 `/api/*` 请求均需：

```http
Authorization: Bearer <Token>
```

### 网络、D-Bus 和监听器稳定性

| 旧行为 | 当前行为 |
|---|---|
| 数据连接操作会清空系统全部 iptables/ip6tables 规则 | 不再通过本项目清空系统全局防火墙规则 |
| D-Bus 短信/电话监听流结束或错误时可能持续空转 | 监听任务退出后按间隔重新建立 system D-Bus 连接 |
| 前端刷新逻辑分散，后台页面行为不一致 | Dashboard 使用可见性与自适应轮询；刷新配置经后端持久化 |
| USB 热切换回退地址固定为 `192.168.66.1` | 热切换优先读取当前 `usb0` IPv4，随后读取 `UDX710_USB_IP`，最后才回退到 `192.168.67.1` |

> **USB 注意事项**：OTA 与普通 `reboot` 不会执行高级 USB 热切换。高级热切换会重建 gadget、解绑/重绑 UDC 并短暂断开 USB，建议只在了解设备固件与路由器兼容性的情况下手动使用。多台同类 CPE 同时接入同一路由器时，不要同时重启或热切换，以避免 USB 枚举顺序变化。

### 内存：从易误解的“已用”改为 Linux 可用内存语义

旧页面采用：

```text
已用 = MemTotal - MemAvailable
```

这会把 Linux 可回收文件缓存混入“已用”，容易误认为应用程序占满内存。

现在读取 `/proc/meminfo` 中的：

```text
MemTotal
MemFree
MemAvailable
Buffers
Cached
SReclaimable
Shmem
```

Dashboard 主百分比改为：

```text
可用内存百分比 = MemAvailable / MemTotal × 100
```

并显示：

- **可用内存**：优先使用内核 `MemAvailable`，适合判断真实内存压力；
- **占用（近似）**：进程及不可回收内核占用的近似值，不等同于精确 RSS；
- **可回收缓存**：文件缓存、可回收 slab 和缓冲区，Linux 需要内存时通常可以回收；
- **原始空闲和共享内存**：用于进一步诊断。

若旧内核没有 `MemAvailable`，系统使用保守兼容估算并在 API 中标记 `available_estimated`，页面会提示当前值为估算。

### 自动重启：新增持久化的周期与低可用内存策略

新增配置页面：**系统配置 → 自动重启**，配置保存于：

```text
/data/config.json
```

不会被 OTA 覆盖，默认全部关闭。

| 配置 | 范围 | 语义 |
|---|---:|---|
| 连续运行天数自动重启 | 1–365 天 | 从本次开机起按 uptime 计算，不依赖设备日历时间 |
| 可用内存阈值自动重启 | 5%–50% | 按 `MemAvailable / MemTotal` 判断，不按缓存或旧的已用率判断 |

保护措施：

- 每 60 秒检查一次；
- 低内存必须连续 3 次低于阈值才触发；
- 启动后前 10 分钟不触发低内存重启；
- OTA 暂存目录存在时不自动重启；
- 同一时刻只允许一个重启请求；
- 低内存自动重启在 24 小时内最多一次，避免阈值误设造成循环。

期望的组合策略为：

```text
一个开机周期内：
  低可用内存先发生 → 低内存重启，周期结束
  未发生低内存 → 运行达到 N 天后周期重启，周期结束
  系统重启后 → 新周期重新开始
```

重启会中断网络、USB、通话和当前数据传输。建议先在备用设备测试，初始生产值可设为：

```text
周期：7 天
低可用内存阈值：10%
```

## 本轮稳定性修复（v3.4.5 及更高）

以下为最近一轮针对长期运行的稳定性修复说明。

### USB 热切换失败自动恢复

| 旧行为 | 当前行为 |
|---|---|
| 高级热切换在异步 worker 中同步执行，切换期间管理页面可能卡顿无响应 | 热切换放入 `spawn_blocking` 后台线程，不阻塞 HTTP worker |
| 切换中途失败会留下“UDC 已禁用、未重绑”的半配置 gadget，可能造成 usb0 掉线、只能物理重启 | 切换失败后尽力自动重绑 UDC、重启 adbd、恢复 USB 网络配置，保住管理链路 |

> 自动恢复是 best-effort 尽力而为，无法 100% 覆盖所有硬件异常；仍建议在了解设备固件与路由器兼容性的前提下手动使用热切换。

### 短信与通话记录自动清理（防 flash 占满）

| 旧行为 | 当前行为 |
|---|---|
| `cleanup_old_sms` 未接线，通话记录无清理入口，`data.db` 会无限增长直至占满 `/data` 分区 | 新增每小时清理任务，短信保留最近 2000 条、通话保留最近 1000 条 |

该清理只删除最旧记录，不影响最新数据，保证设备长期运行时 flash 不会被历史记录耗尽。

### 定时计划去重与跨零点窗口

| 旧行为 | 当前行为 |
|---|---|
| 去重槽位单一，同一分钟内的不同计划项会互相覆盖，可能漏触发或重复触发 | 去重集合按（时间+动作）独立记录，同分钟多个计划项互不干扰 |
| 时间差按线性 `abs_diff` 计算，23:59 与 00:00 被误判为相差 1439 分钟 | 时间差按“环形分钟”计算，正确识别跨零点窗口 |

### Webhook 与短信推送修正

| 旧行为 | 当前行为 |
|---|---|
| Webhook 模板仅对短信内容转义，号码/时间含引号或换行会产生非法 JSON | 短信与通话模板的所有字符串字段统一做 JSON 转义 |
| Server酱（sctapi.ftqq.com）误传已废弃的 `text` 字段，正文可能丢失 | 调整为只发送 `title` + `desp`（支持 Markdown 正文） |

---

## v3.4.5 新功能

本次版本在不影响设备长期稳定运行和性能的前提下，新增以下可玩性与可用性功能。

### 运行日志查看器

| 功能 | 说明 |
|---|---|
| 内存环形缓冲 | 保留最近 2000 条日志，不写磁盘，重启后清空 |
| 日志分级筛选 | 支持 debug/info/warn/error 四级，默认显示 info 及以上 |
| Dashboard 快捷入口 | 首页底部显示警告/错误计数，点击跳转完整日志页 |
| API 访问 | `GET /api/logs?min_level=1&limit=200` |

访问 **系统日志** 页面查看实时运行日志。日志仅在内存中保留，不会对设备 flash 造成磨损，适合长期运行场景。

### 流量统计与预警

| 功能 | 说明 |
|---|---|
| 实时流量监控 | 每 300 秒（5 分钟）低频采样一次，按日聚合写入 SQLite |
| 今日/本月统计 | Dashboard 和 API 可查看下行/上行字节数 |
| 阈值预警 | 超过每日阈值时记录到日志（未来可扩展 Webhook/推送） |
| 历史记录 | 保留最近 30 天流量数据（实际表结构保留全部天数，页面展示最近 30 天） |

访问 **高级工具 → 流量统计** 查看。流量数据写入 `data.db`（`traffic_daily` 表，与短信/通话记录同库），OTA 不会覆盖。

### 定时计划

| 功能 | 说明 |
|---|---|
| 定时执行 | 支持指定时间自动执行预设动作 |
| 预设动作 | 飞行模式开关、数据连接开关、LTE/NR 切换、设备重启 |
| 灵活调度 | 可选每天/工作日/周末，时间精度到分钟 |
| 配置持久化 | 计划保存在 `/data/config.json`，OTA 后保留 |

访问 **高级工具 → 定时计划** 添加定时任务。所有计划默认关闭，不会影响设备原有行为。

### 短信遥控

| 功能 | 说明 |
|---|---|
| 远程控制 | 收到特定前缀短信时执行预设动作 |
| 前缀匹配 | 默认匹配 `#` 开头的短信，可自定义 |
| 预设动作 | 设备重启、飞行模式开关、数据连接开关 |
| 安全机制 | 支持密码验证（可选），防止误触发 |

访问 **高级工具 → 短信遥控** 配置。例如发送 `#reboot` 短信可触发设备重启（需先启用并配置前缀）。适合无网络环境下的远程管理。

### 一键诊断与配置备份

| 功能 | 说明 |
|---|---|
| 诊断报告 | 汇总设备信息、网络状态、内存/磁盘/CPU 使用率、近期日志 |
| JSON 导出 | 生成 `diagnostics.json` 文件，便于排查问题 |
| 配置导出 | 导出 `/data/config.json` 用于备份 |
| 配置导入 | 导入配置后重启生效（OTA 后可恢复原有配置） |

访问 **高级工具 → 诊断与备份** 使用。诊断报告包含设备运行时的关键指标，适合提交给社区或开发者排查问题。

### 性能优化

| 优化项 | 旧行为 | 新行为 |
|---|---|---|
| 系统状态采样 | `/api/stats` 在异步任务中同步读取 `/proc`，阻塞 worker | 使用 `spawn_blocking` 将阻塞 I/O 移出 tokio worker |
| 连通性检测 | `/api/connectivity` 同步执行 ping 命令，最长阻塞 4 秒 | 使用 `spawn_blocking` 避免阻塞 |
| CPU 信息 | `/api/cpu` 同步读取 `/proc/cpuinfo` | 使用 `spawn_blocking` |
| 流量采样 | 新增 `/api/traffic/stats` | 低频采样（300s），写入 `data.db` 的 `traffic_daily` 表，不阻塞请求 |

### 错误处理改进

| 改进项 | 说明 |
|---|---|
| 锁中毒恢复 | `db.rs`、`state.rs`、`sms_listener.rs`、`config.rs` 中所有 Mutex/RwLock 获取均改为 `unwrap_or_else(poisoned.into_inner())`，避免 panic 传播 |
| 静默失败记录 | 短信/通话记录写入失败时写入运行日志，便于排查 |
| Webhook/推送失败 | 失败时记录到运行日志，不再静默丢弃 |

## GitHub OTA 构建与升级

GitHub Actions 在以下任一情形运行：

- 手动 **Run workflow**；
- 推送 `v*` tag。

构建流程会：

1. 校验 `VERSION`、`backend/Cargo.toml` 与 `frontend/package.json` 的版本一致；
2. 运行 Rust 测试；
3. 构建 `aarch64-unknown-linux-musl` 后端；
4. 构建前端；
5. 生成并自检 `meta.json + udx710 + www/` OTA 包；
6. 上传 artifact；推送 tag 时创建 GitHub Release。

产物：

```text
udx710-ota-<version>.tar.gz
udx710-ota-<version>.tar.gz.sha256
```

首次进行长期稳定性测试时，建议在手动工作流中将 **Build with UPX compression** 设为 `false`，减少一个运行时解压变量。稳定验证后再根据体积需求选择是否使用 UPX。

### 推荐升级步骤

1. 下载并保存当前正常运行的 OTA 包；
2. 保留 IP 修改涉及的文件备份，例如：

   ```sh
   adb pull /usr/sbin/connmand connmand.backup
   adb pull /etc/route_test.sh route_test.sh.backup
   ```

3. 在设备管理页面 **OTA 更新** 上传 GitHub 构建的 `.tar.gz`；
4. 确认校验通过；新版本直接应用，旧版本或同版本包必须明确确认降级；
5. 选择“应用并重启”；
6. 重启后访问：

   ```text
   http://设备IP
   http://设备IP/api/health
   http://设备IP/api/ota/status
   ```

7. 确认管理页面、数据连接、USB 网络和温度正常后，再启用自动重启策略。

## 快速开始

### 后端与完整 OTA 构建

```bash
./scripts/build.sh
```

带 UPX 压缩：

```bash
./scripts/build.sh --upx
```

仅构建前端：

```bash
cd frontend
pnpm install
pnpm run build
```

部署：

```bash
./scripts/deploy.sh
```

### macOS 交叉编译环境

```bash
brew install rust rustup
rustup default stable
rustup target add aarch64-unknown-linux-musl

brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-musl

rustup target list --installed
which aarch64-unknown-linux-musl-gcc
```

## ofono D-Bus 接口

| 接口 | 说明 |
|---|---|
| `org.ofono.Manager` | 调制解调器管理 |
| `org.ofono.Modem` | Modem 属性和控制 |
| `org.ofono.NetworkRegistration` | 网络注册状态 |
| `org.ofono.SimManager` | SIM 卡管理 |
| `org.ofono.ConnectionManager` | 数据连接管理 |
| `org.ofono.VoiceCallManager` | 语音通话管理 |
| `org.ofono.MessageManager` | 短信管理 |

常用命令：

```bash
# 查看 Modem 属性
dbus-send --system --print-reply \
  --dest=org.ofono /ril_0 org.ofono.Modem.GetProperties

# 查看网络状态
dbus-send --system --print-reply \
  --dest=org.ofono /ril_0 org.ofono.NetworkRegistration.GetProperties

# 查看 SIM 卡信息
dbus-send --system --print-reply \
  --dest=org.ofono /ril_0 org.ofono.SimManager.GetProperties

# 设置飞行模式
dbus-send --system --print-reply \
  --dest=org.ofono /ril_0 org.ofono.Modem.SetProperty \
  string:"Online" variant:boolean:false

# 发送 AT 指令
dbus-send --system --print-reply \
  --dest=org.ofono /ril_0 org.ofono.Modem.SendAtcmd \
  string:"AT+CGSN"
```

监听 D-Bus：

```bash
dbus-monitor --system "sender='org.ofono'"
dbus-monitor --system "destination='org.ofono'"
dbus-monitor --system "interface='org.ofono.MessageManager'"
```

## 频段锁定

频段能力、AT 指令格式和实际支持范围受设备固件与运营商影响；以下仅供参考，请以设备实际响应为准。

### LTE（4G）

| 频段 | 位掩码 | 说明 |
|---|---:|---|
| B1 | 1 | FDD 2100MHz |
| B3 | 4 | FDD 1800MHz |
| B5 | 16 | FDD 850MHz |
| B8 | 128 | FDD 900MHz |
| B38 | 32（TDD） | TDD 2600MHz |
| B40 | 128（TDD） | TDD 2300MHz |
| B41 | 256（TDD） | TDD 2500MHz |

### NR（5G）

| 频段 | 位掩码 | 说明 |
|---|---:|---|
| N1 | 1（FDD） | 2100MHz |
| N28 | 512（FDD） | 700MHz |
| N41 | 16（TDD） | 2500MHz |
| N77 | 128（TDD） | 3700MHz |
| N78 | 256（TDD） | 3500MHz |
| N79 | 512（TDD） | 4500MHz |

```text
查询 LTE：AT+SPLBAND=0
查询 NR： AT+SPLBAND=3
锁定 LTE B1+B3：AT+SPLBAND=1,0,0,0,0,5,0
锁定 NR N78：AT+SPLBAND=2,0,0,256,0
解除锁定：
  AT+SPLBAND=1,0,0,0,0,0,0
  AT+SPLBAND=2,0,0,0,0
```

## API 概览

所有 API 使用 `/api` 前缀，生产页面与 API 同源，默认通过设备的 80 端口访问。详情和可直接导入的请求样例见 [bruno-api/](bruno-api/)。

### 基础与网络

| 接口 | 方法 | 说明 |
|---|---|---|
| `/api/health` | GET | 健康检查；启用 Token 后仍无需认证 |
| `/api/device` | GET | 设备信息（IMEI/ICCID/型号） |
| `/api/device/imeisv` | GET | 软件版本号 |
| `/api/sim` | GET | SIM 卡信息 |
| `/api/sim/slot` | GET | SIM 卡槽状态 |
| `/api/sim/slot/switch` | POST | 切换 SIM 卡槽 |
| `/api/network` | GET | 网络注册信息 |
| `/api/network/interfaces` | GET | 网络接口信息 |
| `/api/network/signal-strength` | GET | 信号强度 |
| `/api/network/nitz` | GET | 网络时间 |
| `/api/network/operators` | GET | 运营商列表 |
| `/api/network/operators/scan` | GET | 扫描运营商（耗时） |
| `/api/network/register-manual` | POST | 手动注册运营商 |
| `/api/network/register-auto` | POST | 自动注册运营商 |
| `/api/cells` | GET | 基站信息 |
| `/api/location/cell-info` | GET | 基站定位参数 |
| `/api/qos` | GET | QoS 信息 |
| `/api/connectivity` | GET | 外网连通性检查 |

### 模块、通话与短信

| 接口 | 方法 | 说明 |
|---|---|---|
| `/api/data` | GET/POST | 数据连接开关 |
| `/api/roaming` | GET/POST | 漫游开关 |
| `/api/airplane-mode` | GET/POST | 飞行模式开关 |
| `/api/radio-mode` | GET/POST | 射频模式（4G/5G/自动） |
| `/api/band-lock` | GET/POST | 频段锁定 |
| `/api/cell-lock` | GET/POST | 小区锁定 |
| `/api/cell-lock/unlock-all` | POST | 解锁所有小区 |
| `/api/apn` | GET/POST | APN 配置 |
| `/api/usb-mode` | GET/POST | USB 模式切换，通常需重启 |
| `/api/usb-advance` | POST | 高级 USB 热切换，可能短暂断开 USB |
| `/api/calls` | GET | 当前通话列表 |
| `/api/call/dial` | POST | 拨打电话 |
| `/api/call/hangup` | POST | 挂断指定电话 |
| `/api/call/hangup-all` | POST | 挂断所有电话 |
| `/api/call/answer` | POST | 接听来电 |
| `/api/call/volume` | GET/POST | 通话音量 |
| `/api/call/forwarding` | GET/POST | 呼叫转移 |
| `/api/call/settings` | GET/POST | 通话设置 |
| `/api/call/history` | GET | 通话记录 |
| `/api/call/history/{id}` | DELETE | 删除指定通话记录 |
| `/api/call/history/clear` | POST | 清空通话记录 |
| `/api/sms/send` | POST | 发送短信 |
| `/api/sms/list` | GET | 短信列表 |
| `/api/sms/conversation` | GET | 短信会话 |
| `/api/sms/stats` | GET | 短信统计 |
| `/api/sms/clear` | POST | 清空短信 |
| `/api/ims/status` | GET | IMS 状态 |
| `/api/voicemail/status` | GET | 语音信箱状态 |

### 系统、配置与 OTA

| 接口 | 方法 | 说明 |
|---|---|---|
| `/api/stats` | GET | 网速、准确内存、磁盘、CPU、温度、运行时间与 USB 状态 |
| `/api/system/memory-processes` | GET | 只读返回 RSS 占用最高的 10 个进程 |
| `/api/stats/cpu` | GET | CPU 信息 |
| `/api/system/reboot` | POST | 手动重启系统 |
| `/api/restart/config` | GET/POST | 自动重启策略；默认关闭 |
| `/api/refresh/config` | GET/POST | 前端刷新间隔 |
| `/api/refresh/heartbeat` | POST | 前端活动心跳 |
| `/api/at` | POST | 执行 AT 指令；高风险 |
| `/api/init-script` | GET/POST | 管理 `/home/root/init.sh`；高风险 |
| `/api/webhook/config` | GET/POST | Webhook 配置 |
| `/api/webhook/test` | POST | 测试 Webhook |
| `/api/sms-push/config` | GET/POST | 短信推送配置 |
| `/api/sms-push/test` | POST | 测试短信推送 |
| `/api/ota/status` | GET | 当前版本、待安装包与本机回滚状态 |
| `/api/ota/upload` | POST | 上传 OTA 包，最大 50 MiB |
| `/api/ota/apply` | POST | 应用 OTA；低/同版本需 `allow_downgrade: true` |
| `/api/ota/rollback` | POST | 恢复设备自动保存的上一版本 |
| `/api/ota/cancel` | POST | 取消待安装 OTA |

## 开发约定

### D-Bus 操作序列化

所有 D-Bus / AT 操作必须通过 `with_serial` 串行执行，避免 ofono 同时执行多个操作：

```rust
use crate::serial::with_serial;

pub async fn send_at_command(conn: &Connection, cmd: &str) -> zbus::Result<String> {
    with_serial(async {
        let proxy = Proxy::new(conn, "org.ofono", "/ril_0", "org.ofono.Modem").await?;
        proxy.call("SendAtcmd", &(cmd)).await
    }).await
}
```

### API 响应格式

```rust
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub status: String, // "ok" 或 "error"
    pub message: String,
    pub data: Option<T>,
}
```

## 依赖

- **zbus 5.x**：D-Bus 客户端
- **tokio 1.48**：异步运行时
- **axum 0.8**：Web 框架
- **rusqlite 0.32**：内置 SQLite
- **tower-http 0.6**：HTTP 中间件

## 开源协议

本项目采用 **GNU General Public License v3.0（GPLv3）**。

你可以自由使用、研究、修改和分发本项目；但分发源代码或衍生作品时，必须：

1. 保留版权声明、许可证文本和修改说明；
2. 以 GPLv3 公开相应完整源代码；
3. 以 GPLv3 发布衍生作品。

详见 [LICENSE](LICENSE)。
