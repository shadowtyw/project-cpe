# UDX710 前端管理平台

基于 React + TypeScript + MUI 的现代化设备管理界面。

## 🚀 快速开始

### 1. 安装依赖

```bash
# 使用 pnpm (推荐)
pnpm add @mui/icons-material @mui/x-data-grid @mui/x-charts react-router-dom swr

# 或使用 npm
npm install @mui/icons-material @mui/x-data-grid @mui/x-charts react-router-dom swr
```

### 2. 启动开发服务器

```bash
pnpm dev
# 或
npm run dev
```

访问：`http://localhost:5173`

开发代理默认指向 `http://192.168.67.1`。生产前端通过相对 `/api` 请求同一设备，不依赖这个开发地址。

**注意**：需要同时运行后端服务 (Rust)，请在另一个终端中运行：

```bash
cd ..
cargo run --manifest-path backend/Cargo.toml
```

### 3. 生产构建

```bash
pnpm build
# 或
npm run build
```

构建产物输出到 `dist` 目录；仓库根目录的 `scripts/pack-ota.sh` 会将其放入 OTA 包中的 `www/`。

## 📁 项目结构

```text
src/
├── api/              # API 接口层
├── components/       # 可复用组件
├── hooks/            # 自定义 Hooks
├── pages/            # 页面组件
├── theme.ts          # MUI 主题
└── App.tsx           # 路由配置
```

## 🎨 功能特性

- ✅ 实时设备监控仪表盘
- ✅ 设备信息详细展示
- ✅ 网络状态和小区信息
- ✅ USB 模式热切换
- ✅ AT 指令调试控制台
- ✅ 响应式设计

## 🔧 技术栈

- React 19
- TypeScript
- MUI v7
- React Router v7
- Vite

查看 `SETUP.md` 获取详细配置说明。
