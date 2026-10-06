#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURES="$ROOT/crates/lyyime-core/tests/fixtures"
TGT="${CARGO_TARGET_DIR:-/data/cargo-target/local/codes_apps_lyyIme-93ecbcd94ded0a75}"
ENGINE_BIN="$TGT/release/ibus-engine-lyyime"
CORE_SO="$TGT/release/liblyyime_core.so"
XIM_BIN="$ROOT/xim/build/bin/lyyime-xim"
CLIENT_BIN="$ROOT/xim/build/tests/e2e_client"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1
WORK=""

fail() { KEEP=1; echo "CANDWIN-E2E FAIL: $*"; echo "(失败现场保留:${WORK:-未建})"; exit 1; }

[[ -x "$ENGINE_BIN" ]] || fail "引擎未构建:$ENGINE_BIN"
[[ -f "$CORE_SO" ]] || fail "真库不存在:$CORE_SO"
[[ -x "$CLIENT_BIN" ]] || fail "e2e_client 未构建:$CLIENT_BIN(make -C xim test)"
[[ -x "$XIM_BIN" ]] || fail "lyyime-xim 未构建:$XIM_BIN"
command -v Xvfb >/dev/null || fail "缺 Xvfb"
command -v xdotool >/dev/null || fail "缺 xdotool"
command -v xwininfo >/dev/null || fail "缺 xwininfo"
command -v ibus-daemon >/dev/null || fail "缺 ibus-daemon"
command -v python3 >/dev/null || fail "缺 python3"

WORK="$(mktemp -d /tmp/lyyime-candwin-e2e.XXXXXX)"
ART="$WORK/artifacts"
mkdir -p "$ART"
export HOME="$WORK/home"
export XDG_CONFIG_HOME="$WORK/xdg/config"
export XDG_DATA_HOME="$WORK/xdg/data"
export XDG_CACHE_HOME="$WORK/xdg/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"
unset NO_AT_BRIDGE || true
export LYYIME_RES_DIR="$ROOT/xim/res"

