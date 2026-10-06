#!/usr/bin/env bash
# Mode C 悬浮窗(lyyime-float)E2E: Xvfb + openbox(提供 EWMH 激活)+ xterm(目标)。
# 覆盖: 中文上屏基线 / 空缓冲中文标点直上 / 有缓冲标点(先首选后标点, 合并一次发送) /
#       空缓冲退格直通删目标字符 / 引号开合交替。
# 用法: bash tests/e2e/float_e2e.sh [--keep]
# 前置: cargo build -p lyyime-float --release; xdotool/xterm/openbox 已安装
set -euo pipefail

if [[ -z "${LYYIME_FLOAT_BUS:-}" ]]; then
    if command -v dbus-run-session >/dev/null 2>&1; then
        export LYYIME_FLOAT_BUS=private
        exec dbus-run-session -- bash "$0" "$@"
    fi
    export LYYIME_FLOAT_BUS=none
    unset DBUS_SESSION_BUS_ADDRESS DBUS_SESSION_BUS_PID DBUS_SESSION_BUS_WINDOWID
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FLOAT_BIN="${FLOAT_BIN:-/data/cargo-target/local/lyyIme/release/lyyime-float}"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1
fail() { KEEP=1; echo "FLOAT-E2E-FAIL: $*"; exit 1; }

for t in Xvfb openbox xterm xdotool xwininfo; do
    command -v "$t" >/dev/null 2>&1 || fail "缺依赖: $t"
done
[[ -x "$FLOAT_BIN" ]] || fail "悬浮窗二进制不存在: $FLOAT_BIN(先 cargo build -p lyyime-float --release)"

WORK="$(mktemp -d /tmp/lyyime-float-e2e.XXXXXX)"
export HOME="$WORK/home"
export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share" XDG_CACHE_HOME="$HOME/.cache"
mkdir -p "$HOME/.config/lyyime" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"
unset NO_AT_BRIDGE || true
export PATH="$ROOT/xim/build/bin:$PATH"
export LYYIME_RES_DIR="$ROOT/xim/res"
export LYYIME_ENGINE_STATE_FILE="$WORK/engine-state.json"
printf 'e2e_sentinel = "float-e2e-keep"\n' > "$XDG_CONFIG_HOME/lyyime/config.toml"
CFG_SHA0="$(sha256sum "$XDG_CONFIG_HOME/lyyime/config.toml" | cut -d' ' -f1)"
OUT="$WORK/out.txt"

command -v lyyime-xim >/dev/null 2>&1 || fail "lyyime-xim 不在 PATH(需 xim/build/bin)"
command -v python3 >/dev/null 2>&1 || fail "缺依赖: python3"
python3 -c 'import gi; gi.require_version("Atspi","2.0"); from gi.repository import Atspi' 2>/dev/null \
    || fail "GI/Atspi 不可用,无法做无障碍断言"
python3 -c 'import gi; gi.require_version("Gdk","3.0"); from gi.repository import Gdk' 2>/dev/null \
    || fail "GI/Gdk 不可用,无法截图"

