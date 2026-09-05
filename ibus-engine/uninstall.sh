#!/usr/bin/env bash
# -*- coding: utf-8 -*-
# uninstall.sh —— lyyIme ibus 引擎(Mode A)反向清理脚本(与 install.sh 配对)。
#
# 用法:
#   ./uninstall.sh               # 用户级与系统级注册一起清理(系统级需 root)
#   ./uninstall.sh --user        # 只清理用户级
#   ./uninstall.sh --system      # 只清理系统级
#   ./uninstall.sh --purge       # 额外删除 ~/.local/share/lyyime 下的用户数据(词频/日志)!
#   ./uninstall.sh --no-restart  # 不重启 ibus
#
# 设计要点:
#   - 幂等:目标不存在时静默跳过,可重复执行;
#   - 只删 lyyime 自身文件,绝不触碰其它输入法与用户码表;
#   - 默认保留用户数据(user.tsv 词频、logs),--purge 才删。

set -euo pipefail

MODE="auto"       # auto | user | system
PURGE=0
DO_RESTART=1
ENGINE_NAME="lyyime"

log()  { printf '[uninstall] %s\n' "$*"; }
warn() { printf '[uninstall] 警告:%s\n' "$*" >&2; }

usage() {
    sed -n '3,10p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --user) MODE="user" ;;
        --system) MODE="system" ;;
        --purge) PURGE=1 ;;
        --no-restart) DO_RESTART=0 ;;
        -h|--help) usage ;;
        *) warn "未知参数:$1(忽略;用 --help 查看用法)" ;;
    esac
    shift
done

USER_DATA_ROOT="${HOME}/.local/share/lyyime"
USER_COMPONENT="${HOME}/.local/share/ibus/component/lyyime.xml"
SYSTEM_COMPONENT="/usr/local/share/ibus/component/lyyime.xml"

# 判断某模式是否有可清理的痕迹(组件 XML 或安装目录)
mode_has_files() {
    local mode="$1"
    if [ "$mode" = "user" ]; then
        [ -e "$USER_COMPONENT" ] || [ -d "$USER_DATA_ROOT/ibus" ]
    else
        [ -e "$SYSTEM_COMPONENT" ] || [ -d "/usr/local/share/lyyime/ibus" ] \
            || [ -e "/usr/local/lib/lyyime/liblyyime_core.so" ] \
            || [ -e "/usr/share/ibus/component/lyyime.xml" ]
    fi
}

# ---------------------------------------------------------------- 清理单个模式
clean_mode() {
    local mode="$1"
    local data_root component_dir engine_dir core_lib
    if [ "$mode" = "system" ]; then
        if [ "$(id -u)" -ne 0 ]; then
            warn "系统级清理需要 root,跳过(如需请 sudo 执行)"
            return 0
        fi
        data_root="/usr/local/share/lyyime"
        component_dir="/usr/local/share/ibus/component"
        core_lib="/usr/local/lib/lyyime/liblyyime_core.so"
    else
        data_root="$USER_DATA_ROOT"
        component_dir="${HOME}/.local/share/ibus/component"
        core_lib="${data_root}/lib/liblyyime_core.so"
    fi
    engine_dir="${data_root}/ibus"

    rm -f "${component_dir}/lyyime.xml"
    rmdir "$component_dir" 2>/dev/null || true
    rm -rf "$engine_dir"            # 只删 lyyime 的 engine/icons/bin 子树
    rm -f  "$core_lib"
    rmdir "$(dirname "$core_lib")" 2>/dev/null || true
    if [ "$mode" = "user" ]; then
        # 用户数据目录只删空的;有词频/日志时保留(--purge 才删)
        rmdir "$data_root" 2>/dev/null || true
    fi
    log "[$mode] 已移除组件注册、引擎文件与核心库"
}

# ---------------------------------------------------------------- 执行清理
case "$MODE" in
    user)   [ "$(id -u)" -eq 0 ] && [ "$(stat -c %u "$USER_DATA_ROOT" 2>/dev/null || echo 0)" != "0" ] \
                && warn "当前是 root,清理的是 root 的用户级安装"; clean_mode user ;;
    system) clean_mode system ;;
    auto)   # 双模式都探一遍,有痕迹才动手,全部幂等
        if mode_has_files user; then clean_mode user; else log "[user] 无用户级安装痕迹,跳过"; fi
        if mode_has_files system; then clean_mode system; else log "[system] 无系统级安装痕迹,跳过"; fi ;;
esac

# ---------------------------------------------------------------- 桥接组件
# install.sh 会把组件桥接一份到 ibus 1.5.29 的实际扫描目录(/usr/share),
# 这里无论哪种模式都要清掉(幂等:不存在则跳过)。
BRIDGE_XML="/usr/share/ibus/component/lyyime.xml"
if [ -f "$BRIDGE_XML" ]; then
    if [ "$(id -u)" -eq 0 ]; then
        rm -f "$BRIDGE_XML"
        log "已移除桥接组件:$BRIDGE_XML"
    else
        warn "桥接组件需 root 移除:sudo rm -f $BRIDGE_XML"
    fi
fi

# ---------------------------------------------------------------- 预载列表移除
remove_preload() {
    local cur new reader=""
    cur="$(gsettings get org.freedesktop.ibus.general preload-engines 2>/dev/null || true)"
    if [ -n "$cur" ]; then
        reader="gsettings"
    else
        cur="$(dconf read /desktop/ibus/general/preload-engines 2>/dev/null || true)"
        [ -z "$cur" ] && return 0   # 本来就没配,幂等返回
        reader="dconf"
    fi
    new="$(python3 - "$cur" "$ENGINE_NAME" <<'PY'
import ast, sys
cur, name = sys.argv[1], sys.argv[2]
if cur.startswith('@as'):
    cur = cur[3:].strip() or '[]'
try:
    items = ast.literal_eval(cur)
except (ValueError, SyntaxError):
    items = []
if not isinstance(items, list):
    items = []
print(repr([x for x in items if x != name]))
PY
)"
    if [ "$reader" = "gsettings" ]; then
        gsettings set org.freedesktop.ibus.general preload-engines "$new" 2>/dev/null \
            || dconf write /desktop/ibus/general/preload-engines "$new" 2>/dev/null \
            || warn "预载列表移除失败,请手动在 ibus 设置中移除 lyyIme"
    else
        dconf write /desktop/ibus/general/preload-engines "$new" 2>/dev/null \
            || warn "预载列表移除失败,请手动在 ibus 设置中移除 lyyIme"
    fi
    log "已从 ibus 预载列表移除 ${ENGINE_NAME}(现值:$new)"
}
remove_preload

# ---------------------------------------------------------------- 可选:清用户数据
if [ "$PURGE" -eq 1 ]; then
    rm -rf "$USER_DATA_ROOT"
    log "已删除用户数据目录:$USER_DATA_ROOT(词频学习记录与日志)"
else
    [ -d "$USER_DATA_ROOT" ] && log "保留用户数据:$USER_DATA_ROOT(词频/日志;确认不要可加 --purge)"
fi

# ---------------------------------------------------------------- ibus 缓存刷新
if command -v ibus >/dev/null 2>&1 && ibus write-cache 2>/dev/null; then
    log "ibus write-cache 完成"
fi
if [ "$DO_RESTART" -eq 1 ] && command -v ibus >/dev/null 2>&1 \
        && pgrep -x ibus-daemon >/dev/null 2>&1; then
    log "重启 ibus…"
    ibus restart || warn "ibus restart 失败:请手动执行或注销重登"
fi

log "卸载完成。"
