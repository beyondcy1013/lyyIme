#!/usr/bin/env bash
# lyyIme 构建脚本 —— CARGO_TARGET_DIR 固定到 /data/cargo-target(仓库硬性规则)
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme
echo ">> cargo build $* (target: $CARGO_TARGET_DIR)"
cargo build --workspace "$@"
echo ">> 产物:"
ls -1 "$CARGO_TARGET_DIR/debug/" 2>/dev/null | grep -E 'lyyime|liblyyime' || true
