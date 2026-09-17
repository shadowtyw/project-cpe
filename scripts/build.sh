#!/bin/bash
###
 # @Author: 1orz cloudorzi@gmail.com
 # @Date: 2025-12-07 07:33:11
 # @LastEditors: 1orz cloudorzi@gmail.com
 # @LastEditTime: 2025-12-13 12:51:00
 # @FilePath: /udx710-backend/scripts/build.sh
 # @Description: 
 # 
 # Copyright (c) 2025 by 1orz, All Rights Reserved. 
### 
# 构建脚本 - 构建后端和前端，自动生成 OTA 包

set -e

# 切换到项目根目录
cd "$(dirname "$0")/.."

# 解析命令行参数
BUILD_BACKEND=true
BUILD_FRONTEND=true
USE_UPX=true  # 默认启用 UPX 压缩
PACK_USERDATA=false
COPY_TO_USERDATA=false
SKIP_OTA=false

for arg in "$@"; do
    case $arg in
        --backend-only)
            BUILD_FRONTEND=false
            ;;
        --frontend-only)
            BUILD_BACKEND=false
            ;;
        --no-upx)
            USE_UPX=false
            ;;
        --no-ota)
            SKIP_OTA=true
            ;;
        --pack)
            PACK_USERDATA=true
            COPY_TO_USERDATA=true
            ;;
        --copy-only)
            COPY_TO_USERDATA=true
            BUILD_BACKEND=false
            BUILD_FRONTEND=false
            ;;
        --help|-h)
            echo "用法: ./scripts/build.sh [选项]"
            echo ""
            echo "选项:"
            echo "  --backend-only   只构建后端"
            echo "  --frontend-only  只构建前端"
            echo "  --no-upx         禁用 UPX 压缩 (默认启用)"
            echo "  --no-ota         跳过 OTA 包生成"
            echo "  --copy-only      只复制构建产物到 userdata (跳过构建)"
            echo "  --pack           构建后复制到 userdata 并打包 UBIFS"
            echo "  --help, -h       显示帮助信息"
            echo ""
            echo "示例:"
            echo "  ./scripts/build.sh                    # 构建 + UPX + OTA 包"
            echo "  ./scripts/build.sh --no-upx           # 不压缩"
            echo "  ./scripts/build.sh --no-ota           # 不生成 OTA 包"
            echo "  ./scripts/build.sh --pack             # 构建 + 打包 UBIFS"
            echo ""
            echo "UBIFS 打包需要 mkfs.ubifs 工具 (mtd-utils)"
            echo "在 macOS 上需要使用 Docker 执行打包步骤"
            exit 0
            ;;
    esac
done

# ==================== 同步版本号 ====================
VERSION_FILE="VERSION"
if [ -f "$VERSION_FILE" ]; then
    VERSION=$(cat "$VERSION_FILE" | tr -d '[:space:]')
else
    VERSION="3.0.0"
    echo "⚠️  VERSION 文件不存在，使用默认版本: $VERSION"
fi

echo "📦 版本号: $VERSION"

# 更新 package.json 版本号
if [ -f "frontend/package.json" ]; then
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" frontend/package.json
    else
        sed -i "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" frontend/package.json
    fi
fi

# 更新 Cargo.toml 版本号
if [ -f "backend/Cargo.toml" ]; then
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" backend/Cargo.toml
    else
        sed -i "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" backend/Cargo.toml
    fi
fi

echo ""

# ==================== 构建前端 ====================
if [ "$BUILD_FRONTEND" = true ]; then
    echo "🎨 构建前端..."
    echo ""
    
    cd frontend
    
    # 检查 node_modules
    if [ ! -d "node_modules" ]; then
        echo "📦 安装前端依赖..."
        npm install
    fi
    
    # 构建
    npm run build
    
    cd ..
    
    echo ""
    echo "✅ 前端构建完成！"
    echo "📍 输出目录: frontend/dist/"
    echo ""
fi

# ==================== 构建后端 ====================
if [ "$BUILD_BACKEND" = true ]; then
    echo "🦀 构建后端 (aarch64-unknown-linux-musl)..."
    echo ""

    # 检查交叉编译器
    if ! command -v aarch64-unknown-linux-musl-gcc &> /dev/null; then
        echo "❌ 错误: 未找到 aarch64-unknown-linux-musl-gcc"
        echo ""
        echo "请安装交叉编译工具链:"
        echo "  brew tap messense/macos-cross-toolchains"
        echo "  brew install aarch64-unknown-linux-musl"
        exit 1
    fi
    
    cd backend

    # 设置交叉编译环境变量
    export CC_aarch64_unknown_linux_musl=aarch64-unknown-linux-musl-gcc
    export CXX_aarch64_unknown_linux_musl=aarch64-unknown-linux-musl-g++
    export AR_aarch64_unknown_linux_musl=aarch64-unknown-linux-musl-ar
    export SQLITE3_STATIC=1
    export LIBSQLITE3_SYS_USE_PKG_CONFIG=0

    # 构建
    cargo build --release --target aarch64-unknown-linux-musl

    cd ..

    BINARY_PATH="backend/target/aarch64-unknown-linux-musl/release/udx710"

    echo ""
    echo "✅ 后端构建完成！"
    echo "📍 二进制文件:"
    ls -lh "$BINARY_PATH"
    
    # UPX 压缩
    if [ "$USE_UPX" = true ]; then
        echo ""
        echo "UPX 压缩..."
    
        if ! command -v upx &> /dev/null; then
            echo "错误: 未找到 upx 命令"
            exit 1
        fi
        BEFORE_SIZE=$(stat -f%z "$BINARY_PATH" 2>/dev/null || stat -c%s "$BINARY_PATH" 2>/dev/null)
        upx --best --lzma "$BINARY_PATH"
        AFTER_SIZE=$(stat -f%z "$BINARY_PATH" 2>/dev/null || stat -c%s "$BINARY_PATH" 2>/dev/null)
        RATIO=$(echo "scale=1; 100 - ($AFTER_SIZE * 100 / $BEFORE_SIZE)" | bc)
        echo "压缩完成！节省: ${RATIO}%"
        ls -lh "$BINARY_PATH"
    fi
    
    echo ""
    echo "📋 文件信息:"
    file "$BINARY_PATH"
