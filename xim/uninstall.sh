#!/usr/bin/env bash
# lyyime-xim 卸载脚本(与 install.sh 配对;幂等)
# 只删除系统安装文件,不触碰用户数据(~/.config/lyyime、~/.local/share/lyyime
# 中的词库/配置/日志会保留;如需彻底清理请手动删除并自行备份用户词)。
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"

echo "== 停止运行中的 lyyime-xim =="
PIDFILE="${HOME}/.local/share/lyyime/xim.pid"
if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
    if [[ -n "${pid:-}" ]] && kill "$pid" 2>/dev/null; then
        echo "已停止 pid=$pid"
        sleep 0.5
    fi
    rm -f "$PIDFILE"
fi
pkill -x lyyime-xim 2>/dev/null || true

echo "== 删除文件 =="
rm -f "$PREFIX/bin/lyyime-xim"
rm -f "$PREFIX/share/lyyime/icons/zh.svg" "$PREFIX/share/lyyime/icons/en.svg"
rmdir "$PREFIX/share/lyyime/icons" 2>/dev/null || true
rm -f "$PREFIX/share/lyyime/res/settings.ui" "$PREFIX/share/lyyime/res/candidate.css"
rmdir "$PREFIX/share/lyyime/res" 2>/dev/null || true
# AI 助手脚本:仅当 ibus 引擎安装位不存在时才删(避免误删 Mode A 在用的副本)
if [ ! -f "$PREFIX/share/lyyime/ibus/engine/lyyime_ai.py" ]; then
    rm -f "$PREFIX/share/lyyime/tools/lyyime_ai.py"
    rmdir "$PREFIX/share/lyyime/tools" 2>/dev/null || true
fi
rmdir "$PREFIX/share/lyyime" 2>/dev/null || true
rm -f "$PREFIX/share/applications/lyyime-xim-settings.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi

echo "== 卸载完成(用户数据保留于 ~/.config/lyyime 与 ~/.local/share/lyyime)=="
