#!/usr/bin/env bash
# lyyIme 测试脚本
#   ./scripts/test.sh           # core + dicttool + doctor 单测
#   ./scripts/test.sh core      # 仅 lyyime-core
#   ./scripts/test.sh all       # 单测 + FFI 冒烟 + ibus 引擎无 GUI 单测
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme
SCOPE="${1:-unit}"

run_cargo_test() {
    local pkg="$1"
    echo ">> cargo test -p $pkg"
    cargo test -p "$pkg" --all-features
}

case "$SCOPE" in
  core)
    run_cargo_test lyyime-core
    ;;
  unit)
    run_cargo_test lyyime-core
    run_cargo_test lyyime-dicttool
    run_cargo_test lyyime-doctor
    run_cargo_test lyyime-float
    ;;
  all)
    run_cargo_test lyyime-core
    run_cargo_test lyyime-dicttool
    run_cargo_test lyyime-doctor
    run_cargo_test lyyime-float
    [ -f crates/lyyime-core/ffi-test.py ] && python3 crates/lyyime-core/ffi-test.py
    make -C xim test
    ;;
  *) echo "用法: $0 [core|unit|all]"; exit 1 ;;
esac
echo ">> 测试全部通过 ✅"
