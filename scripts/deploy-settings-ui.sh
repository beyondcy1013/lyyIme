#!/usr/bin/env bash
# 部署 settings.ui 资源(自定义查询设置项 UI)并重启 xim 使顶层配置生效
set -euo pipefail
cd "$(dirname "$0")/.."
install -Dm644 xim/res/settings.ui /usr/local/share/lyyime/res/settings.ui
