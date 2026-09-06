#!/usr/bin/env bash
# lyyime-float 开机自启安装(当前用户) / 卸载: 删除对应 desktop 文件即可
set -e
CRATE_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="${BIN:-/usr/local/bin/lyyime-float}"
if [ ! -x "$BIN" ]; then
    # 未装机时直接指向仓库构建产物
    TD="${CARGO_TARGET_DIR:-$CRATE_DIR/../../target}"
    for c in "$TD/release/lyyime-float" "$TD/debug/lyyime-float"; do
        [ -x "$c" ] && BIN="$c" && break
    done
fi
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
mkdir -p "$AUTOSTART"

cat > "$AUTOSTART/lyyime-float.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=lyyIme 悬浮输入法
Comment=类万能五笔外挂悬浮窗输入(Rust/GTK3,不依赖 ibus/fcitx)
Exec=$BIN
Icon=input-keyboard
Terminal=false
X-GNOME-Autostart-enabled=true
EOF

echo "已写入 $AUTOSTART/lyyime-float.desktop(Exec=$BIN)"
echo "卸载: rm $AUTOSTART/lyyime-float.desktop"
