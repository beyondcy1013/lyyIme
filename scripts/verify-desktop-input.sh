#!/usr/bin/env bash
# verify-desktop-input.sh — 真实桌面中文输入链路窄验证(只读观测,不改配置)
#
# 用 ime-probe.py 弹一个真 GTK Entry,按现行桌面会话环境
# (GTK_IM_MODULE=ibus / XMODIFIERS=@im=ibus)键入指定码(默认 ymlf)上屏期望文本,
# 断言:文本真实落盘、IBus.Panel 在当前私有总线有主且属主不变、
# ibus.log 出现期望的菜单提示行。只向自己的探针窗输入;退出时恢复原焦点。
#
# 参数:键码 期望上屏文本 引擎日志提示正则 [repeat-settings]
#   默认:bash scripts/verify-desktop-input.sh            # ymlf→设置→F7 提示
#   截图:bash scripts/verify-desktop-input.sh falt 截图 \
#        '匹配.*截屏.*F7.*Ctrl\+Alt\+A'
#   repeat-settings(仅默认码):连做 3 轮「输入设置+F7 打开设置窗→
#        WM_DELETE 关闭→再次输入」,每轮要求新菜单提示行与新
#        'settings show' 日志;收尾核验 config/XIM/引擎/面板属主不变。
set -euo pipefail
CODE="${1:-ymlf}"
TEXT="${2:-设置}"
HINT="${3:-匹配.*设置.*F7}"
MODE="${4:-}"
LIFE=20
if [[ $MODE == repeat-settings ]]; then
    [[ $CODE == ymlf && $TEXT == 设置 && $HINT == '匹配.*设置.*F7' ]] || {
        echo "repeat-settings 仅支持默认 ymlf/设置/F7 参数" >&2; exit 2; }
    LIFE=90
elif [[ $MODE == paging ]]; then
    LIFE=60
elif [[ -n $MODE ]]; then
    echo "未知模式:$MODE(仅支持 repeat-settings/paging)" >&2; exit 2
fi
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$ROOT/scripts/lib-live-pids.sh"
ENGINE_EXE=/home/root/.local/share/lyyime/ibus/engine/ibus-engine-lyyime

proc_start() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
dump_procs() { ps -C lyyime-xim,ibus-engine-lyyime -o pid,ppid,stat,comm,args >&2 || true; }
PID="$(session_xim_pid)" || {
    echo "当前用户下需有且仅有一个存活的已安装 XIM 进程" >&2
    dump_procs
    exit 1
}
PID_START="$(proc_start "$PID")"
[[ -n $PID_START ]]
while IFS= read -r -d '' entry; do
    case "$entry" in
        HOME=*|DISPLAY=*|XDG_RUNTIME_DIR=*|DBUS_SESSION_BUS_ADDRESS=*|XAUTHORITY=*)
            export "$entry" ;;
    esac
done < "/proc/$PID/environ"
unset IBUS_ADDRESS
export GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
IBUS_LOG=/home/root/.local/share/lyyime/logs/ibus.log
XIM_LOG=/home/root/.local/share/lyyime/logs/xim.log
CONFIG=/home/root/.config/lyyime/config.toml
SETTINGS_RE='^lyyIme 输入法设置$'   # settings.ui 真实窗口标题

# 面板属主经当前私有总线查询(独立 unit 与 daemon 托管面板都支持):
# GetConnectionUnixProcessID 在当前私有 bus 上为 UnknownMethod,不可用;
# unique owner 名 + 固定总线地址已足以识别"换主/重注册"。
ADDR="$(timeout 3 ibus address)"
[[ -n $ADDR ]]
panel_owner() {
    local raw owner
    raw="$(timeout 3 gdbus call --address "$ADDR" --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.GetNameOwner \
        org.freedesktop.IBus.Panel 2>/dev/null)" || return 1
    owner="$(grep -o "':[0-9][0-9.]*'" <<<"$raw" | tr -d "'")"
    [[ -n $owner ]] || return 1
    printf '%s\n' "$owner"
}
PANEL_BASE="$(panel_owner)" || { echo "IBus.Panel 无主,拒绝运行" >&2; exit 1; }

