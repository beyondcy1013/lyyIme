#!/usr/bin/env bash
# lyyime-xim 安装脚本(Mode B 独立外挂;幂等,可重复执行)
# 安装内容:
#   /usr/local/bin/lyyime-xim                  主程序
#   /usr/local/share/lyyime/icons/{zh,en}.svg  托盘图标
#   /usr/local/share/lyyime/res/*              设置界面 .ui / 候选窗样式 .css
#   /usr/local/share/applications/lyyime-xim-settings.desktop  设置入口
# 注意:liblyyime_core.so 不由本脚本安装(集成期由 scripts/install-all.sh 统一
#       部署到 /usr/local/lib/lyyime/;缺失时 lyyime-xim 以直通模式运行并给指引)。
set -euo pipefail

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-/usr/local}"

echo "== 构建(make all) =="
make -C "$XIM_DIR" all

echo "== 安装到 $PREFIX =="
install -D -m 0755 "$XIM_DIR/build/bin/lyyime-xim" "$PREFIX/bin/lyyime-xim"
install -D -m 0644 "$XIM_DIR/res/zh.svg" "$PREFIX/share/lyyime/icons/zh.svg"
install -D -m 0644 "$XIM_DIR/res/en.svg" "$PREFIX/share/lyyime/icons/en.svg"
# 主题图标(hicolor):应用菜单/桌面图标按名字查找
install -D -m 0644 "$XIM_DIR/res/zh.svg" \
    "$PREFIX/share/icons/hicolor/scalable/apps/lyyime-xim.svg"
install -D -m 0644 "$XIM_DIR/res/settings.ui" "$PREFIX/share/lyyime/res/settings.ui"
install -D -m 0644 "$XIM_DIR/res/candidate.css" "$PREFIX/share/lyyime/res/candidate.css"

# 主启动器(应用菜单/桌面;重复点击=唤起已运行实例的设置窗)
install -D -m 0644 /dev/stdin "$PREFIX/share/applications/lyyime-xim.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=lyyIme 独立外挂输入法
Name[en]=lyyIme Standalone IME
Comment=不依赖 ibus/fcitx 的 XIM 独立输入法(Mode B)
Exec=$PREFIX/bin/lyyime-xim
Icon=lyyime-xim
Terminal=false
Categories=Utility;System;
Keywords=input;method;ime;chinese;输入法;五笔;拼音;
StartupNotify=false
DESKTOP

install -D -m 0644 /dev/stdin "$PREFIX/share/applications/lyyime-xim-settings.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=lyyIme 输入法设置
Name[en]=lyyIme IM Settings
Comment=设置 lyyIme 独立输入法外挂(XIM)
Exec=$PREFIX/bin/lyyime-xim --settings
Icon=lyyime-xim
Terminal=false
Categories=Settings;DesktopSettings;
Keywords=input;method;ime;chinese;输入法;
DESKTOP

# 刷新桌面数据库(存在才执行,幂等)
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi

echo "== 安装完成 =="
cat <<'TIP'
后续步骤:
  1. 部署核心库:cp liblyyime_core.so /usr/local/lib/lyyime/
     (或对单次运行使用 LYYIME_CORE_LIB=/路径/liblyyime_core.so)
  2. 应用侧接入:XMODIFIERS=@im=lyyime、GTK_IM_MODULE=xim
     (可用 lyyime-doctor 的 Mode B profile 一键写入会话环境)
  3. 自启动:托盘右键 → 设置 → 勾选"开机自启动"
卸载:xim/uninstall.sh
TIP