fi

# ==================== 复制到 userdata ====================
if [ "$COPY_TO_USERDATA" = true ]; then
    echo ""
    echo "=========================================="
    echo "  复制构建产物到 userdata"
    echo "=========================================="
    echo ""
    
    USERDATA_ROOT="userdata/home/root"
    USERDATA_WWW="$USERDATA_ROOT/www"
    
    # 检查 userdata 目录
    if [ ! -d "userdata" ]; then
        echo "错误: userdata 目录不存在"
        echo "请先提取 UBIFS:"
        echo "  ubireader_extract_files -o userdata 'img-0_vol-userdata.ubifs'"
        exit 1
    fi
    
    # 确保目标目录存在
    mkdir -p "$USERDATA_ROOT"
    mkdir -p "$USERDATA_WWW"
    
    # 复制后端二进制
    BACKEND_BIN="backend/target/aarch64-unknown-linux-musl/release/udx710"
    if [ -f "$BACKEND_BIN" ]; then
        echo "复制后端: $BACKEND_BIN"
        cp "$BACKEND_BIN" "$USERDATA_ROOT/udx710"
        chmod 755 "$USERDATA_ROOT/udx710"
        echo "  -> $USERDATA_ROOT/udx710 (755)"
    else
        echo "警告: 后端二进制不存在，跳过"
    fi
    
    # 确保脚本文件有可执行权限
    if [ -f "$USERDATA_ROOT/loader.sh" ]; then
        chmod 755 "$USERDATA_ROOT/loader.sh"
        echo "  -> $USERDATA_ROOT/loader.sh (755)"
    fi
    if [ -f "$USERDATA_ROOT/ttyd/start.sh" ]; then
        chmod 755 "$USERDATA_ROOT/ttyd/start.sh"
        chmod 755 "$USERDATA_ROOT/ttyd/ttyd"
        echo "  -> $USERDATA_ROOT/ttyd/*.sh, ttyd (755)"
    fi
    
    # 复制前端文件
    FRONTEND_DIR="frontend/dist"
    if [ -d "$FRONTEND_DIR" ]; then
        echo "复制前端: $FRONTEND_DIR"
        rm -rf "$USERDATA_WWW"/*
        cp -r "$FRONTEND_DIR"/* "$USERDATA_WWW/"
        # 设置权限: 文件 644, 目录 755
        find "$USERDATA_WWW" -type f -exec chmod 644 {} \;
        find "$USERDATA_WWW" -type d -exec chmod 755 {} \;
        echo "  -> $USERDATA_WWW/ (文件: 644, 目录: 755)"
    else
        echo "警告: 前端构建产物不存在，跳过"
    fi
    
    echo ""
    echo "userdata 文件列表:"
    ls -la "$USERDATA_ROOT/"
    echo ""
fi

# ==================== 打包 UBIFS ====================
if [ "$PACK_USERDATA" = true ]; then
    echo ""
    echo "=========================================="
    echo "  打包 UBIFS 镜像"
    echo "=========================================="
    echo ""
    
    # 检查 mkfs.ubifs 是否可用
    if command -v mkfs.ubifs &> /dev/null; then
        ./scripts/pack-userdata.sh --skip-copy
    else
        echo "mkfs.ubifs 不可用，尝试使用 Docker..."
        echo ""
        
        if command -v docker &> /dev/null; then
            docker run --rm -v "$(pwd)":/work -w /work ubuntu:22.04 bash -c \
                "apt-get update -qq && apt-get install -y -qq mtd-utils && ./scripts/pack-userdata.sh --skip-copy"
        else
            echo "错误: mkfs.ubifs 和 Docker 都不可用"
            echo ""
            echo "请安装其中之一:"
            echo "  - Linux: sudo apt-get install mtd-utils"
            echo "  - macOS: brew install --cask docker"
            exit 1
        fi
    fi
fi

# ==================== 生成 OTA 包 ====================
if [ "$SKIP_OTA" = false ] && [ "$BUILD_BACKEND" = true ] && [ "$BUILD_FRONTEND" = true ]; then
    echo ""
    echo "=========================================="
    echo "  生成 OTA 更新包"
    echo "=========================================="
    echo ""
    ./scripts/pack-ota.sh
fi

echo ""
echo "=========================================="
echo "部署命令: ./scripts/deploy.sh"
if [ "$PACK_USERDATA" = false ]; then
    echo "UBIFS 打包: ./scripts/build.sh --pack"
fi
echo "=========================================="
