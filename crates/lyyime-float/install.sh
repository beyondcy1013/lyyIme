#!/usr/bin/env bash
# lyyime-float(Rust 悬浮窗输入)安装脚本:应用菜单/桌面启动器(幂等,可重复执行)
# 安装内容:
#   /usr/local/bin/lyyime-float                                     主程序(Rust)
#   /usr/local/share/applications/lyyime-float.desktop              桌面入口
#   /usr/local/share/icons/hicolor/scalable/apps/lyyime-float.svg   主题图标
# 前置:scripts/build.sh 已产出 target(debug/release)二进制,或传入 BIN= 路径
# 卸载:crates/lyyime-float/uninstall.sh
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-/usr/local}"
TARGET_DIR="${CARGO_TARGET_DIR:-$CRATE_DIR/../../target}"

# 二进制来源:BIN= 显式指定 > release > debug
BIN="${BIN:-}"
if [ -z "$BIN" ]; then
  for c in "$TARGET_DIR/release/lyyime-float" "$TARGET_DIR/debug/lyyime-float"; do
    [ -x "$c" ] && BIN="$c" && break
  done
fi
[ -n "$BIN" ] && [ -x "$BIN" ] || { echo "错误:未找到 lyyime-float 二进制,先跑 scripts/build.sh 或用 BIN= 指定"; exit 1; }

echo "== 安装到 $PREFIX(二进制: $BIN)=="
install -d "$PREFIX/bin"
install -m 0755 "$BIN" "$PREFIX/bin/lyyime-float"
install -D -m 0644 "$CRATE_DIR/res/float.svg" \
    "$PREFIX/share/icons/hicolor/scalable/apps/lyyime-float.svg"
install -D -m 0644 /dev/stdin "$PREFIX/share/applications/lyyime-float.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=lyyIme 悬浮窗输入
Name[en]=lyyIme Floating IME
Comment=类万能五笔外挂的悬浮窗打字板(Rust/GTK3,不依赖 ibus/fcitx,点开即用)
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
  菜单 ☰ → 自定义短语管理…:自定义编码 → 任意长度文本(支持 $date/$time/$week 变量)。
  重复点击图标 = 唤起已有悬浮窗(单实例,不会开第二个)。
卸载:crates/lyyime-float/uninstall.sh
TIP
