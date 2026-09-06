#!/usr/bin/env bash
# install.sh —— lyyIme ibus 引擎(Mode A,Rust)注册安装脚本。
#
# 用法:
#   ./install.sh                 # 用户级安装(免 root,推荐)
#   ./install.sh --system        # 系统级安装(需 root)
#   ./install.sh --enable        # 安装并追加到 ibus 预载列表(preload-engines)
#   ./install.sh --no-restart    # 不重启 ibus(只 write-cache)
#
# 设计要点:
#   - 引擎为 Rust 二进制(ibus-engine-lyyime,zbus 直连 lyyime-core,
#     不再依赖 liblyyime_core.so 的 ctypes FFI);
#   - AI 助手为 Rust 二进制 lyyime-ai(Mode B xim 以子进程共用);
#   - 组件注册走静态 engine XML(占位符在安装时替换为真实路径);
#   - 注册后执行 `ibus write-cache` 并重启 ibus 使托盘出现 "lyyIme 五笔拼音"。

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}"

MODE="user"
DO_ENABLE=0
DO_RESTART=1
ENGINE_NAME="lyyime"

log()  { printf '[install] %s\n' "$*"; }
warn() { printf '[install] 警告:%s\n' "$*" >&2; }
die()  { printf '[install] 错误:%s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

# ---------------------------------------------------------------- 参数解析
while [ $# -gt 0 ]; do
    case "$1" in
        --system) MODE="system" ;;
        --user)   MODE="user" ;;
        --enable) DO_ENABLE=1 ;;
        --no-restart) DO_RESTART=0 ;;
        -h|--help) usage ;;
        *) die "未知参数:$1(用 --help 查看用法)" ;;
    esac
    shift
done

# ---------------------------------------------------------------- 安装路径
if [ "$MODE" = "system" ]; then
    [ "$(id -u)" -eq 0 ] || die "系统级安装需要 root:请 sudo 执行,或去掉 --system 用用户级安装"
    DATA_ROOT="/usr/local/share/lyyime"
    COMPONENT_DIR="/usr/local/share/ibus/component"
else
    DATA_ROOT="${HOME}/.local/share/lyyime"
    COMPONENT_DIR="${HOME}/.local/share/ibus/component"
fi
ENGINE_DIR="${DATA_ROOT}/ibus/engine"
ICON_DIR="${DATA_ROOT}/ibus/icons"
BIN_DIR="${DATA_ROOT}/ibus/bin"
TOOLS_DIR="${DATA_ROOT}/tools"

log "模式=${MODE}  引擎目录=${ENGINE_DIR}"

# ---------------------------------------------------------------- 二进制
pick_bin() { # $1=名字
    for c in "$TARGET_DIR/release/$1" "$TARGET_DIR/debug/$1"; do
        [ -x "$c" ] && { echo "$c"; return 0; }
    done
    return 1
}

ENGINE_SRC="$(pick_bin ibus-engine-lyyime)" || true
[ -n "${ENGINE_SRC:-}" ] || die "未找到 ibus-engine-lyyime 二进制:先执行 scripts/build.sh(或设 CARGO_TARGET_DIR)"
AI_SRC="$(pick_bin lyyime-ai)" || true
[ -n "${AI_SRC:-}" ] || warn "未找到 lyyime-ai 二进制(/AI 功能不可用,其余正常;先 scripts/build.sh)"

install -d -m 755 "$ENGINE_DIR" "$ICON_DIR" "$BIN_DIR" "$TOOLS_DIR" "$COMPONENT_DIR"
install -m 755 "$ENGINE_SRC" "$ENGINE_DIR/ibus-engine-lyyime"
log "已安装引擎:$ENGINE_DIR/ibus-engine-lyyime"
if [ -n "${AI_SRC:-}" ]; then
    install -m 755 "$AI_SRC" "$ENGINE_DIR/lyyime-ai"
    install -m 755 "$AI_SRC" "$TOOLS_DIR/lyyime-ai"
    log "已安装 AI 助手:$TOOLS_DIR/lyyime-ai(xim 的 /AI 亦复用)"
fi
SHOT_SRC="$(pick_bin lyyime-shot)" || true
if [ -n "${SHOT_SRC:-}" ]; then
    install -m 755 "$SHOT_SRC" "$BIN_DIR/lyyime-shot"
    log "已安装截屏助手:$BIN_DIR/lyyime-shot(热键默认 Ctrl+Alt+A)"
else
    warn "未找到 lyyime-shot 二进制(截屏功能不可用,其余正常;先 scripts/build.sh)"
