#!/usr/bin/env bash
# lyyime-float(Rust 悬浮窗输入)卸载:删除启动器/桌面入口/主题图标
# (不影响开机自启项;如需一并移除:rm ~/.config/autostart/lyyime-float.desktop)
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"

pkill -x lyyime-float 2>/dev/null || true
rm -f "$PREFIX/bin/lyyime-float"
rm -f "$PREFIX/share/applications/lyyime-float.desktop"
rm -f "$PREFIX/share/icons/hicolor/scalable/apps/lyyime-float.svg"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi

echo "== lyyime-float 已卸载 =="
