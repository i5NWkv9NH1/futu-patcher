#!/usr/bin/env bash
# futu-patcher 一键在线安装与执行脚本
set -euo pipefail

TMP_BIN="/tmp/futu-patcher"
RELEASE_URL="https://github.com/i5NWkv9NH1/futu-patcher/releases/latest/download/futu-patcher"

# * 退出时自动清理临时二进制文件
cleanup() {
    rm -f "$TMP_BIN"
}
trap cleanup EXIT

# ! 1. 架构与操作系统校验
OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" != "Darwin" ]; then
    echo "错误: 本工具仅支持 macOS 系统 (当前系统: $OS)" >&2
    exit 1
fi

if [ "$ARCH" != "arm64" ]; then
    echo "错误: 本工具仅支持 Apple Silicon (arm64) 架构 (当前架构: $ARCH)" >&2
    exit 1
fi

# * 2. 下载预编译的独立二进制文件
echo "==> 正在下载 futu-patcher 预编译二进制..."
if command -v curl >/dev/null 2>&1; then
    curl -fSL "$RELEASE_URL" -o "$TMP_BIN"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TMP_BIN" "$RELEASE_URL"
else
    echo "错误: 未找到 curl 或 wget 下载工具" >&2
    exit 1
fi

chmod +x "$TMP_BIN"

# * 3. 使用 sudo 运行并重定向 /dev/tty 以支持交互菜单
echo "==> 启动客户端处理程序 (若提示请输入当前用户开机密码)..."
if [ -t 0 ]; then
    sudo "$TMP_BIN" "$@"
else
    # ! 管道执行场景 (curl ... | bash) 下必须重定向 /dev/tty
    sudo "$TMP_BIN" "$@" < /dev/tty
fi