cleanup() {
    for p in "${FLOAT_PID:-}" "${XTERM_PID:-}" "${SETTINGS_PID:-}" "${OB_PID:-}" "${XVFB_PID:-}"; do
        [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
    done
    if [[ $KEEP = 1 ]]; then
        echo "[e2e] 现场保留: $WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# 自定义短语做固定候选: 断言不依赖码表内容(精确码置顶)
printf '{"aa": ["浮窗短语"]}\n' > "$HOME/.config/lyyime/phrase.json"

DISP_FD_FILE="$WORK/xvfb.display"
Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$DISP_FD_FILE" & XVFB_PID=$!
DISP=""
for _ in $(seq 60); do
    [[ -s "$DISP_FD_FILE" ]] && DISP=":$(head -1 "$DISP_FD_FILE")" && break
    sleep 0.1
done
[[ -n "$DISP" ]] || fail "Xvfb -displayfd 未给出显示号"
kill -0 "$XVFB_PID" 2>/dev/null || fail "Xvfb 进程已退出"
export DISPLAY="$DISP"
for _ in $(seq 50); do
    [[ -S "/tmp/.X11-unix/X${DISP#:}" ]] && break
    sleep 0.1
done
[[ -S "/tmp/.X11-unix/X${DISP#:}" ]] || fail "Xvfb 显示 socket 未就绪($DISP)"
if [[ "${LYYIME_FLOAT_BUS:-none}" == private ]]; then
    command -v dbus-update-activation-environment >/dev/null 2>&1 \
        || fail "缺依赖: dbus-update-activation-environment"
    dbus-update-activation-environment DISPLAY \
        || fail "私有总线 DISPLAY 激活环境更新失败"
fi
# openbox 提供 _NET_ACTIVE_WINDOW 与窗口激活(悬浮窗目标追踪/回焦依赖 EWMH)
DISPLAY="$DISP" openbox & OB_PID=$!
sleep 0.8

# 目标窗口: xterm 跑 cat, 敲 Return 后逐行落盘
DISPLAY="$DISP" xterm -e "cat > '$OUT'" & XTERM_PID=$!
XTERM_WID=""
for _ in $(seq 50); do
    XTERM_WID="$(DISPLAY="$DISP" xdotool search --onlyvisible --class '[Xx][Tt]erm' 2>/dev/null | head -1 || true)"
    [[ -n "$XTERM_WID" ]] && break
    sleep 0.2
done
[[ -n "$XTERM_WID" ]] || fail "xterm 目标窗口未出现"

DISPLAY="$DISP" "$FLOAT_BIN" >>"$WORK/float.log" 2>&1 & FLOAT_PID=$!
FLOAT_WID=""
for _ in $(seq 50); do
    FLOAT_WID="$(DISPLAY="$DISP" xdotool search --name '^lyyIme 悬浮输入法$' 2>/dev/null | head -1 || true)"
    [[ -n "$FLOAT_WID" ]] && break
    sleep 0.2
done
[[ -n "$FLOAT_WID" ]] || fail "悬浮窗未出现"

act() { DISPLAY="$DISP" xdotool windowactivate "$1" >/dev/null 2>&1 || true; }
wait_float_focus() {
    local deadline=$((SECONDS + 5)) s1 s2 fw aw
    while (( SECONDS < deadline )); do
        s1="$(DISPLAY="$DISP" xdotool getwindowfocus 2>/dev/null),$(DISPLAY="$DISP" xdotool getactivewindow 2>/dev/null)"
        sleep 0.1
        s2="$(DISPLAY="$DISP" xdotool getwindowfocus 2>/dev/null),$(DISPLAY="$DISP" xdotool getactivewindow 2>/dev/null)"
        [[ "$s1" == "$s2" ]] || continue
        fw="${s1%%,*}"; aw="${s1##*,}"
        [[ -n "$fw" && -n "$aw" ]] || continue
        (( fw == FLOAT_WID && aw == FLOAT_WID )) && return 0
    done
    fail "悬浮窗未在 5s 内获得稳定 X 焦点与 EWMH 激活态"
}
type_in_float() { wait_float_focus; DISPLAY="$DISP" xdotool type --delay 70 "$1"; sleep 0.6; }
key_in_float() { wait_float_focus; DISPLAY="$DISP" xdotool key "$1"; sleep 0.8; }

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

atspi_find() { timeout 2 python3 "$ATSPI_PY" "$1" "$2" "${3:-}" "${4:-$FLOAT_PID}" 2>/dev/null; }
atspi_click() {
    local xy cx cy cw ch
    xy="$(atspi_find "$1" "$2" "${3:-}" "${4:-}")" || return 1
    read -r cx cy cw ch <<<"$xy"
    DISPLAY="$DISP" xdotool mousemove "$cx" "$cy" click 1
}
atspi_rclick() {
    local xy cx cy cw ch
    xy="$(atspi_find "$1" "$2" "${3:-}" "${4:-}")" || return 1
    read -r cx cy cw ch <<<"$xy"
    DISPLAY="$DISP" xdotool mousemove "$cx" "$cy" click 3
}
wait_atspi() {
    local deadline=$((SECONDS + 10))
    while (( SECONDS < deadline )); do
        atspi_find "$1" "$2" "${3:-}" "${4:-}" >/dev/null && return 0
        sleep 0.1
    done
    return 1
}
wait_no_atspi() {
    local deadline=$((SECONDS + 10))
    while (( SECONDS < deadline )); do
        atspi_find "$1" "$2" "${3:-}" "${4:-}" >/dev/null || return 0
        sleep 0.1
    done
    return 1
}
shot() { DISPLAY="$DISP" timeout 10 python3 "$SHOT_PY" "$WORK/$1.png" 2>/dev/null || fail "截图 $1 采集失败"; }
mark() {
    echo "[trace:$1] focus=$(DISPLAY="$DISP" xdotool getwindowfocus -f 2>/dev/null || echo '?')"
    shot "trace-$1"
}

# 目标登记: 先激活 xterm, 等 poll_target(250ms) 记住它, 再回悬浮窗
act "$XTERM_WID"; sleep 1.2
act "$FLOAT_WID"; sleep 0.6

# 1) 基线: 短语码 aa + 空格 → 「浮窗短语」
mark s1-pre
type_in_float "aa"
mark s1-typed
key_in_float space
mark s1-committed
# 2) 空缓冲标点: , → 「，」
type_in_float ","
mark s2-punct
# 3) 有缓冲标点: aa + , → 「浮窗短语，」(首选与标点合并一次发送)
type_in_float "aa"
mark s3-typed
type_in_float ","
mark s3-punct
# 4) 空缓冲退格直通: 删掉上一步尾部「，」(目标 tty 行内删字)
key_in_float BackSpace
mark s4-backspace
# 5) 引号开合交替: 两个单引号 → 「''」
type_in_float "'"
mark s5-quote1
type_in_float "'"
mark s5-quote2

key_in_float Escape
type_in_float "aa"
wait_atspi "浮窗短语" "push button" "contains" \
    || { shot "float-cand-missing"; fail "aa 未出现 浮窗短语 候选按钮"; }
atspi_rclick "浮窗短语" "push button" "contains" || fail "定位 浮窗短语 候选按钮失败"
wait_atspi "删除词组" "menu item" \
    || { shot "float-word-menu-missing"; fail "候选右键未弹出词菜单"; }
atspi_find "固定首位" "menu item" >/dev/null \
    || atspi_find "取消固定首位" "menu item" >/dev/null \
    || fail "词菜单缺 固定首位/取消固定首位"
atspi_find "反查英文" "menu item" >/dev/null || fail "词菜单缺 反查英文"
atspi_find "设置…" "menu item" >/dev/null && fail "词菜单混入 设置…"
atspi_find "重载词库" "menu item" >/dev/null && fail "词菜单混入 重载词库"
shot "float-word-menu"
DISPLAY="$DISP" xdotool key Escape; sleep 0.4
wait_no_atspi "删除词组" "menu item" || fail "词菜单 Escape 后未关闭"

CAND_R=0
for d in 1 2 3 4 5 6 7 8 9; do
    xy="$(atspi_find "$d" "push button" "contains" || true)"
    [[ -n "$xy" ]] || continue
    read -r _x _y _w _h <<<"$xy"
    r=$((_x + _w / 2))
    (( r > CAND_R )) && CAND_R=$r
done
(( CAND_R > 0 )) || fail "未能量到任何候选按钮右缘"
GEAR_XY="$(atspi_find "打开设置" "push button")" || GEAR_XY="$(atspi_find "打开设置" "any")" \
    || fail "定位 打开设置 齿轮失败"
read -r GX GY GW GH <<<"$GEAR_XY"
GEAR_L=$((GX - GW / 2))
DISPLAY="$DISP" xdotool mousemove "$GX" "$GY" click 3; sleep 0.4
wait_atspi "设置…" "menu item" \
    || { shot "float-gear-menu-missing"; fail "齿轮右键未弹通用菜单"; }
atspi_find "重载词库" "menu item" >/dev/null || fail "齿轮菜单缺 重载词库"
for w in "固定首位" "取消固定首位" "删除词组" "反查英文"; do
    atspi_find "$w" "menu item" >/dev/null && fail "齿轮菜单混入词操作 [$w]"
done
shot "float-gear-menu"
DISPLAY="$DISP" xdotool key Escape; sleep 0.4
wait_no_atspi "设置…" "menu item" || fail "齿轮菜单 Escape 后未关闭"

GAP=$((GEAR_L - CAND_R))
if (( GAP > 8 )); then
    MID_X=$(( (CAND_R + GEAR_L) / 2 ))
    DISPLAY="$DISP" xdotool mousemove "$MID_X" "$GY" click 3; sleep 0.4
    wait_atspi "设置…" "menu item" \
        || { shot "float-gap-menu-missing"; fail "候选-齿轮空白右键未弹通用菜单"; }
    atspi_find "删除词组" "menu item" >/dev/null && fail "空白区菜单混入词操作"
    atspi_find "固定首位" "menu item" >/dev/null && fail "空白区菜单混入词操作"
    shot "float-gap-menu"
    DISPLAY="$DISP" xdotool key Escape; sleep 0.4
    wait_no_atspi "设置…" "menu item" || fail "空白区菜单 Escape 后未关闭"
fi

key_in_float Escape
type_in_float "zzzzzzzz"
sleep 0.3
for d in 1 2 3 4 5 6 7 8 9; do
    atspi_find "$d" "push button" "contains" >/dev/null \
        && fail "零候选态出现编号候选按钮 [$d]"
done
GEAR_XY="$(atspi_find "打开设置" "push button")" || fail "零候选态齿轮不可见"
read -r GX GY GW GH <<<"$GEAR_XY"
BX=$((GX - GW / 2 - 6))
(( BX > 0 )) || fail "齿轮左侧无空白可点"
DISPLAY="$DISP" xdotool mousemove "$BX" "$GY" click 3; sleep 0.4
wait_atspi "设置…" "menu item" \
    || { shot "float-zero-menu-missing"; fail "零候选空白右键未弹通用菜单"; }
atspi_find "删除词组" "menu item" >/dev/null && fail "零候选空白菜单混入词操作"
DISPLAY="$DISP" xdotool key Escape; sleep 0.4
wait_no_atspi "设置…" "menu item" || fail "零候选菜单 Escape 后未关闭"
key_in_float Escape

GEAR_XY="$(atspi_find "打开设置" "push button")" || fail "设置前齿轮定位失败"
read -r GX GY GW GH <<<"$GEAR_XY"
OUT0="$(cat "$OUT" 2>/dev/null || true)"
DISPLAY="$DISP" xdotool mousemove "$GX" "$GY" click 1; sleep 0.8
SWID=""
for _ in $(seq 1 40); do
    SWID="$(DISPLAY="$DISP" xdotool search --onlyvisible --name 'lyyIme 输入法设置' 2>/dev/null | head -1 || true)"
    [[ -n "$SWID" ]] && break
    sleep 0.25
done
[[ -n "$SWID" ]] || { shot "float-settings-missing"; fail "齿轮左键未打开真实设置窗"; }
shot "float-settings"
atspi_find "目标: lyyIme 输入法设置" "label" "contains" >/dev/null \
    && fail "设置窗被错误登记为输入目标"
SPID="$(DISPLAY="$DISP" xdotool getwindowpid "$SWID" 2>/dev/null || echo 0)"
[[ "$SPID" != "0" ]] && SETTINGS_PID="$SPID"
if [[ "$SPID" != "0" ]] && atspi_click "取消" "push button" "" "$SPID"; then
    sleep 0.6
else
    DISPLAY="$DISP" xdotool windowfocus "$SWID" 2>/dev/null || true
    DISPLAY="$DISP" xdotool key Escape; sleep 0.6
fi
for _ in $(seq 1 25); do
    DISPLAY="$DISP" xdotool search --onlyvisible --name 'lyyIme 输入法设置' >/dev/null 2>&1 \
        || break
    sleep 0.2
done
DISPLAY="$DISP" xdotool search --onlyvisible --name 'lyyIme 输入法设置' >/dev/null 2>&1 \
    && fail "设置窗取消后仍可见(隐藏即关闭语义)"
shot "float-settings-hidden"
[[ "$(sha256sum "$XDG_CONFIG_HOME/lyyime/config.toml" | cut -d' ' -f1)" == "$CFG_SHA0" ]] \
    || fail "设置取消后 config.toml 被改动"
act "$FLOAT_WID"; sleep 0.6
GEAR_XY="$(atspi_find "打开设置" "push button")" || fail "二次设置前齿轮定位失败"
read -r GX GY GW GH <<<"$GEAR_XY"
DISPLAY="$DISP" xdotool mousemove "$GX" "$GY" click 1; sleep 0.8
SWID=""
for _ in $(seq 1 40); do
    SWID="$(DISPLAY="$DISP" xdotool search --onlyvisible --name 'lyyIme 输入法设置' 2>/dev/null | head -1 || true)"
    [[ -n "$SWID" ]] && break
    sleep 0.25
done
[[ -n "$SWID" ]] || { shot "float-settings2-missing"; fail "设置窗二次打开未复现"; }
shot "float-settings-reopen"
atspi_find "目标: lyyIme 输入法设置" "label" "contains" >/dev/null \
    && fail "二次设置窗被错误登记为输入目标"
SPID="$(DISPLAY="$DISP" xdotool getwindowpid "$SWID" 2>/dev/null || echo 0)"
[[ "$SPID" != "0" ]] && SETTINGS_PID="$SPID"
if [[ "$SPID" != "0" ]] && atspi_click "取消" "push button" "" "$SPID"; then
    sleep 0.6
else
    DISPLAY="$DISP" xdotool windowfocus "$SWID" 2>/dev/null || true
    DISPLAY="$DISP" xdotool key Escape; sleep 0.6
fi
for _ in $(seq 1 25); do
    DISPLAY="$DISP" xdotool search --onlyvisible --name 'lyyIme 输入法设置' >/dev/null 2>&1 \
        || break
    sleep 0.2
done
DISPLAY="$DISP" xdotool search --onlyvisible --name 'lyyIme 输入法设置' >/dev/null 2>&1 \
    && fail "设置窗二次取消后仍可见"
shot "float-settings-hidden2"
[[ "$(sha256sum "$XDG_CONFIG_HOME/lyyime/config.toml" | cut -d' ' -f1)" == "$CFG_SHA0" ]] \
    || fail "二次设置取消后 config.toml 被改动"
[[ "$(cat "$OUT" 2>/dev/null || true)" == "$OUT0" ]] || fail "设置过程产生意外上屏"
act "$FLOAT_WID"; sleep 0.6
type_in_float "aa"
key_in_float space

# 收尾: 回车让 xterm 的 cat 落盘
act "$XTERM_WID"; sleep 0.5
DISPLAY="$DISP" xdotool key Return; sleep 0.8
kill "$XTERM_PID" 2>/dev/null || true

[[ -f "$OUT" ]] || fail "目标输出文件不存在"
CONTENT="$(cat "$OUT")"
echo "[e2e] 目标窗口收到: $CONTENT"

# tty 行缓冲推演: 浮窗短语 | ，| 浮窗短语， | 退格删掉尾部，| 弯引号对 → 一行
EXPECTED="浮窗短语，浮窗短语"$'\u2018\u2019'"浮窗短语"
[[ "$CONTENT" == "$EXPECTED" ]] || fail "上屏内容不符: 期望 [$EXPECTED]"
if grep -q 'aa' "$OUT"; then fail "字母 aa 裸上屏泄漏"; fi
if grep -qF ',' "$OUT"; then fail "出现半角逗号(中文标点未生效)"; fi

echo "FLOAT-E2E-OK"