# 引擎/config 基线(收尾须不变);-f 锚定绝对路径,排除命令行含同名的包装进程
mapfile -t EPIDS < <(engine_pids /proc "$ENGINE_EXE")
[[ ${#EPIDS[@]} -eq 1 ]] || {
    echo "需有且仅有一个存活、会话匹配的已安装引擎进程(实得 ${#EPIDS[@]})" >&2
    dump_procs
    exit 1
}
ENGINE_PID="${EPIDS[0]}"
CONFIG_SHA_BEFORE="ABSENT"
[[ -f $CONFIG ]] && CONFIG_SHA_BEFORE="$(sha256sum "$CONFIG" | cut -d' ' -f1)"

# repeat-settings 前置核验(只读,聚焦前完成):真实配置须 F7 可执行 settings
if [[ $MODE == repeat-settings ]]; then
    python3 - "$CONFIG" <<'PY'
import pathlib, sys, tomllib
p = pathlib.Path(sys.argv[1])
cfg = tomllib.loads(p.read_text()) if p.exists() else {}
assert cfg.get("menu_trigger_enabled", True), "菜单触发已禁用,拒绝验证"
assert cfg.get("menu_trigger_key", 7) == 7, "本验证要求 F7"
disabled = {s.strip() for s in cfg.get("menu_trigger_disabled", "fix_ime").split(",")}
assert "settings" not in disabled, "settings 在黑名单中,拒绝验证"
PY
    [[ "$(timeout 3 ibus engine 2>/dev/null)" == lyyime ]] \
        || { echo "当前 ibus engine 非 lyyime,拒绝验证" >&2; exit 1; }
fi

# repeat-settings:聚焦前拒绝干扰已存在的可见设置窗(用户可能正在用)
if [[ $MODE == repeat-settings ]]; then
    [[ -z "$(xdotool search --all --onlyvisible --pid "$PID" --name "$SETTINGS_RE" 2>/dev/null || true)" ]] \
        || { echo "XIM 已有可见设置窗,拒绝干扰" >&2; exit 1; }
fi

python3 - <<'PY'
import gi
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk
state = Gdk.Keymap.get_for_display(Gdk.Display.get_default()).get_modifier_state()
blocked = (Gdk.ModifierType.SHIFT_MASK | Gdk.ModifierType.LOCK_MASK |
           Gdk.ModifierType.CONTROL_MASK | Gdk.ModifierType.MOD1_MASK |
           Gdk.ModifierType.MOD4_MASK)
assert not state & blocked, "请松开 Shift/Ctrl/Alt/Super 并关闭 CapsLock 后再验证"
PY

WORK="$(mktemp -d /tmp/lyyime-desktop-check.XXXXXX)"
printf 'Evidence: %s\n' "$WORK"
OLD_FOCUS="$(xdotool getwindowfocus)"
CLIENT_PID=""
SET_WID=""

# 只向本次打开的设置窗发 WM_DELETE(不保存);ctypes 直调 libX11 XSendEvent。
wm_delete() {
    python3 - "$1" <<'PY'
import ctypes, sys
from ctypes import Structure, Union, c_int, c_long, c_ulong, c_void_p

class XClientMessageEvent(Structure):
    _fields_ = [("type", c_int), ("serial", c_ulong), ("send_event", c_int),
                ("display", c_void_p), ("window", c_ulong),
                ("message_type", c_ulong), ("format", c_int),
                ("data", c_long * 5)]

class XEvent(Union):
    _fields_ = [("type", c_int), ("xclient", XClientMessageEvent),
                ("pad", c_long * 24)]

x11 = ctypes.cdll.LoadLibrary("libX11.so.6")
x11.XOpenDisplay.restype = c_void_p
x11.XInternAtom.restype = c_ulong
x11.XInternAtom.argtypes = [c_void_p, ctypes.c_char_p, c_int]
x11.XSendEvent.argtypes = [c_void_p, c_ulong, c_int, c_long, c_void_p]
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XFlush.argtypes = [c_void_p]
x11.XCloseDisplay.argtypes = [c_void_p]
dpy = x11.XOpenDisplay(None)
assert dpy, "XOpenDisplay failed"
wid = int(sys.argv[1])
ev = XEvent()
ev.xclient.type = 33  # ClientMessage
ev.xclient.display = dpy
ev.xclient.window = wid
ev.xclient.message_type = x11.XInternAtom(dpy, b"WM_PROTOCOLS", 0)
ev.xclient.format = 32
ev.xclient.data[0] = x11.XInternAtom(dpy, b"WM_DELETE_WINDOW", 0)
ev.xclient.data[1] = 0  # CurrentTime
assert x11.XSendEvent(dpy, wid, 0, 0, ctypes.byref(ev)) != 0, "XSendEvent failed"
x11.XFlush(dpy)
x11.XCloseDisplay(dpy)
PY
}

cleanup() {
    [[ -z $SET_WID ]] || wm_delete "$SET_WID" 2>/dev/null || true
    [[ -z "$CLIENT_PID" ]] || kill "$CLIENT_PID" 2>/dev/null || true
    xdotool windowfocus "$OLD_FOCUS" 2>/dev/null || true
}
trap cleanup EXIT

MARK="$(wc -l < "$IBUS_LOG")"
XMARK="$(wc -l < "$XIM_LOG")"
python3 "$ROOT/scripts/ime-probe.py" "$WORK/buffer.txt" "$LIFE" >"$WORK/probe.log" 2>&1 &
CLIENT_PID=$!
WID=""
for _ in {1..50}; do
    WID="$(xdotool search --all --onlyvisible --pid "$CLIENT_PID" --name '^lyyime-probe$' 2>/dev/null | head -1 || true)"
    [[ -z "$WID" ]] || break
    sleep .1
done
[[ -n "$WID" ]]

if [[ $MODE == paging ]]; then
    wait_text() {
        local want="$1"
        for _ in {1..30}; do
            [[ -f $WORK/buffer.txt && "$(cat "$WORK/buffer.txt")" == "$want" ]] && return 0
            sleep .1
        done
        echo "翻页验证:输入框内容与预期不符" >&2
        return 1
    }
    own_focus() { [[ "$(xdotool getwindowfocus)" == "$WID" ]]; }
    xdotool windowactivate --sync "$WID"
    sleep .3
    own_focus
    wait_text ""
    xdotool type --delay 160 yi
    sleep .6
    wait_text ""
    for key in minus equal equal minus Prior Next; do
        own_focus
        xdotool key "$key"
        sleep .4
        wait_text ""
    done
    own_focus
    xdotool key Escape
    sleep .4
    wait_text ""
    own_focus
    xdotool type --delay 160 nihao
    sleep .6
    own_focus
    xdotool key space
    wait_text "你好"
    own_focus
    xdotool key minus equal
    wait_text "你好-="
    CONFIG_SHA_AFTER="ABSENT"
    [[ -f $CONFIG ]] && CONFIG_SHA_AFTER="$(sha256sum "$CONFIG" | cut -d' ' -f1)"
    [[ $CONFIG_SHA_AFTER == "$CONFIG_SHA_BEFORE" ]]
    { [[ "$(session_xim_pid)" == "$PID" ]] && [[ "$(proc_start "$PID")" == "$PID_START" ]]; } \
        || { echo "收尾核验:XIM 进程身份/启动时间已变化" >&2; dump_procs; exit 1; }
    { mapfile -t EPIDS_END < <(engine_pids /proc "$ENGINE_EXE"); } \
        && [[ ${#EPIDS_END[@]} -eq 1 && ${EPIDS_END[0]} == "$ENGINE_PID" ]] \
        || { echo "收尾核验:引擎进程身份已变化/不唯一" >&2; dump_procs; exit 1; }
    [[ "$(panel_owner)" == "$PANEL_BASE" ]] \
        || { echo "收尾核验:面板属主变化" >&2; exit 1; }
    printf 'PASS paging: -=/PageUp/PageDown 组合中零上屏;正常选词你好;空闲 -= 直通;config/XIM/引擎/面板未变。Evidence: %s\n' "$WORK"
    exit 0
fi

if [[ $MODE == repeat-settings ]]; then
    EXPECT=""
    for i in 1 2 3; do
        EXPECT+="$TEXT"
        xdotool windowactivate --sync "$WID"
        sleep .3
        [[ "$(xdotool getwindowfocus)" == "$WID" ]]
        xdotool type --delay 160 "$CODE"
        sleep .5
        # autocommit may already commit; press space only if not yet committed.
        if ! grep -Fq -- "$EXPECT" "$WORK/buffer.txt" 2>/dev/null; then
            [[ "$(xdotool getwindowfocus)" == "$WID" ]]
            xdotool key space
        fi
        for _ in {1..30}; do
            [[ "$(cat "$WORK/buffer.txt" 2>/dev/null)" == "$EXPECT" ]] && break
            sleep .1
        done
        [[ "$(cat "$WORK/buffer.txt")" == "$EXPECT" ]]
        # 本轮必须出现新的菜单提示行
        for _ in {1..30}; do
            tail -n +"$((MARK+1))" "$IBUS_LOG" | grep -qE -- "$HINT" && break
            sleep .1
        done
        tail -n +"$((MARK+1))" "$IBUS_LOG" | grep -E -- "$HINT" > /dev/null
        MARK="$(wc -l < "$IBUS_LOG")"
        # F7 只在探针仍持焦点时按下(绝不落进用户应用)
        [[ "$(xdotool getwindowfocus)" == "$WID" ]]
        xdotool key F7
        # 等 XIM 拥有的可见设置窗 + 本轮新 settings show 日志
        SWID=""
        for _ in {1..50}; do
            SWID="$(xdotool search --all --onlyvisible --pid "$PID" --name "$SETTINGS_RE" 2>/dev/null | head -1 || true)"
            [[ -z $SWID ]] || break
            sleep .1
        done
        [[ -n $SWID ]] || { echo "第 $i 轮:设置窗未出现" >&2; exit 1; }
        SET_WID="$SWID"
        for _ in {1..50}; do
            tail -n +"$((XMARK+1))" "$XIM_LOG" | grep -qF 'settings show: 设置窗已呈现' && break
            sleep .1
        done
        tail -n +"$((XMARK+1))" "$XIM_LOG" | grep -F 'settings show: 设置窗已呈现' > /dev/null
        XMARK="$(wc -l < "$XIM_LOG")"
        # 不保存关闭(WM_DELETE),确认隐藏后焦点回探针
        wm_delete "$SET_WID"
        for _ in {1..50}; do
            [[ -z "$(xdotool search --all --onlyvisible --pid "$PID" --name "$SETTINGS_RE" 2>/dev/null || true)" ]] && break
            sleep .1
        done
        [[ -z "$(xdotool search --all --onlyvisible --pid "$PID" --name "$SETTINGS_RE" 2>/dev/null || true)" ]] \
            || { echo "第 $i 轮:设置窗未关闭" >&2; exit 1; }
        SET_WID=""
        # F7/关窗不得污染已提交文本(防重复/丢失 commit 回归)
        [[ "$(cat "$WORK/buffer.txt")" == "$EXPECT" ]] \
            || { echo "第 $i 轮:F7/关窗污染了已提交文本" >&2; exit 1; }
        [[ "$(panel_owner)" == "$PANEL_BASE" ]] \
            || { echo "第 $i 轮:面板属主变化" >&2; exit 1; }
        xdotool windowactivate --sync "$WID"
        [[ "$(xdotool getwindowfocus)" == "$WID" ]]
        echo "round $i ok:设置窗呈现并关闭,累计文本=$EXPECT"
    done
    # 收尾核验:config 字节不变、XIM/引擎 PID 不变、面板属主不变且连续 5s 有主
    CONFIG_SHA_AFTER="ABSENT"
    [[ -f $CONFIG ]] && CONFIG_SHA_AFTER="$(sha256sum "$CONFIG" | cut -d' ' -f1)"
    [[ $CONFIG_SHA_AFTER == "$CONFIG_SHA_BEFORE" ]] || { echo "config.toml 被改动" >&2; exit 1; }
    { [[ "$(session_xim_pid)" == "$PID" ]] && [[ "$(proc_start "$PID")" == "$PID_START" ]]; } \
        || { echo "收尾核验:XIM 进程身份/启动时间已变化" >&2; dump_procs; exit 1; }
    { mapfile -t EPIDS_END < <(engine_pids /proc "$ENGINE_EXE"); } \
        && [[ ${#EPIDS_END[@]} -eq 1 && ${EPIDS_END[0]} == "$ENGINE_PID" ]] \
        || { echo "收尾核验:引擎进程身份已变化/不唯一" >&2; dump_procs; exit 1; }
    for _ in {1..20}; do
        [[ "$(panel_owner || true)" == "$PANEL_BASE" ]] || { echo "面板属主变化/丢失" >&2; exit 1; }
        sleep .25
    done
    printf 'PASS repeat-settings: 3 轮 F7 设置窗呈现/关闭;config 未变;XIM/引擎/面板属主稳定(%s)。Evidence: %s\n' \
        "$PANEL_BASE" "$WORK"
    exit 0
fi

xdotool windowactivate --sync "$WID"
sleep .3
[[ "$(xdotool getwindowfocus)" == "$WID" ]]
xdotool type --delay 160 "$CODE"
sleep .5
# autocommit may already commit; press space only if not yet committed.
if ! grep -Fq -- "$TEXT" "$WORK/buffer.txt"; then
    [[ "$(xdotool getwindowfocus)" == "$WID" ]]
    xdotool key space
fi
for _ in {1..30}; do
    grep -Fq -- "$TEXT" "$WORK/buffer.txt" && break
    sleep .1
done
[[ "$(cat "$WORK/buffer.txt")" == "$TEXT" ]]
sleep 5
[[ "$(panel_owner)" == "$PANEL_BASE" ]]
tail -n +"$((MARK+1))" "$IBUS_LOG" > "$WORK/ibus-new.log"
grep -E -- "$HINT" "$WORK/ibus-new.log"
printf 'PASS: real GTK/IBus committed %s; expected shortcut hint logged; panel stable. Evidence: %s\n' "$TEXT" "$WORK"
