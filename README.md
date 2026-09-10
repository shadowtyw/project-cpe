# UDX710 后台管理系统

面向成品 5G CPE / 通讯壳的 Web 管理系统。后端采用 Rust + Axum + zbus，通过 ofono D-Bus 管理 5G/LTE 调制解调器；前端基于 React + Vite + @tanstack/react-query，提供网络、短信、电话、频段、小区、USB、OTA、Webhook 和系统状态管理界面。

> 当前版本：`3.6.0`  
> 目标平台：`aarch64-unknown-linux-musl`（展锐 UDX710 SoC）  
> 授权协议：[GNU GPLv3](LICENSE)

---

## 目录

- [项目结构](#项目结构)
- [技术栈](#技术栈)
- [架构概览](#架构概览)
- [免责声明与兼容性](#免责声明与兼容性)
- [快速开始](#快速开始)
- [API 概览](#api-概览)
- [功能特性](#功能特性)
- [稳定性设计](#稳定性设计)
- [ofono D-Bus 接口](#ofono-d-bus-接口)
- [频段锁定](#频段锁定)
- [OTA 更新](#ota-更新)
- [开发约定](#开发约定)
- [依赖](#依赖)
- [开源协议](#开源协议)

---

## 项目结构

```
project-cpe-main/
├── backend/                    # Rust 后端（Axum + zbus）
│   ├── Cargo.toml              # 依赖与构建配置
│   └── src/
│       ├── main.rs             # 入口：启动服务、初始化后台任务
│       ├── serial.rs           # D-Bus 全局串行化锁（with_serial）
│       ├── dbus.rs             # ofono D-Bus 操作（网络/Modem/通话/短信）
│       ├── db.rs               # SQLite 数据库（短信/通话/流量）
│       ├── config.rs           # 配置管理（JSON 读写、原子写入）
│       ├── models.rs           # API 数据模型与响应类型
│       ├── handlers.rs         # Axum HTTP 路由处理函数
│       ├── sms_listener.rs     # 短信/通话 D-Bus 信号监听 + 通话遥控
│       ├── call_control.rs     # 通话遥控（白名单来电→接听→计时→执行动作）
│       ├── sms_control.rs      # 短信遥控指令（#REBOOT#/#RECONNECT#/#STATUS#）
│       ├── webhook.rs          # Webhook 转发（飞书/自定义）+ HMAC-SHA256
│       ├── sms_push.rs         # 短信推送（Pushplus/Server酱/Pushdeer/Bark/Ntfy）
│       ├── ota.rs              # OTA 更新（tar.gz 校验/安装/回滚/哨兵恢复）
│       ├── usb_switch.rs       # USB 热切换（NCM/ECM/RNDIS via configfs）
│       ├── restart.rs          # 自动重启策略（周期/低内存）
│       ├── schedule.rs         # 定时计划（每天/工作日/每天指定时间执行动作）
│       ├── traffic.rs          # 流量统计与预警
│       ├── net_health.rs       # 外网探活与分级断网自愈（L1/L2/L3 阶梯恢复）
│       ├── band_manager.rs     # 频段/小区锁持久化与开机/重连自动重套
│       ├── log_buffer.rs       # 内存环形日志缓冲（2000条，不写磁盘）
│       ├── state.rs            # 前端运行时状态
│       ├── process_monitor.rs  # 进程内存占用读取
│       ├── iptables.rs         # 防火墙规则辅助
│       └── utils.rs            # 系统工具（/proc 读取、命令执行）
├── frontend/                   # React 前端（Vite + TanStack Query）
│   ├── package.json
│   └── src/
│       ├── main.tsx            # 入口
│       ├── App.tsx             # 路由与布局
│       ├── api/
│       │   ├── index.ts        # API 客户端（含指数退避）
│       │   └── types.ts        # TypeScript 类型定义
│       ├── hooks/
│       │   ├── useAdaptivePolling.ts  # 自适应轮询（可见性感知）
│       │   └── useApi.ts       # 通用 API Hook
│       ├── contexts/
│       │   ├── RefreshContext.tsx     # 刷新配置 Context
│       │   └── ThemeContext.tsx       # 主题 Context
│       ├── lib/
│       │   └── queryClient.ts  # TanStack Query 配置
│       ├── utils/
│       │   ├── carriers.ts     # 运营商工具
│       │   └── theme.ts       # 主题工具
│       ├── components/
│       │   ├── Layout/         # 主布局（Sidebar + TopBar）
│       │   └── ErrorSnackbar.tsx
│       └── pages/
│           ├── Dashboard/      # 仪表盘（10+ 组件）
│           ├── Network.tsx     # 网络管理
│           ├── SMS.tsx         # 短信管理
│           ├── Phone.tsx       # 电话管理
│           ├── DeviceInfo.tsx  # 设备信息
│           ├── Logs.tsx        # 运行日志
│           ├── OtaUpdate.tsx   # OTA 更新
│           ├── Tools.tsx       # 高级工具
│           ├── Configuration.tsx # 系统配置
│           ├── ATConsole.tsx   # AT 指令控制台
│           ├── Terminal.tsx    # 终端
│           ├── InitScript.tsx  # 启动脚本
│           └── MemoryProcesses.tsx # 进程内存
├── scripts/                    # 构建与部署脚本
│   ├── build.sh                # 完整构建（Rust + 前端 + OTA 打包）
│   ├── deploy.sh               # 部署脚本
│   ├── monitor.sh              # 监控脚本
│   ├── pack-ota.sh             # OTA 打包
│   ├── pack-userdata.sh        # 用户数据打包
│   └── setup-env.sh            # 环境配置
├── bruno-api/                  # Bruno API 测试集合（80+ 请求）
├── band.md                     # 频段支持调查报告
├── AGENTS.md                   # 项目代码风格说明
└── README.md
```

## 技术栈

### 后端

| 组件 | 技术 | 用途 |
|------|------|------|
| 异步运行时 | tokio 1.48（rt-multi-thread） | 多线程异步 I/O |
| Web 框架 | axum 0.8 | HTTP 路由、中间件、静态文件 |
| 中间件 | tower-http 0.6 | CORS、SPA fallback |
| D-Bus 客户端 | zbus 5.x（tokio） | ofono 通信 |
| 数据库 | rusqlite 0.32（bundled SQLite） | 短信/通话/流量持久化 |
| HTTP 客户端 | reqwest 0.12（rustls-tls） | Webhook/推送转发 |
| 序列化 | serde / serde_json | JSON 配置与 API 响应 |
| 日志 | tracing + tracing-subscriber | 结构化日志 |
| 加密 | hmac 0.12 + sha2 0.10 + md5 0.7 | Webhook 签名、OTA 校验 |
| 时间 | chrono 0.4 | 时间戳处理 |

### 前端

| 组件 | 技术 | 用途 |
|------|------|------|
| 框架 | React 18 | 组件化 UI |
| 构建 | Vite | 快速开发与构建 |
| 数据获取 | @tanstack/react-query | 缓存、轮询、自动刷新 |
| 路由 | React Router | 页面路由 |
| 样式 | CSS Modules + 自定义主题 | 深色/浅色模式 |
| 状态 | React Context | 刷新间隔、主题切换 |

### 目标平台

- **SoC**：展锐 UDX710（aarch64）
- **系统**：Linux（systemd 管理）
- **二进制**：aarch64-unknown-linux-musl（静态链接）
- **构建优化**：LTO + codegen-units=1 + panic=abort + strip

---

## 架构概览

```
┌─────────────────────────────────────────────────────┐
│                    浏览器 / 管理页面                   │
│           http://设备IP:80  (React SPA)              │
└────────────────────┬────────────────────────────────┘
                     │ HTTP /api/*
┌────────────────────▼────────────────────────────────┐
│              axum HTTP Server (port 80)               │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │ 静态文件  │  │ API 路由  │  │ 可选 Bearer Token │  │
│  │ (www/)  │  │(handlers)│  │     认证           │  │
│  └──────────┘  └────┬─────┘  └───────────────────┘  │
│                     │                                │
│  ┌──────────────────▼────────────────────────────┐   │
│  │              后台任务（supervise 自动重启）      │   │
│  │  ┌──────────┐ ┌────────┐ ┌──────────┐        │   │
│  │  │SMS 监听   │ │通话监听 │ │数据Watchdog│  ...  │   │
│  │  └──────────┘ └────────┘ └──────────┘        │   │
│  └───────────────────────────────────────────────┘   │
│                     │                                │
│  ┌──────────────────▼────────────────────────────┐   │
│  │              with_serial() 全局锁               │   │
│  │      (30s timeout → process::abort 恢复)       │   │
│  └──────────────────┬────────────────────────────┘   │
│                     │                                │
│  ┌──────────────────▼────────────────────────────┐   │
│  │              zbus → ofono D-Bus                │   │
│  │  Modem / NetworkRegistration / ConnectionMgr   │   │
│  │  VoiceCallManager / MessageManager / SimMgr   │   │
│  └──────────────────┬────────────────────────────┘   │
│                     │                                │
│  ┌──────────────────▼────────────────────────────┐   │
│  │       SQLite (WAL + busy_timeout=5s)           │   │
│  │  sms_messages / call_history / traffic_daily   │   │
│  └───────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

核心设计原则：

1. **D-Bus 串行化**：所有 ofono 操作通过 `with_serial` 全局锁串行执行，避免 "Operation already in progress" 错误。30 秒超时后 `process::abort()`，由 systemd 自动重启恢复。
2. **后台任务监督**：8 个后台任务（SMS/通话监听、数据连接 Watchdog、重启策略、定时计划、流量统计、数据库清理）均通过 `supervise()` 封装，panic 或退出后自动重启。
3. **阻塞 I/O 隔离**：文件系统操作、进程执行等阻塞 I/O 使用 `spawn_blocking` 卸载到 tokio 阻塞线程池，不占用 HTTP worker。
4. **锁中毒恢复**：所有 `Mutex`/`RwLock` 获取均使用 `unwrap_or_else(|p| p.into_inner())`，避免锁中毒导致 panic 传播。

---

## 免责声明与兼容性

本项目仅供技术交流和学习使用。调制解调器控制、AT 指令、USB 模式、频段/小区锁定、OTA 与重启都可能中断网络或改变设备状态；使用者应自行备份并承担操作后果。

目前已测试的设备为：

- 华为 5G 通讯壳 P50 / P60 / Mate 系列

其他设备的固件、ofono、USB gadget、内核、启动脚本和路由器兼容性可能不同。请先在备用设备或可恢复环境中测试，保留原始 OTA 包，并确保可通过 ADB、串口或原厂恢复方式救援。

---

## 快速开始

### 完整构建

```bash
./scripts/build.sh
```

带 UPX 压缩（减小二进制体积）：

```bash
./scripts/build.sh --upx
```

### 仅构建前端

```bash
cd frontend
pnpm install
pnpm run build
```

### 部署

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

### 设备上运行

```bash
# 默认启动（80 端口）
/home/root/udx710 -p 80 &

# 启用 API Token 认证
export UDX710_API_TOKEN='请生成一段足够随机的Token'
/home/root/udx710 -p 80 &
```

### 启动顺序

设备启动时，`/home/root/loader.sh` 会依次启动：

```bash
#!/bin/sh
/home/root/ttyd/start.sh &    # 终端服务（可选）
/home/root/udx710 -p 80 &     # 管理后台
sh /home/root/init.sh &       # 用户自定义启动脚本
```

---

## API 概览

所有 API 使用 `/api` 前缀，生产页面与 API 同源。详情和可直接导入的请求样例见 [bruno-api/](bruno-api/)。

### 认证

- 认证默认关闭。启用后除 `GET /api/health` 与 CORS 预检外的所有 `/api/*` 请求均需 `Authorization: Bearer <Token>` 头
- Token 通过环境变量 `UDX710_API_TOKEN` 设置
- 前端 Token 保存在浏览器 `localStorage`，不写入设备

### 响应格式

```json
{
  "status": "ok",
  "message": "success",
  "data": { ... }
}
```

### 基础与网络

| 接口 | 方法 | 说明 |
|---|---|---|
| `/api/health` | GET | 健康检查（无需认证） |
| `/api/device` | GET | 设备信息（IMEI/ICCID/型号） |
| `/api/device/imeisv` | GET | 软件版本号 |
| `/api/sim` | GET | SIM 卡信息 |
| `/api/sim/slot` | GET/POST | SIM 卡槽状态与切换 |
| `/api/network` | GET | 网络注册信息 |
| `/api/network/interfaces` | GET | 网络接口详情 |
| `/api/network/signal-strength` | GET | 信号强度 |
| `/api/network/nitz` | GET | 网络时间 |
| `/api/network/operators` | GET | 运营商列表 |
| `/api/network/operators/scan` | GET | 扫描运营商（约 150s） |
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
| `/api/usb-mode` | GET/POST | USB 模式（需重启） |
| `/api/usb-advance` | POST | 高级 USB 热切换（立即生效） |
| `/api/calls` | GET | 当前通话列表 |
| `/api/call/dial` | POST | 拨打电话 |
| `/api/call/hangup` | POST | 挂断指定电话 |
| `/api/call/hangup-all` | POST | 挂断所有电话 |
| `/api/call/answer` | POST | 接听来电 |
| `/api/call/volume` | GET/POST | 通话音量 |
| `/api/call/forwarding` | GET/POST | 呼叫转移 |
| `/api/call/settings` | GET/POST | 通话设置 |
| `/api/call/history` | GET | 通话记录（分页） |
| `/api/call/history/{id}` | DELETE | 删除指定通话记录 |
| `/api/call/history/clear` | POST | 清空通话记录 |
| `/api/sms/send` | POST | 发送短信 |
| `/api/sms/list` | GET | 短信列表（分页） |
| `/api/sms/conversation` | GET | 短信会话 |
| `/api/sms/stats` | GET | 短信统计 |
| `/api/sms/clear` | POST | 清空短信 |
| `/api/ims/status` | GET | IMS 状态 |
| `/api/voicemail/status` | GET | 语音信箱状态 |
| `/api/sms-control/config` | GET/POST | 短信遥控配置（开关/白名单） |

### 系统、配置与 OTA

| 接口 | 方法 | 说明 |
|---|---|---|
| `/api/stats` | GET | 网速、内存、磁盘、CPU、温度、运行时间 |
| `/api/system/memory-processes` | GET | RSS 占用最高的 10 个进程 |
| `/api/stats/cpu` | GET | CPU 信息 |
| `/api/system/reboot` | POST | 手动重启系统 |
| `/api/restart/config` | GET/POST | 自动重启策略（默认关闭） |
| `/api/refresh/config` | GET/POST | 前端刷新间隔 |
| `/api/refresh/heartbeat` | POST | 前端活动心跳 |
| `/api/at` | POST | 执行 AT 指令（高风险） |
| `/api/init-script` | GET/POST | 管理 `/home/root/init.sh` |
| `/api/webhook/config` | GET/POST | Webhook 配置 |
| `/api/webhook/test` | POST | 测试 Webhook |
| `/api/sms-push/config` | GET/POST | 短信推送配置 |
| `/api/sms-push/test` | POST | 测试短信推送 |
| `/api/ota/status` | GET | 当前版本、待安装包、回滚状态 |
| `/api/ota/upload` | POST | 上传 OTA 包（最大 50 MiB） |
| `/api/ota/apply` | POST | 应用 OTA（低版本需 `allow_downgrade`） |
| `/api/ota/rollback` | POST | 恢复到上一版本 |
| `/api/ota/cancel` | POST | 取消待安装 OTA |
| `/api/logs` | GET | 运行日志（min_level=0-3, limit） |
| `/api/logs/clear` | POST | 清空日志缓冲 |
| `/api/diag/report` | GET | 一键诊断报告 |
| `/api/config/backup/export` | GET | 导出配置备份 |
| `/api/config/backup/import` | POST | 导入配置备份 |
| `/api/traffic/stats` | GET | 流量统计（今日/本月/历史） |
| `/api/traffic/alert` | POST | 流量预警阈值 |
| `/api/schedule/config` | GET/POST | 定时计划配置 |
| `/api/net-health/config` | GET/POST | 外网探活与分级断网自愈配置 |

---

## 功能特性

### Dashboard

- 实时网速、信号强度、连接状态
- 可用内存百分比（Linux `MemAvailable` 语义）
- CPU 温度、磁盘使用、运行时间
- 警告/错误日志计数快捷入口
- 自适应轮询（页面可见时高频，隐藏时降频）

### 网络管理

- 数据连接开关、漫游控制
- 飞行模式、射频模式切换（LTE/NR/自动）
- 频段锁定（LTE + NR，支持位掩码）
- 小区锁定（PCI + EARFCN）
- APN 配置管理
- 运营商手动/自动注册
- **频段/小区锁持久化**：设置后自动写入 config.json，开机和断网重连后自动重套

### 电话与短信

- 拨号、接听、挂断
- 通话音量调节、呼叫转移、通话设置
- 短信收发、对话历史、统计
- 通话记录（含未接来电标记）
- 短信转发（Webhook + 多平台推送）
- 通话遥控（白名单来电→自动接听→计时→执行动作）
- **短信遥控指令**：白名单号码发送 `#REBOOT#` / `#RECONNECT#` / `#STATUS#` 远程控制设备（与通话遥控共享白名单，控制短信不转发到第三方）

### USB 模式

- 配置文件模式切换（永久/临时，需重启生效）
- 高级热切换（NCM/ECM/RNDIS，立即生效，短暂断开 USB）
- 热切换失败自动恢复（重绑 UDC + 重启 adbd + 恢复网络配置）
- 切换后自动验证（UDC 绑定 + usb0 IP 确认）

### OTA 更新

- tar.gz 包校验（路径安全、文件类型、条目数、大小、MD5、aarch64 ELF）
- 原子安装（先写临时路径，再 rename 切换）
- 安装失败自动恢复当前版本
- 自动备份上一版本（支持 Web 页面一键回滚）
- 安装哨兵文件（断电/崩溃后下次启动自动回滚）

### 系统管理

- 运行日志查看器（内存环形缓冲，2000 条，不写磁盘）
- 流量统计与预警（今日/本月/历史，按日聚合）
- 定时计划（每天/指定日期/指定时间执行预设动作）
- 自动重启策略（周期重启 + 低内存重启，默认关闭）
- 一键诊断报告（JSON 导出）
- 配置备份/恢复（JSON 导出/导入）
- AT 指令控制台
- 启动脚本管理（`/home/root/init.sh`）

### 安全

- 可选 Bearer Token API 认证
- 配置响应脱敏（Webhook secret、推送凭据不完整返回）
- Webhook HMAC-SHA256 签名
- 短信推送多平台支持（Pushplus / Server酱 / Pushdeer / Bark / Ntfy）

---

## 稳定性设计

### 1. D-Bus 操作保护

- **全局串行化**：`with_serial` 确保所有 ofono 操作互斥执行
- **超时保护**：默认 30s 超时，超时后 `process::abort()` 由 systemd 重启恢复
- **可配置超时**：长时间操作（如运营商扫描 150s）使用 `with_serial_timeout`

### 2. 后台任务监督

9 个后台任务通过 `supervise()` 封装，panic 或意外退出后自动重启：

| 任务 | 功能 | 重启间隔 |
|------|------|----------|
| `sms_listener` | 监听 D-Bus 短信信号 | 5s |
| `call_listener` | 监听 D-Bus 通话信号 | 5s |
| `call_control_poll` | 通话遥控轮询（1s） | 2s |
| `data_connection_watchdog` | 数据连接保活（15s） | 2s |
| `net_health_watchdog` | 外网探活与分级断网自愈 | 2s |
| `restart_watchdog` | 自动重启策略（60s） | 2s |
| `schedule_watchdog` | 定时计划执行 | 2s |
| `traffic_watchdog` | 流量统计采样（300s） | 2s |
| `db_cleanup` | 数据库记录清理（3600s） | 2s |

### 3. 数据库稳定性

- **WAL 模式**：读写不再互相阻塞，提升并发性能
- **busy_timeout=5s**：写锁冲突时等待而非立即报错
- **synchronous=NORMAL**：平衡性能与安全（target 为嵌入式设备）
- **自动清理**：短信保留 2000 条，通话保留 1000 条，防止 `/data` 分区占满

### 4. 配置安全

- **原子写入**：先写 `.tmp` 再 `rename`，断电不损坏配置文件
- **权限保护**：配置文件自动设置为 `0o600`
- **默认关闭**：自动重启、定时计划、通话遥控等策略默认关闭

### 5. 内存管理

- **环形日志缓冲**：最多 2000 条，不写磁盘，不磨损 flash
- **ACTIVE_CALLS TTL**：孤儿通话条目 30 分钟自动清理，防止内存泄漏
- **可用内存语义**：使用 Linux `MemAvailable` 而非 `MemTotal - MemFree`，反映真实可用内存

### 6. 前端健壮性

- **指数退避**：API 请求连续失败后自动延长轮询间隔（2s→4s→8s→...→60s），避免网络恢复后请求风暴
- **自适应轮询**：页面可见时高频刷新，隐藏时降频，减少设备负载

### 7. 错误恢复

- **锁中毒恢复**：所有 `Mutex`/`RwLock` 获取均处理中毒状态
- **OTA 哨兵恢复**：安装中断后下次启动自动回滚
- **USB 热切换恢复**：切换失败自动重绑 UDC、重启 adbd、恢复网络

### 8. 外网连通性探活与分级断网自愈

针对 ofono 状态显示 `registered` 但实际已断流的"假死"问题，增加独立的外网探活 watchdog：

- **周期 ping 公共 IP**：同时探测 `223.5.5.5`（阿里 DNS）和 `119.29.29.29`（腾讯 DNS）
- **分级恢复阶梯**：
  - **L1**：连续失败 3 次 → 重置数据连接（Active=false→true）
  - **L2**：连续失败 6 次 → 飞行模式复位基带（Online=false→true）
  - **L3**：连续失败 10 次 → 系统重启
- **失败计数持久化**：写入 `net_health_state.json`，进程重启不清零
- **防误判设计**：
  - 仅在 `registered`/`roaming` 状态下才判失败，`searching` 阶段不计入
  - 启动后 30 秒启动宽限
  - 每级动作后进入 cooldown 静默期

### 9. 短信遥控指令

通讯壳无实体按键，Web UI 不可达时提供外部应急通道：

- **共享白名单**：复用通话遥控的 `CallControlConfig.numbers`，两项功能共用同一组管理员号码
- **三条指令**：
  - `#REBOOT#` → 延迟 3 秒重启系统
  - `#RECONNECT#` → 重置数据连接
  - `#STATUS#` → 回复设备运行状态（信号、上网状态、运行时间等）
- **安全隔离**：命中指令的短信仅入库审计，不转发到 Webhook/推送平台
- **独立开关**：`SmsControlConfig.enabled` 控制，与通话遥控互不干扰

### 10. 频段/小区锁持久化与自动重套

解决 4G 物联卡自动模式无信号、频段锁定 NVRAM 不可靠、小区锁定 RAM 态丢失问题：

- **配置持久化**：射频模式、频段锁、小区锁设置后自动写入 `config.json`
- **开机自动重套**：`init_data_connection` 成功后调用 `apply_persisted_locks()`
- **断网重连自动重套**：watchdog 检测到连接恢复后调用 `apply_persisted_locks()`
- **三项配置**：
  - 射频模式（LTE-only / NR-only / auto）
  - 频段锁（AT+SPLBAND）
  - 小区锁（AT+SPFORCEFRQ）
- **容错设计**：每步失败只打 warn 日志，不阻断后续步骤

---

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

监听 D-Bus 信号：

```bash
dbus-monitor --system "sender='org.ofono'"
dbus-monitor --system "destination='org.ofono'"
dbus-monitor --system "interface='org.ofono.MessageManager'"
```

---

## 频段锁定

频段能力、AT 指令格式和实际支持范围受设备固件与运营商影响；以下仅供参考，请以设备实际响应为准。详细调查报告见 [band.md](band.md)。

### 本设备（UDX710）实测支持频段

| 类型 | 频段 |
|------|------|
| LTE FDD | B1, B3, B5, B8 |
| LTE TDD | B39, B41 |
| NR FDD | N1, N3, N28 |
| NR TDD | N41, N77, N78, N79 |

### 常用 AT 指令

```bash
# 查询 LTE 当前锁定
AT+SPLBAND=0

# 查询 NR 当前锁定
AT+SPLBAND=3

# 查询 NR 支持能力
AT+SPLBAND=4

# 查询 LTE 支持能力
AT+SPLBAND=5

# 锁定 LTE B1+B3 (FDD)
AT+SPLBAND=1,0,0,0,0,5,0

# 锁定 NR N78 (TDD)
AT+SPLBAND=2,0,0,256,0

# 解锁所有频段
AT+SPLBAND=1,0,0,0,0,0,0
AT+SPLBAND=2,0,0,0,0
```

---

## OTA 更新

### 包结构

```text
meta.json
udx710          # aarch64 二进制
www/            # 前端静态文件
```

### GitHub Actions 构建

运行条件：

- 手动 **Run workflow**
- 推送 `v*` tag

构建流程：

1. 校验 `VERSION`、`backend/Cargo.toml` 与 `frontend/package.json` 的版本一致
2. 运行 Rust 测试
3. 构建 `aarch64-unknown-linux-musl` 后端
4. 构建前端
5. 生成并自检 `meta.json + udx710 + www/` OTA 包
6. 上传 artifact；推送 tag 时创建 GitHub Release

产物：

```text
udx710-ota-<version>.tar.gz
udx710-ota-<version>.tar.gz.sha256
```

### 安全校验

上传时校验：

- 包大小 ≤ 50 MiB
- 拒绝 ZIP 格式（仅接受 tar.gz）
- 路径安全检查（拒绝 `..`、绝对路径、非允许路径）
- 条目数 ≤ 4096，解压总大小 ≤ 200 MiB，目录深度 ≤ 16
- 禁止符号链接和非普通文件
- 顶层只允许 `meta.json`、`udx710`、`www`
- 二进制必须是 aarch64 ELF
- 二进制和前端 MD5 必须与 `meta.json` 一致
- 架构必须匹配 `aarch64-unknown-linux-musl`
- 支持 `min_version` 最低版本检查

### 安装流程

1. 备份当前版本到 `/home/root/.ota_backup`
2. 写入哨兵文件 `/tmp/ota_installing`
3. 将新文件写入临时路径
4. 将当前文件 rename 到 `.old` 路径
5. 将新文件 rename 到正式路径
6. 发布备份快照
7. 移除哨兵文件，清理旧文件

**安装失败**：自动恢复当前版本（从 `.old` 路径 rename 回来）。

**断电/崩溃恢复**：哨兵文件存在时，下次启动自动从备份或 `.old` 路径恢复。

### OTA 替换范围

**只替换**：

```text
/home/root/udx710
/home/root/www
```

**不替换**：

```text
/usr/sbin/connmand
/etc/route_test.sh
/mnt/data/mode.cfg
/data/config.json
/data/data.db
```

---

## 开发约定

### D-Bus 操作序列化

所有 D-Bus / AT 操作必须通过 `with_serial` 串行执行：

```rust
use crate::serial::with_serial;

pub async fn send_at_command(conn: &Connection, cmd: &str) -> zbus::Result<String> {
    with_serial(async {
        let proxy = Proxy::new(conn, "org.ofono", "/ril_0", "org.ofono.Modem").await?;
        proxy.call("SendAtcmd", &(cmd)).await
    }).await
}
```

长时间操作（如运营商扫描）使用 `with_serial_timeout`：

```rust
use crate::serial::with_serial_timeout;

pub async fn scan_operators(conn: &Connection) -> zbus::Result<OperatorListResponse> {
    with_serial_timeout(std::time::Duration::from_secs(150), async {
        // ... 扫描逻辑
    }).await
}
```

### 阻塞 I/O

文件系统读写、进程执行等阻塞操作使用 `spawn_blocking`：

```rust
let result = tokio::task::spawn_blocking(move || {
    // 阻塞 I/O 操作
}).await?;
```

### API 响应格式

```rust
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub status: String,  // "ok" 或 "error"
    pub message: String,
    pub data: Option<T>,
}
```

### 代码风格

- 新文件使用 TypeScript
- React 优先使用函数组件
- 使用 React Router 管理路由
- Rust 后端遵循标准 Rust 惯例

---

## 依赖

### 运行时依赖

| 库 | 版本 | 用途 |
|----|------|------|
| zbus | 5.x | ofono D-Bus 通信 |
| tokio | 1.48 | 异步运行时（rt-multi-thread） |
| axum | 0.8 | HTTP 框架 |
| rusqlite | 0.32 | SQLite 数据库（bundled） |
| tower-http | 0.6 | HTTP 中间件（CORS、静态文件） |
| reqwest | 0.12 | HTTP 客户端（Webhook/推送） |
| serde / serde_json | 1.0 | JSON 序列化 |
| chrono | 0.4 | 时间处理 |
| hmac / sha2 | 0.12 / 0.10 | Webhook 签名 |
| md5 | 0.7 | OTA 校验 |
| tracing | 0.1 | 结构化日志 |

### 前端依赖

| 库 | 用途 |
|----|------|
| React 18 | UI 框架 |
| Vite | 构建工具 |
| @tanstack/react-query | 数据获取与缓存 |
| React Router | 路由 |

---

## 开源协议

本项目采用 **GNU General Public License v3.0（GPLv3）**。

你可以自由使用、研究、修改和分发本项目；但分发源代码或衍生作品时，必须：

1. 保留版权声明、许可证文本和修改说明
2. 以 GPLv3 公开相应完整源代码
3. 以 GPLv3 发布衍生作品

详见 [LICENSE](LICENSE)。