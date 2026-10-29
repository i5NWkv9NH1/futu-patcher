#!/usr/bin/env fish
# * 兼容 Fish 4.9 的极简一键构建与提权运行脚本

set -l SCRIPT_DIR (cd (status dirname); and pwd)
cd "$SCRIPT_DIR"

echo "==> 🚀 正在编译发布版本 (cargo build --release)..."
cargo build --release

if test $status -ne 0
    echo "❌ 编译失败，请检查 Rust 环境！"
    exit 1
end

echo "==> 🔐 正在以管理员权限启动 futu-patcher..."
sudo ./target/release/futu-patcher $argv
