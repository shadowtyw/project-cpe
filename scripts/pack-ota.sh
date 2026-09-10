#!/bin/bash
# Build a UDX710 OTA package compatible with the device OTA endpoint.
# Package layout: meta.json, udx710, www/

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION_FILE="VERSION"
BINARY_PATH="backend/target/aarch64-unknown-linux-musl/release/udx710"
FRONTEND_DIR="frontend/dist"
ARCH="aarch64-unknown-linux-musl"
OUTPUT_DIR="release"

if [ ! -f "$VERSION_FILE" ]; then
    echo "错误: VERSION 文件不存在" >&2
    exit 1
fi

VERSION=$(tr -d '[:space:]' < "$VERSION_FILE")
if ! [[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,3}$ ]]; then
    echo "错误: VERSION 必须是数字语义版本，例如 3.4.1" >&2
    exit 1
fi

if [ ! -f "$BINARY_PATH" ]; then
    echo "错误: 后端二进制不存在: $BINARY_PATH" >&2
    echo "请先运行: ./scripts/build.sh" >&2
    exit 1
fi
if [ ! -d "$FRONTEND_DIR" ]; then
    echo "错误: 前端构建产物不存在: $FRONTEND_DIR" >&2
    echo "请先运行: ./scripts/build.sh" >&2
    exit 1
fi

if command -v git >/dev/null 2>&1 && [ -d .git ]; then
    COMMIT=$(git rev-parse --short HEAD 2>/dev/null || printf 'unknown')
else
    COMMIT="unknown"
fi
BUILD_TIME=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
OTA_TMP=$(mktemp -d)
trap 'rm -rf "$OTA_TMP"' EXIT

md5_file() {
    if command -v md5sum >/dev/null 2>&1; then
        md5sum "$1" | cut -d' ' -f1
    else
        md5 -q "$1"
    fi
}

frontend_md5() {
    if command -v md5sum >/dev/null 2>&1; then
        find "$1" -type f -exec md5sum {} \; | cut -d' ' -f1 | sort | md5sum | cut -d' ' -f1
    else
        find "$1" -type f -exec md5 -q {} \; | sort | md5 -q
    fi
}

printf '%s\n' "=========================================="
printf '%s\n' "  打包 UDX710 OTA 更新包"
printf '%s\n' "=========================================="
printf '版本: %s\nCommit: %s\n构建时间: %s\n架构: %s\n\n' "$VERSION" "$COMMIT" "$BUILD_TIME" "$ARCH"

cp "$BINARY_PATH" "$OTA_TMP/udx710"
chmod 755 "$OTA_TMP/udx710"
mkdir -p "$OTA_TMP/www"
cp -R "$FRONTEND_DIR"/. "$OTA_TMP/www/"
find "$OTA_TMP/www" -type d -exec chmod 755 {} +
find "$OTA_TMP/www" -type f -exec chmod 644 {} +

BINARY_MD5=$(md5_file "$OTA_TMP/udx710")
FRONTEND_MD5=$(frontend_md5 "$OTA_TMP/www")

MIN_VERSION=${MIN_VERSION:-}
if [ -n "$MIN_VERSION" ]; then
    cat > "$OTA_TMP/meta.json" <<EOF
{
    "version": "$VERSION",
    "commit": "$COMMIT",
    "build_time": "$BUILD_TIME",
    "binary_md5": "$BINARY_MD5",
    "frontend_md5": "$FRONTEND_MD5",
    "arch": "$ARCH",
    "min_version": "$MIN_VERSION"
}
EOF
else
    cat > "$OTA_TMP/meta.json" <<EOF
{
    "version": "$VERSION",
    "commit": "$COMMIT",
    "build_time": "$BUILD_TIME",
    "binary_md5": "$BINARY_MD5",
    "frontend_md5": "$FRONTEND_MD5",
    "arch": "$ARCH"
}
EOF
fi

mkdir -p "$OUTPUT_DIR"
OTA_FILE="$OUTPUT_DIR/udx710-ota-${VERSION}.tar.gz"
rm -f "$OTA_FILE" "$OTA_FILE.sha256"
(
    cd "$OTA_TMP"
    tar -czf "$OLDPWD/$OTA_FILE" meta.json udx710 www
)

# CI and local builds use the same contract checks as the device-side verifier.
TOP_LEVEL=$(tar -tzf "$OTA_FILE" | cut -d/ -f1 | sort -u)
if [ "$TOP_LEVEL" != "meta.json
udx710
www" ]; then
    echo "错误: OTA 顶层内容必须严格为 meta.json、udx710、www/" >&2
    tar -tzf "$OTA_FILE" >&2
    exit 1
fi
if ! file "$OTA_TMP/udx710" | grep -qi 'aarch64'; then
    echo "错误: 后端二进制不是 aarch64 ELF: $(file "$OTA_TMP/udx710")" >&2
    exit 1
fi
if [ "$(md5_file "$OTA_TMP/udx710")" != "$BINARY_MD5" ] || [ "$(frontend_md5 "$OTA_TMP/www")" != "$FRONTEND_MD5" ]; then
    echo "错误: OTA 摘要自检失败" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$OTA_FILE" > "$OTA_FILE.sha256"
else
    shasum -a 256 "$OTA_FILE" > "$OTA_FILE.sha256"
fi

printf '\n%s\n' "=========================================="
printf '%s\n' "OTA 更新包打包完成"
printf '%s\n' "=========================================="
printf '输出: %s\n二进制 MD5: %s\n前端 MD5: %s\nSHA-256 清单: %s.sha256\n' \
    "$OTA_FILE" "$BINARY_MD5" "$FRONTEND_MD5" "$OTA_FILE"
printf '\n包内容:\n'
tar -tzf "$OTA_FILE"
