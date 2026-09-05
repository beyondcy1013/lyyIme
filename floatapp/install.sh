#!/usr/bin/env bash
# floatapp(悬浮窗输入)安装脚本:应用菜单/桌面启动器(幂等,可重复执行)
# 安装内容:
#   /usr/local/bin/lyyime-float                                     启动器
#   /usr/local/share/lyyime/floatapp/*.py                           程序本体
#   /usr/local/share/applications/lyyime-float.desktop              桌面入口
#   /usr/local/share/icons/hicolor/scalable/apps/lyyime-float.svg   主题图标
# 卸载:floatapp/uninstall.sh
set -euo pipefail

FLOAT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-/usr/local}"

echo "== 依赖检查 =="
MISS=()
command -v xdotool >/dev/null 2>&1 \
    || MISS+=("xdotool — 发送模式依赖(dnf install xdotool)")
python3 -c 'import gi' >/dev/null 2>&1 \
    || MISS+=("python3-gobject — GTK3 界面(dnf install python3-gobject)")
python3 -c 'import Xlib' >/dev/null 2>&1 \
    || MISS+=("python-xlib — 目标窗口识别(dnf install python3-xlib)")
if [ ${#MISS[@]} -gt 0 ]; then
    echo "错误:缺少运行依赖,请先安装后再执行本脚本:"
    for m in "${MISS[@]}"; do echo "  - $m"; done
    exit 1
fi

echo "== 安装到 $PREFIX =="
install -d "$PREFIX/share/lyyime/floatapp"
install -m 0755 "$FLOAT_DIR/lyyime_float.py" "$FLOAT_DIR/dictload.py" \
    "$FLOAT_DIR/chinese_pad.py" "$PREFIX/share/lyyime/floatapp/"
install -D -m 0755 /dev/stdin "$PREFIX/bin/lyyime-float" <<WRAPPER
#!/bin/sh
# lyyime-float 启动器:悬浮窗输入(单实例,二次启动唤起已有悬浮窗)
exec python3 $PREFIX/share/lyyime/floatapp/lyyime_float.py "\$@"
WRAPPER
install -D -m 0644 "$FLOAT_DIR/res/float.svg" \
    "$PREFIX/share/icons/hicolor/scalable/apps/lyyime-float.svg"
install -D -m 0644 /dev/stdin "$PREFIX/share/applications/lyyime-float.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=lyyIme 悬浮窗输入
Name[en]=lyyIme Floating IME
Comment=类万能五笔外挂的悬浮窗打字板(不依赖 ibus/fcitx,点开即用)
Exec=$PREFIX/bin/lyyime-float
Icon=lyyime-float
Terminal=false
Categories=Utility;
Keywords=input;method;ime;chinese;float;输入法;五笔;悬浮窗;外挂;
StartupNotify=false
StartupWMClass=lyyime-float
DESKTOP

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi

echo "== 安装完成 =="
cat <<'TIP'
使用:
  应用菜单/桌面点 "lyyIme 悬浮窗输入"(或直接运行 lyyime-float)。
  先点一下目标应用窗口,再回悬浮窗打五笔86;空格/数字上屏。
  重复点击图标 = 唤起已有悬浮窗(单实例,不会开第二个)。
卸载:floatapp/uninstall.sh
TIP
