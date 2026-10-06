#!/usr/bin/env bash
# CapsLock 解锁部署编排:
#   ./scripts/deploy-capslock.sh [--dry-run]   # 按本次构建产物计算 sha256
#   契约,委托 deploy-candidate-menu.sh 完成哈希核验/原子替换/回滚/终核验。
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme

sha() { sha256sum "$1" | cut -d' ' -f1; }
EXPECT_CORE_SHA="$(sha "$CARGO_TARGET_DIR/release/liblyyime_core.so")"
EXPECT_IBUS_SHA="$(sha "$CARGO_TARGET_DIR/release/ibus-engine-lyyime")"
EXPECT_FLOAT_SHA="$(sha "$CARGO_TARGET_DIR/release/lyyime-float")"
EXPECT_XIM_SHA="$(sha "$ROOT/xim/build/bin/lyyime-xim")"
EXPECT_SETTINGS_SHA="$(sha "$ROOT/xim/res/settings.ui")"
export EXPECT_CORE_SHA EXPECT_IBUS_SHA EXPECT_FLOAT_SHA EXPECT_XIM_SHA EXPECT_SETTINGS_SHA

exec bash "$ROOT/scripts/deploy-candidate-menu.sh" "$@"