fi
for svg in "$SCRIPT_DIR"/icons/*.svg; do
    install -m 644 "$svg" "$ICON_DIR/$(basename "$svg")"
done
log "图标已就位(重复安装为覆盖,幂等)"

# ---------------------------------------------------------------- 组件 XML(占位符替换)
sed -e "s|@ENGINE_EXEC@|${ENGINE_DIR}/ibus-engine-lyyime|g" \
    -e "s|@ICON_DIR@|${ICON_DIR}|g" \
    -e "s|@SETUP@|${BIN_DIR}/lyyime-setup|g" \
    "$SCRIPT_DIR/lyyime.xml" > "$COMPONENT_DIR/lyyime.xml"
chmod 644 "$COMPONENT_DIR/lyyime.xml"
log "组件注册文件已生成:$COMPONENT_DIR/lyyime.xml"

# ---------------------------------------------------------------- 可见性桥接
# 实证(ibus 1.5.29 源码 src/ibusregistry.c):daemon 只扫描
# IBUS_DATA_DIR/component(即 /usr/share/ibus/component);用户目录支持被
# 上游 "#if 0" 禁用,唯一覆盖手段是 IBUS_COMPONENT_PATH 环境变量(注意它
# 会整体替换默认搜索路径)。因此把组件再桥接一份到实际扫描目录,保证托盘
# 一定能看到 "lyyIme 五笔拼音"。
bridge_component() {
    local scan_dir="/usr/share/ibus/component"
    local dest="${scan_dir}/lyyime.xml"
    if [ "$COMPONENT_DIR" = "$scan_dir" ]; then
        return 0
    fi
    if [ -n "${IBUS_COMPONENT_PATH:-}" ]; then
        case ":$IBUS_COMPONENT_PATH:" in
            *":$COMPONENT_DIR:"*)
                log "IBUS_COMPONENT_PATH 已包含 ${COMPONENT_DIR},无需桥接" ;;
            *)
                warn "IBUS_COMPONENT_PATH 已设置但未包含 ${COMPONENT_DIR}。"
                warn "该变量会整体替换默认搜索路径,请改为(务必含标准目录):"
                warn "  export IBUS_COMPONENT_PATH=${COMPONENT_DIR}:/usr/share/ibus/component" ;;
        esac
        return 0
    fi
    if [ ! -d "$scan_dir" ]; then
        warn "未找到 $scan_dir,跳过桥接(此 ibus 版本行为未知,请用 ibus list-engine 验证)"
        return 0
    fi
    if install -m 644 "$COMPONENT_DIR/lyyime.xml" "$dest" 2>/dev/null; then
        log "已桥接组件到 ibus 实际扫描目录:$dest"
    else
        warn "无法写入 $dest(需要 root)。当前 ibus 1.5.29 不扫描用户/系统(local)组件目录,"
        warn "仅扫 /usr/share/ibus/component。两种方案让托盘出现 lyyIme:"
        warn "  a) sudo 重跑:$0 --system"
        warn "  b) 会话环境(如 ~/.xprofile)导出:"
        warn "     export IBUS_COMPONENT_PATH=${COMPONENT_DIR}:/usr/share/ibus/component"
    fi
}
bridge_component

# ---------------------------------------------------------------- 词典数据提示
if [ ! -f "${DATA_ROOT}/wubi.tsv" ] && [ ! -f "${HOME}/.local/share/lyyime/wubi.tsv" ]; then
    warn "未检测到词典(wubi.tsv 等)。引擎启动后无候选属正常,请先运行词库管线:"
    warn "  scripts/build.sh && cargo run -p lyyime-dicttool -- convert ...(见 README)"
fi

# ---------------------------------------------------------------- ibus 注册生效
if command -v ibus >/dev/null 2>&1; then
    if ibus write-cache 2>/dev/null; then
        log "ibus write-cache 完成"
    else
        warn "ibus write-cache 失败(ibus 可能未运行),登出重登或手动 ibus-daemon -drx 亦可生效"
    fi
else
    warn "未找到 ibus 命令,请确认 ibus 已安装"
fi

if [ "$DO_RESTART" -eq 1 ]; then
    if command -v ibus >/dev/null 2>&1 && pgrep -x ibus-daemon >/dev/null 2>&1; then
        log "重启 ibus 使注册生效(任务栏会闪一下,属正常)…"
        ibus restart || warn "ibus restart 失败:请手动执行,或注销重登"
    else
        log "检测到 ibus-daemon 未运行;下次进入桌面会话时自动加载,无需处理"
    fi
else
    log "按参数跳过 ibus 重启(--no-restart);变更需重启 ibus 后生效"
fi

# ---------------------------------------------------------------- 预载列表(--enable)
# 键位与形态见 RESEARCH §1.6:org.freedesktop.ibus.general preload-engines,
# 只追加、绝不破坏现有条目;gsettings 优先,不可用时降级 dconf 直写。
append_preload() {
    local cur new
    cur="$(gsettings get org.freedesktop.ibus.general preload-engines 2>/dev/null || true)"
    if [ -z "$cur" ]; then
        cur="$(dconf read /desktop/ibus/general/preload-engines 2>/dev/null || true)"
        [ -z "$cur" ] && cur='@as []'
        READER="dconf"
    else
        READER="gsettings"
    fi
    # 纯 shell 追加(避免依赖 python):cur 形如 "@as []" 或 "['a','b']"
    if [[ "$cur" == *"$ENGINE_NAME"* ]]; then
        new="$cur"
    elif [ "$cur" = "@as []" ]; then
        new="@as ['$ENGINE_NAME']"
    else
        new="${cur%]}, '$ENGINE_NAME']"
    fi
    if [ "$READER" = "gsettings" ]; then
        if gsettings set org.freedesktop.ibus.general preload-engines "$new" 2>/dev/null; then
            log "已把 ${ENGINE_NAME} 追加进预载列表(gsettings):$new"
            return 0
        fi
        warn "gsettings 写入失败,降级 dconf 直写"
    fi
    if command -v dconf >/dev/null 2>&1 && dconf write /desktop/ibus/general/preload-engines "$new" 2>/dev/null; then
        log "已把 ${ENGINE_NAME} 追加进预载列表(dconf):$new"
    else
        warn "预载列表写入失败(gsettings/dconf 均不可用,常见于无 DBus 会话)。"
        warn "手动添加:ibus 设置 → 输入法 → 添加 \"lyyIme 五笔拼音\",或执行:"
        warn "  gsettings set org.freedesktop.ibus.general preload-engines \"[…,'${ENGINE_NAME}']\""
    fi
}

if [ "$DO_ENABLE" -eq 1 ]; then
    append_preload
else
    log "提示:未指定 --enable;请到 ibus 设置里手动添加 \"lyyIme 五笔拼音\",或重跑 ./install.sh --enable"
fi

log "安装完成。验证:ibus list-engine | grep lyyime;托盘添加后即可在任意应用输入。"
