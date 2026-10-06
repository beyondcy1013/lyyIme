#!/usr/bin/env bash
# CapsLock 联动解锁验证编排:
#   ./scripts/build-capslock.sh   # 全量单测→release 构建→xim 编译+单测→
#                                 # 隔离 Xvfb GUI E2E(--keep 留存证据):
#                                 # xim_e2e --caps-only 聚焦套件 +
#                                 # ibus_candwin + candidate_menu(真 core)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme

bash scripts/test.sh
bash scripts/build.sh --release --locked
make -C xim all test
bash tests/e2e/xim_e2e.sh --caps-only --keep
bash tests/e2e/ibus_candwin_e2e.sh --keep
bash tests/e2e/candidate_menu_e2e.sh --keep
