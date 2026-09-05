#!/bin/bash
# floatapp 开机自启安装(当前用户) / 卸载: 删除对应 desktop 文件即可
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
mkdir -p "$AUTOSTART"

cat > "$AUTOSTART/lyyime-float.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=lyyIme 悬浮输入法
Comment=类万能五笔外挂悬浮窗输入(不依赖 ibus/fcitx)
Exec=python3 $DIR/lyyime_float.py
Icon=input-keyboard
Terminal=false
X-GNOME-Autostart-enabled=true
EOF

echo "已写入 $AUTOSTART/lyyime-float.desktop"
echo "卸载: rm $AUTOSTART/lyyime-float.desktop"