XVFB_PID=""
SESSION_PID=""
INNER_PID=""
CLIENT2_PID=""
cleanup() {
    [[ -n "${CLIENT2_PID:-}" ]] && { kill "$CLIENT2_PID" 2>/dev/null || true; wait "$CLIENT2_PID" 2>/dev/null || true; }
    [[ -n "${INNER_PID:-}" ]] && { kill "$INNER_PID" 2>/dev/null || true; wait "$INNER_PID" 2>/dev/null || true; }
    [[ -n "${SESSION_PID:-}" ]] && { kill "$SESSION_PID" 2>/dev/null || true; wait "$SESSION_PID" 2>/dev/null || true; }
    [[ -n "${XVFB_PID:-}" ]] && { kill "$XVFB_PID" 2>/dev/null || true; wait "$XVFB_PID" 2>/dev/null || true; }
    if [[ $KEEP -eq 1 ]]; then
        echo "[candwin-e2e] 现场保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

Xvfb -displayfd 3 -screen 0 1024x768x24 -nolisten tcp 3>"$WORK/xvfb.dpy" 2>"$ART/xvfb.log" &
XVFB_PID=$!
for _ in $(seq 1 50); do [[ -s "$WORK/xvfb.dpy" ]] && break; sleep 0.1; done
[[ -s "$WORK/xvfb.dpy" ]] || fail "Xvfb 未报告 display"
DISP=":$(tr -d ' \n' <"$WORK/xvfb.dpy")"
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X${DISP#:}" ]] && break; sleep 0.1; done
export DISPLAY="$DISP" GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8
export LYYIME_DEBUG=1
unset IBUS_ADDRESS || true

DICT="$WORK/dict"; mkdir -p "$DICT"; cp "$FIXTURES"/* "$DICT"/
printf 'yi\t一\t8000\nyi\t衣\t7000\nyi\t依\t6000\nyi\t医\t5000\nyi\t伊\t4000\nyi\t易\t3000\n' >> "$DICT/pinyin_char.tsv"
export LYYIME_DATA_DIR="$DICT" LYYIME_CORE_LIB="$CORE_SO"
export LYYIME_ENGINE_STATE_FILE="$WORK/engine-state.json"
CFG_PATH="$XDG_CONFIG_HOME/lyyime/config.toml"
mkdir -p "$XDG_CONFIG_HOME/lyyime"
cat > "$CFG_PATH" <<'CFG'
commit_after_four = false
commit_first_at_four = false
commit_unique_four = false
phrase_hint = false
learning = true
page_size = 5
CFG
CFG_SHA0="$(sha256sum "$CFG_PATH" | cut -d' ' -f1)"

ENGINE_HOME="$HOME/.local/share/lyyime/ibus/engine"
ICON_HOME="$HOME/.local/share/lyyime/ibus/icons"
mkdir -p "$ENGINE_HOME" "$ICON_HOME" "$HOME/.local/share/ibus/component"
install -m 755 "$ENGINE_BIN" "$ENGINE_HOME/"
install -m 644 "$ROOT"/ibus-engine/icons/*.svg "$ICON_HOME/" 2>/dev/null || true
write_component() {
    sed -e "s|@ENGINE_EXEC@|$1|g" \
        -e "s|@ICON_DIR@|$ICON_HOME|g" \
        -e "s|@SETUP@|/nonexistent|g" \
        "$ROOT/ibus-engine/lyyime.xml" > "$2/lyyime.xml"
}
write_component "$ENGINE_HOME/ibus-engine-lyyime" "$HOME/.local/share/ibus/component"
export IBUS_COMPONENT_PATH="$HOME/.local/share/ibus/component:/usr/share/ibus/component"

mkdir -p "$WORK/bin"
ln -sf "$XIM_BIN" "$WORK/bin/lyyime-xim"
export PATH="$WORK/bin:$PATH"
IBUS_LOG="$HOME/.local/share/lyyime/logs/ibus.log"
BUFFER="$WORK/buffer.txt"
export CLIENT_BIN BUFFER WORK

ATSPI_PY="$WORK/atspi.py"
cat > "$ATSPI_PY" <<'PY'
import sys
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

ROLE_MAP = {
    "menu item": Atspi.Role.MENU_ITEM,
    "check menu item": Atspi.Role.CHECK_MENU_ITEM,
    "radio menu item": Atspi.Role.RADIO_MENU_ITEM,
    "push button": Atspi.Role.PUSH_BUTTON,
    "toggle button": Atspi.Role.TOGGLE_BUTTON,
    "label": Atspi.Role.LABEL,
    "any": None,
}

def showing(node):
    try:
        st = node.get_state_set()
        return st.contains(Atspi.StateType.SHOWING) and st.contains(Atspi.StateType.VISIBLE)
    except Exception:
        return False

def match_role(node, roles):
    if "any" in roles:
        return True
    try:
        if node.get_role_name() in roles:
            return True
        enum = node.get_role()
    except Exception:
        return False
    return any(ROLE_MAP.get(r) == enum for r in roles)

def walk(node, label, roles, out, depth):
    if depth > 30:
        return
    try:
        name = node.get_name() or ""
    except Exception:
        name = ""
    if (name == label or (CONTAINS and label in name)) and match_role(node, roles) and showing(node):
        try:
            comp = node.get_component_iface()
            ext = comp.get_extents(Atspi.CoordType.SCREEN)
            out.append("%d %d %d %d" % (
                ext.x + ext.width // 2, ext.y + ext.height // 2,
                ext.width, ext.height))
        except Exception:
            pass
    try:
        for i in range(node.get_child_count()):
            walk(node.get_child_at_index(i), label, roles, out, depth + 1)
    except Exception:
        pass

label = sys.argv[1]
roles = set(sys.argv[2].split(","))
CONTAINS = len(sys.argv) > 3 and sys.argv[3] == "contains"
PID = int(sys.argv[4]) if len(sys.argv) > 4 else 0
desktop = Atspi.get_desktop(0)
out = []
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    try:
        if PID and app.get_process_id() != PID:
            continue
    except Exception:
        continue
    walk(app, label, roles, out, 0)
if out:
    print(out[0])
    sys.exit(0)
sys.exit(1)
PY

SHOT_PY="$WORK/shot.py"
cat > "$SHOT_PY" <<'PY'
import sys
import gi
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk
Gdk.init([])
root = Gdk.get_default_root_window()
pb = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
assert pb is not None
pb.savev(sys.argv[1], 'png', [], [])
PY

# CapsLock 锁存位只读探针(T12b):libX11 ctypes XkbGetState,
# 打印 locked_mods & LockMask(2=锁存/0=未锁)
CAPS_PY="$WORK/caps_state.py"
cat > "$CAPS_PY" <<'PY'
import ctypes, sys, time
lib = ctypes.CDLL("libX11.so.6")
class XkbStateRec(ctypes.Structure):
    _fields_ = [
        ("group", ctypes.c_ubyte), ("locked_group", ctypes.c_ubyte),
        ("base_group", ctypes.c_ushort), ("latched_group", ctypes.c_ushort),
        ("mods", ctypes.c_ubyte), ("base_mods", ctypes.c_ubyte),
        ("latched_mods", ctypes.c_ubyte), ("locked_mods", ctypes.c_ubyte),
        ("compat_state", ctypes.c_ubyte), ("grab_mods", ctypes.c_ubyte),
        ("compat_grab_mods", ctypes.c_ubyte), ("lookup_mods", ctypes.c_ubyte),
        ("compat_lookup_mods", ctypes.c_ubyte), ("ptr_buttons", ctypes.c_ushort),
    ]
lib.XOpenDisplay.restype = ctypes.c_void_p
lib.XOpenDisplay.argtypes = [ctypes.c_char_p]
lib.XkbGetState.restype = ctypes.c_int
lib.XkbGetState.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                          ctypes.POINTER(XkbStateRec)]
lib.XKeysymToKeycode.restype = ctypes.c_ubyte
lib.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
lib.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
lib.XCloseDisplay.argtypes = [ctypes.c_void_p]

# 原生单键事件:--shift <Shift_L|Shift_R|a> <click|down|up>
# XKeysymToKeycode→XTestFakeKeyEvent 直发 press/release;不像 xdotool
# key 那样为凑修饰位补发其它修饰键 —— 保证 Caps 锁存下"干净 Shift
# 单击/按住"合同。不改任何修饰映射,不做 Xkb 状态查询。
if sys.argv[1:2] == ["--shift"]:
    KEYS = {"Shift_L": 0xFFE1, "Shift_R": 0xFFE2, "a": 0x61}
    if len(sys.argv) != 4 or sys.argv[2] not in KEYS \
            or sys.argv[3] not in ("click", "down", "up"):
        sys.exit(2)
    tst = ctypes.CDLL("libXtst.so.6")
    tst.XTestFakeKeyEvent.restype = ctypes.c_int
    tst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                                    ctypes.c_int, ctypes.c_ulong]
    dpy = lib.XOpenDisplay(None)
    if not dpy:
        sys.exit(2)
    kc = lib.XKeysymToKeycode(dpy, KEYS[sys.argv[2]])
    if kc == 0:
        lib.XCloseDisplay(dpy)
        sys.exit(2)
    act, ok = sys.argv[3], 1
    if act != "up":
        ok &= tst.XTestFakeKeyEvent(dpy, kc, 1, 0)
        lib.XSync(dpy, 0)
        if act == "click":
            time.sleep(0.012)
    if act != "down":
        ok &= tst.XTestFakeKeyEvent(dpy, kc, 0, 0)
    lib.XSync(dpy, 0)
    lib.XCloseDisplay(dpy)
    sys.exit(0 if ok else 2)

dpy = lib.XOpenDisplay(None)
if not dpy:
    sys.exit(2)
st = XkbStateRec()
rc = lib.XkbGetState(dpy, 0x100, ctypes.byref(st))  # XkbUseCoreKbd
lib.XCloseDisplay(dpy)
if rc != 0:
    sys.exit(2)
print(st.locked_mods & 2)
PY

caps_mask() { python3 "$CAPS_PY" 2>/dev/null || echo "ERR"; }
# 原生单键事件(T12b 前提):xdotool key 会为凑修饰位补发其它修饰键
# (实测 Shift_R 先带出 Shift_L 按下),破坏干净单击;这里直发 XTest。
key_event() { python3 "$CAPS_PY" --shift "$1" "${2:-click}"; }
wait_caps_mask() {
    local want="$1" i
    for ((i = 0; i < 60; i++)); do
        [[ "$(caps_mask)" == "$want" ]] && return 0
        sleep 0.1
    done
    fail "CapsLock 锁存位未达 $want(当前 $(caps_mask))"
}

SESS_BUS=""
ENGINE_PID=""
atspi_find() { DBUS_SESSION_BUS_ADDRESS="$SESS_BUS" timeout 8 python3 "$ATSPI_PY" "$1" "$2" "${3:-}" "${4:-$ENGINE_PID}" 2>/dev/null; }
atspi_click() {
    local xy cx cy cw ch
    xy="$(atspi_find "$1" "$2" "${3:-}" "${4:-}")" || return 1
    read -r cx cy cw ch <<<"$xy"
    xdotool mousemove "$cx" "$cy" click 1
}
atspi_rclick() {
    local xy cx cy cw ch
    xy="$(atspi_find "$1" "$2" "${3:-}" "${4:-}")" || return 1
    read -r cx cy cw ch <<<"$xy"
    xdotool mousemove "$cx" "$cy" click 3
}

shot() {
    python3 "$SHOT_PY" "$ART/$1.png" || fail "截图 $1 采集失败"
}

wait_log() {
    local pat="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        grep -q "$pat" "$IBUS_LOG" 2>/dev/null && return 0
        sleep 0.05
    done
    tail -40 "$IBUS_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 超时"
}

# 按出现次数等待日志(ibus.log 跨运行追加,防同串陈旧行误配)
wait_log_count() {
    local pat="$1" want="$2" timeout="${3:-10}" i n
    for ((i = 0; i < timeout * 20; i++)); do
        n="$(grep -c "$pat" "$IBUS_LOG" 2>/dev/null || true)"
        [[ ${n:-0} -ge $want ]] && return 0
        sleep 0.05
    done
    tail -40 "$IBUS_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 次数≥$want 超时"
}
wait_buf() {
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ -f "$BUFFER" ]] && grep -qF "$want" "$BUFFER" && return 0
        sleep 0.05
    done
    cat "$BUFFER" 2>/dev/null || true
    fail "等待缓冲 [$want] 超时"
}
buf_text() { cat "$BUFFER" 2>/dev/null || true; }
wait_appended() {
    local base="$1" want="$2" timeout="${3:-10}" i cur
    for ((i = 0; i < timeout * 20; i++)); do
        cur="$(buf_text)"
        if [[ "$cur" == "$base$want"* && "$cur" != "$base" ]]; then
            return 0
        fi
        sleep 0.05
    done
    echo "---- buffer ----"; buf_text
    fail "缓冲未精确追加 [$want](原值:[$base])"
}

candwin_geom() {
    local ids w info
    ids="$(xwininfo -root -children 2>/dev/null | grep '"lyyime-candwin"' \
        | awk '{print $1}' || true)"
    while IFS= read -r w; do
        [[ -n "$w" ]] || continue
        info="$(xwininfo -id "$w" 2>/dev/null || true)"
        grep -q "Map State: IsViewable" <<<"$info" || continue
        X="$(awk '/Absolute upper-left X/{print $NF}' <<<"$info")"
        Y="$(awk '/Absolute upper-left Y/{print $NF}' <<<"$info")"
        WIDTH="$(awk '/Width:/{print $NF}' <<<"$info")"
        HEIGHT="$(awk '/Height:/{print $NF}' <<<"$info")"
        CWID="$w"
        [[ -n "$X" && -n "$WIDTH" ]] && return 0
    done <<<"$ids"
    return 1
}
wait_candwin() {
    local i
    for ((i = 0; i < 100; i++)); do
        candwin_geom && return 0
        sleep 0.1
    done
    shot "candwin-missing"
    fail "自有候选窗(lyyime-candwin)未映射为可见"
}
wait_no_candwin() {
    local i
    for ((i = 0; i < 100; i++)); do
        candwin_geom || return 0
        sleep 0.1
    done
    shot "candwin-stuck"
    fail "自有候选窗应隐藏仍可见"
}

other_or_window() {
    local w info pid
    for w in $(xwininfo -root -children 2>/dev/null | awk '/^ *0x[0-9a-f]+ /{print $1}'); do
        [[ "$w" == "${CWID:-0x0}" ]] && continue
        info="$(xwininfo -id "$w" 2>/dev/null || true)"
        grep -q "Map State: IsViewable" <<<"$info" || continue
        grep -q "Override Redirect State: yes" <<<"$info" || continue
        pid="$(xdotool getwindowpid "$w" 2>/dev/null || echo 0)"
        [[ "$pid" == "${ENGINE_PID:-0}" ]] && continue
        echo "$w"
        return 0
    done
    return 1
}
engine_or_window() {
    local w info pid
    for w in $(xwininfo -root -children 2>/dev/null | awk '/^ *0x[0-9a-f]+ /{print $1}'); do
        [[ "$w" == "${CWID:-0x0}" ]] && continue
        info="$(xwininfo -id "$w" 2>/dev/null || true)"
        grep -q "Map State: IsViewable" <<<"$info" || continue
        grep -q "Override Redirect State: yes" <<<"$info" || continue
        pid="$(xdotool getwindowpid "$w" 2>/dev/null || echo 0)"
        [[ "$pid" == "${ENGINE_PID:-0}" ]] && { echo "$w"; return 0; }
    done
    return 1
}
wait_menu() {
    local i
    for ((i = 0; i < 80; i++)); do
        engine_or_window >/dev/null && return 0
        sleep 0.1
    done
    shot "menu-missing"
    fail "GTK 菜单未弹出"
}
wait_no_menu() {
    local i
    for ((i = 0; i < 60; i++)); do
        engine_or_window >/dev/null || return 0
        sleep 0.1
    done
    fail "GTK 菜单应关闭仍可见"
}

refocus_client() {
    local w
    w="$(xdotool search --name '^lyyime-e2e-client$' | head -1 || true)"
    [[ -n "$w" ]] && xdotool windowfocus "$w" && sleep 0.4
}

settings_visible_wid() {
    local w
    for w in $(xwininfo -root -children 2>/dev/null | grep 'lyyIme 输入法设置' | awk '{print $1}'); do
        xwininfo -id "$w" 2>/dev/null | grep -q "Map State: IsViewable" && {
            echo "$w"; return 0;
        }
    done
    return 1
}

echo "== [setup] dbus-run-session + ibus-daemon(-rx 前台) =="
dbus-run-session -- bash -c '
    set -uo pipefail
    echo "INNER_PID=$$"
    echo "SESS_BUS=$DBUS_SESSION_BUS_ADDRESS"
    ibus-daemon -rx >/dev/null 2>&1 &
    IBUS_PID=$!
    CLIENT_PID=""
    trap "kill $IBUS_PID \$CLIENT_PID 2>/dev/null || true" EXIT TERM INT
    ready=0
    for _ in $(seq 1 100); do
        f=$(ls "$XDG_CONFIG_HOME"/ibus/bus/*-unix-* 2>/dev/null | head -1)
        [[ -n "$f" ]] && grep -q "^IBUS_ADDRESS=" "$f" && { ready=1; break; }
        sleep 0.2
    done
    [[ "$ready" = 1 ]] || { echo "RUN-FAIL: ibus 地址未就绪"; exit 1; }
    export IBUS_ADDRESS="$(sed -n 's/^IBUS_ADDRESS=//p' "$f" | head -1)"
    for _ in $(seq 1 100); do
        ibus list-engine 2>/dev/null | grep -q "lyyime - " && break
        sleep 0.2
    done
    for _ in $(seq 1 10); do
        ibus engine lyyime 2>/dev/null || true
        [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] && break
        sleep 1
    done
    [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] || { echo "RUN-FAIL: 引擎未激活"; exit 1; }
    echo "SESSION-READY"
    "$CLIENT_BIN" "$BUFFER" 300 >"$WORK/client.log" 2>&1 &
    CLIENT_PID=$!
    echo "CLIENT_PID_INNER=$CLIENT_PID"
    sleep 1.2
    WID="$(xdotool search --name "^lyyime-e2e-client$" | head -1 || true)"
    [[ -n "$WID" ]] && xdotool windowfocus "$WID"
    sleep infinity &
    wait $!
' >"$WORK/session.log" 2>&1 &
SESSION_PID=$!
for _ in $(seq 1 150); do
    grep -q "CLIENT_PID_INNER=" "$WORK/session.log" 2>/dev/null && break
    sleep 0.2
done
grep -q "CLIENT_PID_INNER=" "$WORK/session.log" || { cat "$WORK/session.log"; fail "隔离会话未就绪"; }
SESS_BUS="$(sed -n 's/^SESS_BUS=//p' "$WORK/session.log" | head -1)"
INNER_PID="$(sed -n 's/^INNER_PID=//p' "$WORK/session.log" | head -1)"
ENGINE_PID="$(pgrep -f "$ENGINE_HOME/ibus-engine-lyyime" | head -1 || true)"
[[ -n "$ENGINE_PID" && -n "$INNER_PID" ]] || { cat "$WORK/session.log"; fail "引擎/内层进程未就绪"; }
sleep 0.6
refocus_client

echo "== [T1] nihao → 自有候选窗可见,原生 lookup 缺席 =="
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
other_or_window >/dev/null && { shot "native-lookup"; fail "原生 ibus lookup 与自有候选窗并存"; }
shot "t1-candwin"
echo "PASS T1:lyyime-candwin 可见(宽=$WIDTH 高=$HEIGHT),原生 lookup 缺席"

echo "== [T2] 词行右键 → 纯词菜单(AT-SPI 存在+缺失,SHOWING 过滤) =="
atspi_rclick "你好" "label" || fail "AT-SPI 定位候选词 你好 失败"
sleep 0.5
wait_menu
wait_log '候选窗菜单:词'
atspi_find "固定首位" "menu item" >/dev/null || fail "词菜单缺 固定首位"
atspi_find "删除词组" "menu item" >/dev/null || fail "词菜单缺 删除词组"
atspi_find "反查英文" "menu item" >/dev/null || fail "词菜单缺 反查英文"
atspi_find "设置…" "menu item" >/dev/null && fail "词菜单混入 设置…(通用动作泄漏)"
atspi_find "重载词库" "menu item" >/dev/null && fail "词菜单混入 重载词库"
shot "t2-word-menu"
echo "PASS T2:词菜单纯词操作(固定/删除/反查),无通用动作"

echo "== [T3] Esc 取消 → 无提交,nihao+Space 精确追加 你好 =="
B0="$(buf_text)"
xdotool key Escape; sleep 0.5
wait_no_menu
[[ "$(buf_text)" == "$B0" ]] || fail "菜单取消产生了上屏"
xdotool key space; sleep 0.6
wait_appended "$B0" "你好"
echo "PASS T3:菜单取消零提交,Space 上屏精确追加 你好"

echo "== [T4] 输入串区右键(同排非词区)→ 纯通用菜单 =="
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
atspi_rclick "nihao" "label" || fail "AT-SPI 定位输入串区失败"
sleep 0.5
wait_menu
wait_log '候选窗菜单:通用'
atspi_find "设置…" "menu item" >/dev/null || fail "通用菜单缺 设置…"
atspi_find "重载词库" "menu item" >/dev/null || fail "通用菜单缺 重载词库"
atspi_find "固定首位" "menu item" >/dev/null && fail "通用菜单混入 固定首位"
atspi_find "删除词组" "menu item" >/dev/null && fail "通用菜单混入 删除词组"
shot "t4-general-menu"
xdotool key Escape; sleep 0.4
wait_no_menu
echo "PASS T4:输入串区右键纯通用菜单(设置/重载),无词操作"

echo "== [T5] 系统区(候选系统区)右键 → 通用菜单 =="
atspi_rclick "候选系统区" "any" || fail "AT-SPI 定位 候选系统区 失败"
sleep 0.5
wait_menu
atspi_find "设置…" "menu item" >/dev/null || fail "系统区右键未弹通用菜单"
atspi_find "固定首位" "menu item" >/dev/null && fail "系统区菜单混入词操作"
xdotool key Escape; sleep 0.4
wait_no_menu
B0="$(buf_text)"
atspi_rclick "打开设置" "any" || fail "T5 齿轮右键定位失败"
sleep 0.5
wait_menu
atspi_find "设置…" "menu item" >/dev/null || fail "齿轮右键未弹通用菜单"
atspi_find "重载词库" "menu item" >/dev/null || fail "齿轮右键菜单缺 重载词库"
atspi_find "固定首位" "menu item" >/dev/null && fail "齿轮右键菜单混入词操作"
atspi_find "删除词组" "menu item" >/dev/null && fail "齿轮右键菜单混入词操作"
atspi_find "反查英文" "menu item" >/dev/null && fail "齿轮右键菜单混入词操作"
xdotool key Escape; sleep 0.4
wait_no_menu
[[ "$(buf_text)" == "$B0" ]] || fail "齿轮右键菜单取消产生上屏"
echo "PASS T5:系统区+齿轮右键 → 通用菜单(无词操作),取消零提交"

echo "== [T6] 最右齿轮 → 真实设置窗,取消隐藏配置/缓冲不变 + 二次打开 =="
GEAR_XY="$(atspi_find "打开设置" "push button")" \
    || GEAR_XY="$(atspi_find "打开设置" "any")" || fail "AT-SPI 定位齿轮失败"
read -r GX GY GW GH <<<"$GEAR_XY"
for bn in "候选下一页" "候选上一页"; do
    bxy="$(atspi_find "$bn" "push button" "" "" || true)"
    [[ -n "$bxy" ]] && { read -r bx by bw bh <<<"$bxy"; (( GX > bx + bw / 2 )) || fail "齿轮不在 $bn 右侧"; }
done
B0="$(buf_text)"
xdotool mousemove "$GX" "$GY" click 1; sleep 1.0
wait_log '候选窗:齿轮打开设置'
SWID=""
for _ in $(seq 1 40); do
    SWID="$(settings_visible_wid || true)"
    [[ -n "$SWID" ]] && break
    sleep 0.25
done
[[ -n "$SWID" ]] || { shot "settings-missing"; fail "真实设置窗未映射为可见"; }
shot "t6-settings"
SPID="$(xdotool getwindowpid "$SWID" 2>/dev/null || echo 0)"
if [[ "$SPID" != "0" ]] && atspi_click "取消" "push button" "" "$SPID"; then
    sleep 0.6
else
    xdotool windowfocus "$SWID" 2>/dev/null || true
    xdotool key Escape; sleep 0.6
fi
for _ in $(seq 1 25); do
    xwininfo -id "$SWID" 2>/dev/null | grep -q "Map State: IsViewable" || break
    sleep 0.2
done
xwininfo -id "$SWID" 2>/dev/null | grep -q "Map State: IsViewable" \
    && fail "设置窗取消后仍可见(隐藏即关闭语义)"
shot "t6-settings-hidden"
[[ "$(buf_text)" == "$B0" ]] || fail "设置打开/取消期间产生上屏"
[[ "$(sha256sum "$CFG_PATH" | cut -d' ' -f1)" == "$CFG_SHA0" ]] \
    || fail "设置取消后 config.toml 被改动"
refocus_client
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
GEAR_XY="$(atspi_find "打开设置" "push button")" \
    || GEAR_XY="$(atspi_find "打开设置" "any")" || fail "二次打开前 AT-SPI 定位齿轮失败"
read -r GX GY GW GH <<<"$GEAR_XY"
B0="$(buf_text)"
xdotool mousemove "$GX" "$GY" click 1; sleep 1.0
SWID=""
for _ in $(seq 1 40); do
    SWID="$(settings_visible_wid || true)"
    [[ -n "$SWID" ]] && break
    sleep 0.25
done
[[ -n "$SWID" ]] || { shot "settings-reopen-missing"; fail "设置窗二次打开未复现"; }
shot "t6-settings-reopen"
SPID="$(xdotool getwindowpid "$SWID" 2>/dev/null || echo 0)"
if [[ "$SPID" != "0" ]] && atspi_click "取消" "push button" "" "$SPID"; then
    sleep 0.6
else
    xdotool windowfocus "$SWID" 2>/dev/null || true
    xdotool key Escape; sleep 0.6
fi
for _ in $(seq 1 25); do
    xwininfo -id "$SWID" 2>/dev/null | grep -q "Map State: IsViewable" || break
    sleep 0.2
done
xwininfo -id "$SWID" 2>/dev/null | grep -q "Map State: IsViewable" \
    && fail "设置窗二次取消后仍可见"
shot "t6-settings-hidden2"
[[ "$(buf_text)" == "$B0" ]] || fail "二次设置期间产生上屏"
[[ "$(sha256sum "$CFG_PATH" | cut -d' ' -f1)" == "$CFG_SHA0" ]] \
    || fail "二次设置取消后 config.toml 被改动"
refocus_client
echo "PASS T6:齿轮→真实设置窗开/关两次,配置与缓冲不变,齿轮最右"

echo "== [T7] 通用菜单切纯拼音 → aa 零候选仍显条+齿轮+系统区 =="
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
atspi_rclick "候选系统区" "any" || fail "AT-SPI 定位 候选系统区 失败"
sleep 0.5
wait_menu
atspi_click "输入方案" "menu item" "contains" || fail "通用菜单缺 输入方案 项"
sleep 0.6
wait_log '候选菜单:输入方案 → true(已保存)'
grep -q "^pinyin_only = true" "$CFG_PATH" || fail "pinyin_only 未落盘"
for _ in $(seq 1 40); do
    atspi_find "已切换:纯拼音" "label" >/dev/null && break
    sleep 0.25
done
atspi_find "已切换:纯拼音" "label" >/dev/null \
    || fail "组合清空后切换提示标签未显示(隐藏帧 Notice 缺失)"
B0="$(buf_text)"
xdotool key Escape; sleep 0.3
xdotool type --delay 90 "aa"; sleep 0.8
wait_candwin
atspi_find "aa" "label" >/dev/null || fail "aa 输入串未在辅助区显示"
atspi_find "已切换:纯拼音" "label" >/dev/null \
    && fail "旧切换提示未被新输入清除(sticky notice)"
[[ "$(buf_text)" == "$B0" ]] || fail "零候选输入却产生上屏"
if atspi_find "1." "label" >/dev/null || atspi_find "0." "label" >/dev/null; then
    fail "纯拼音 aa 出现可见候选行"
fi
atspi_find "打开设置" "any" >/dev/null || fail "零候选态齿轮缺失"
atspi_rclick "候选系统区" "any" || fail "零候选态系统区定位失败"
sleep 0.5
wait_menu
atspi_find "设置…" "menu item" >/dev/null || fail "零候选系统区右键未弹通用菜单"
xdotool key Escape; sleep 0.4
xdotool key Escape; sleep 0.3
xdotool key BackSpace BackSpace; sleep 0.4
echo "PASS T7:纯拼音 aa 零候选,条/齿轮/系统菜单可用"

echo "== [T8] 纯拼音态 nihao 连续 → 你好 + 词菜单固定写 pinned.tsv =="
B0="$(buf_text)"
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
atspi_rclick "你好" "label" || fail "AT-SPI 定位候选词 你好 失败"
sleep 0.5
wait_menu
wait_log '候选窗菜单:词'
atspi_click "固定首位" "menu item" || fail "AT-SPI 点选 固定首位 失败"
sleep 0.6
wait_log '候选窗菜单点选:abs=0'
PINF="$HOME/.local/share/lyyime/pinned.tsv"
for _ in $(seq 1 20); do [[ -f "$PINF" ]] && break; sleep 0.2; done
[[ -f "$PINF" ]] || fail "pinned.tsv 未生成"
grep -qP '^\S+\t你好$' "$PINF" || { cat "$PINF"; fail "pinned.tsv 缺 code<TAB>你好"; }
xdotool key Escape; sleep 0.3
B1="$(buf_text)"
xdotool type --delay 90 "nihao"; sleep 0.6
xdotool key space; sleep 0.5
wait_appended "$B1" "你好"
echo "PASS T8:固定首位落盘 pinned.tsv,组合流连续精确追加 你好"

echo "== [T9] ‹ › 翻页箭头(accessible 名命中) =="
xdotool type --delay 90 "yi"; sleep 0.8
wait_candwin
atspi_click "候选下一页" "push button" || fail "AT-SPI 定位 候选下一页 失败"
sleep 0.6
wait_log '候选窗面板翻页:下一页 → 第 2/'
atspi_click "候选上一页" "push button" || fail "AT-SPI 定位 候选上一页 失败"
sleep 0.6
wait_log '候选窗面板翻页:上一页 → 第 1/'
echo "PASS T9:候选条 ‹ › 按钮翻页"

echo "== [T10] 通用菜单切回混输 → 数字 0 越界不上屏 + 数字 2 精确追加 稻 =="
xdotool key Escape; sleep 0.3
B0="$(buf_text)"
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
atspi_rclick "候选系统区" "any" || fail "AT-SPI 定位 候选系统区 失败"
sleep 0.5
wait_menu
atspi_click "输入方案" "menu item" "contains" || fail "通用菜单缺 输入方案 项"
sleep 0.6
wait_no_menu
wait_log '候选菜单:输入方案 → false(已保存)'
grep -q "^pinyin_only = false" "$CFG_PATH" || fail "pinyin_only 未切回 false"
[[ "$(buf_text)" == "$B0" ]] || fail "方案切换产生了上屏"
xdotool key Escape; sleep 0.3
xdotool type --delay 90 "dh"; sleep 0.8
B0="$(buf_text)"
xdotool key 0; sleep 0.4
[[ "$(buf_text)" == "$B0" ]] || fail "数字 0 越界却改了缓冲"
xdotool key 2; sleep 0.5
wait_appended "$B0" "稻"
echo "PASS T10:数字 0 越界不上屏,数字 2 选词精确追加 稻"

echo "== [T11] 失焦隐藏 + 重聚焦恢复 =="
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
B0="$(buf_text)"
DBUS_SESSION_BUS_ADDRESS="$SESS_BUS" "$CLIENT_BIN" "$WORK/buffer2.txt" 8 >>"$WORK/client.log" 2>&1 &
CLIENT2_PID=$!
sleep 1.2
WID2="$(xdotool search --name '^lyyime-e2e-client$' | tail -1 || true)"
[[ -n "$WID2" ]] && xdotool windowfocus "$WID2"
sleep 1.0
wait_no_candwin
[[ "$(buf_text)" == "$B0" ]] || fail "失焦期间产生上屏"
kill "$CLIENT2_PID" 2>/dev/null || true; wait "$CLIENT2_PID" 2>/dev/null || true; CLIENT2_PID=""
sleep 0.8
refocus_client
B0="$(buf_text)"
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
xdotool key space; sleep 0.5
wait_appended "$B0" "你好"
echo "PASS T11:FocusOut 隐藏候选窗,重聚焦后组合/上屏正常"

echo "== [T12] 菜单打开中失焦 → 弹层取消且不上屏 =="
xdotool type --delay 90 "nihao"; sleep 0.8
wait_candwin
atspi_rclick "你好" "label" || fail "AT-SPI 定位候选词 你好 失败"
sleep 0.5
wait_menu
B0="$(buf_text)"
DBUS_SESSION_BUS_ADDRESS="$SESS_BUS" "$CLIENT_BIN" "$WORK/buffer3.txt" 6 >>"$WORK/client.log" 2>&1 &
CLIENT2_PID=$!
sleep 1.2
WID2=""
for _ in $(seq 1 40); do
    WID2="$(xdotool search --onlyvisible --pid "$CLIENT2_PID" --name '^lyyime-e2e-client$' 2>/dev/null | head -1 || true)"
    [[ -n "$WID2" ]] && break
    sleep 0.2
done
[[ -n "$WID2" ]] || fail "CLIENT2 自有窗口(PID+标题)未出现"
xdotool windowfocus "$WID2"
sleep 0.6
wait_no_menu
wait_no_candwin
[[ "$(buf_text)" == "$B0" ]] || fail "失焦期间产生上屏"
kill "$CLIENT2_PID" 2>/dev/null || true; wait "$CLIENT2_PID" 2>/dev/null || true; CLIENT2_PID=""
sleep 0.8
refocus_client
B0="$(buf_text)"
xdotool type --delay 90 "nihao"; sleep 0.6
xdotool key space; sleep 0.5
wait_appended "$B0" "你好"
echo "PASS T12:菜单态失焦取消无提交,重聚焦连续输入正常"

echo "== [T12b] CapsLock 联动:英文→中文 Shift 单击确认解锁 =="
B0="$(buf_text)"
UNLK_N="$(grep -c 'CapsLock 已解除' "$IBUS_LOG" 2>/dev/null || true)"
xdotool key Caps_Lock; sleep 0.5
[[ "$(caps_mask)" == "2" ]] || fail "Caps_Lock 后锁存位非 2($(caps_mask))"
# 中→英:锁存必须保持(该方向从不动锁)
key_event Shift_L; sleep 0.8
[[ "$(caps_mask)" == "2" ]] || fail "中→英 单击误清 CapsLock"
# 英→中(Shift_R 变体):release 确认 → 宿主经 XkbLockModifiers 解锁
key_event Shift_R; sleep 0.6
wait_log_count 'CapsLock 已解除' "$((${UNLK_N:-0} + 1))"
wait_caps_mask 0
# 解锁后立即打 fixture 中文:nihao+space 精确追加 你好
xdotool type --delay 90 "nihao"; sleep 0.8
xdotool key space; sleep 0.5
wait_appended "$B0" "你好"
# Shift_L 变体同合同:锁存态中→英(保持)→ 英→中(解除)
xdotool key Caps_Lock; sleep 0.4
[[ "$(caps_mask)" == "2" ]] || fail "Caps_Lock 重开后锁存位非 2"
key_event Shift_L; sleep 0.8
[[ "$(caps_mask)" == "2" ]] || fail "Shift_L 中→英 误清 CapsLock"
key_event Shift_L; sleep 0.6
wait_caps_mask 0
# 组合键负向:英文态锁存 → Shift+字母 与 Ctrl+Shift 均不切换不解锁
xdotool key Caps_Lock; sleep 0.4
[[ "$(caps_mask)" == "2" ]] || fail "Caps_Lock 第三次开后锁存位非 2"
key_event Shift_L; sleep 0.8   # 中→英(锁保持)
[[ "$(caps_mask)" == "2" ]] || fail "组合前置中→英 误清 CapsLock"
key_event Shift_L down; sleep 0.2
key_event a; sleep 0.3         # Shift+字母:取消单击,字母直通(原生按键不改修饰)
key_event Shift_L up; sleep 0.4
[[ "$(caps_mask)" == "2" ]] || fail "Shift+字母 组合误清 CapsLock"
xdotool key ctrl+shift; sleep 0.4
[[ "$(caps_mask)" == "2" ]] || fail "Ctrl+Shift 组合误清 CapsLock"
xdotool key Caps_Lock; sleep 0.4 # 收尾关锁并回中文态,还原后续用例现场
wait_caps_mask 0
key_event Shift_L; sleep 0.6   # 无锁态英→中(不触发解锁)
echo "PASS T12b:Shift 单击英→中确认解除 CapsLock(两 Shift 变体),组合/中→英不动锁"

echo "== [T13] 销毁:客户端退出 → 候选窗释放 =="
xdotool key Escape; sleep 0.3
WID="$(xdotool search --name '^lyyime-e2e-client$' | head -1 || true)"
[[ -n "$WID" ]] && xdotool windowclose "$WID" 2>/dev/null || true
sleep 1.5
wait_no_candwin
echo "PASS T13:客户端销毁后自有候选窗不再残留"

kill "$INNER_PID" 2>/dev/null || true; wait "$INNER_PID" 2>/dev/null || true; INNER_PID=""
kill "$SESSION_PID" 2>/dev/null || true; wait "$SESSION_PID" 2>/dev/null || true; SESSION_PID=""
sleep 0.5

echo "== [T14] GTK 不可用回退:独立 HOME/组件/缓存,无 DISPLAY 引擎走原生 lookup =="
HOME2="$WORK/home-fb"
mkdir -p "$HOME2/.local/share/ibus/component" "$HOME2/.config" "$HOME2/.cache" "$HOME2/.local/share/lyyime/ibus/engine" "$HOME2/.local/share/lyyime/ibus/icons"
install -m 755 "$ENGINE_BIN" "$HOME2/.local/share/lyyime/ibus/engine/"
install -m 644 "$ROOT"/ibus-engine/icons/*.svg "$HOME2/.local/share/lyyime/ibus/icons/" 2>/dev/null || true
printf '#!/bin/sh\nunset DISPLAY\nexec "$(dirname "$0")/ibus-engine-lyyime" "$@"\n' \
    > "$HOME2/.local/share/lyyime/ibus/engine/fallback-launcher"
chmod +x "$HOME2/.local/share/lyyime/ibus/engine/fallback-launcher"
write_component "$HOME2/.local/share/lyyime/ibus/engine/fallback-launcher" "$HOME2/.local/share/ibus/component"
dbus-run-session -- bash -c '
    set -uo pipefail
    export HOME="'"$HOME2"'"
    export XDG_CONFIG_HOME="$HOME/.config"
    export XDG_DATA_HOME="$HOME/.local/share"
    export XDG_CACHE_HOME="$HOME/.cache"
    export IBUS_COMPONENT_PATH="$HOME/.local/share/ibus/component:/usr/share/ibus/component"
    unset IBUS_ADDRESS
    ibus-daemon -rx >>"'"$WORK"'/session-fb.log" 2>&1 &
    IBUS_PID=$!
    CLIENT_PID=""
    trap "kill $IBUS_PID \$CLIENT_PID 2>/dev/null || true" EXIT TERM INT
    ready=0
    for _ in $(seq 1 100); do
        f=$(ls "$XDG_CONFIG_HOME"/ibus/bus/*-unix-* 2>/dev/null | head -1)
        [[ -n "$f" ]] && grep -q "^IBUS_ADDRESS=" "$f" && { ready=1; break; }
        sleep 0.2
    done
    [[ "$ready" = 1 ]] || { echo "RUN-FAIL: 回退 ibus 地址未就绪"; exit 1; }
    export IBUS_ADDRESS="$(sed -n 's/^IBUS_ADDRESS=//p' "$f" | head -1)"
    listed=0
    for _ in $(seq 1 50); do
        ibus list-engine 2>/dev/null | grep -q "lyyime - " && { listed=1; break; }
        sleep 0.2
    done
    [[ "$listed" = 1 ]] || { echo "RUN-FAIL: 回退 list-engine 无 lyyime"; exit 1; }
    for _ in $(seq 1 10); do
        ibus engine lyyime 2>/dev/null || true
        [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] && break
        sleep 1
    done
    [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] || { echo "RUN-FAIL: 回退引擎未激活"; exit 1; }
    "$CLIENT_BIN" "$WORK/buffer-fb.txt" 40 >>"$WORK/client.log" 2>&1 &
    CLIENT_PID=$!
    sleep 1.2
    WID="$(xdotool search --name "^lyyime-e2e-client$" | head -1 || true)"
    [[ -n "$WID" ]] && xdotool windowfocus "$WID"
    xdotool type --delay 90 "nihao"; sleep 0.8
    xdotool key space; sleep 0.6
' >>"$WORK/session-fb.log" 2>&1 || { cat "$WORK/session-fb.log"; fail "回退会话执行失败"; }
sleep 0.5
grep -q "你好" "$WORK/buffer-fb.txt" 2>/dev/null || fail "GTK 不可用回退后 nihao+Space 未上屏 你好"
FB_LOG="$HOME2/.local/share/lyyime/logs/ibus.log"
grep -q "自有 GTK 候选窗初始化失败" "$FB_LOG" 2>/dev/null \
    || fail "回退日志缺 GTK 初始化失败记录"
echo "PASS T14:无 DISPLAY 引擎回退原生 lookup,nihao+Space→你好(独立 HOME)"

echo "CANDWIN-E2E-ALL-PASS: T1-T14 通过"
