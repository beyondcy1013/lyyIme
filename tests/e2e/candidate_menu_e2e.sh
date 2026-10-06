#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${LYYIME_MENU_BUS:-}" ]]; then
    if command -v dbus-run-session >/dev/null 2>&1; then
        export LYYIME_MENU_BUS=private
        exec dbus-run-session -- bash "$0" "$@"
    fi
    export LYYIME_MENU_BUS=none
    unset DBUS_SESSION_BUS_ADDRESS DBUS_SESSION_BUS_PID DBUS_SESSION_BUS_WINDOWID
fi

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../xim" && pwd)"
FIXTURES="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../crates/lyyime-core/tests/fixtures" && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

WORK="$(mktemp -d /tmp/lyyime-menu-e2e.XXXXXX)"
ART="$WORK/artifacts"
BUFFER="$WORK/buffer.txt"
CLIENT_LOG="$WORK/client.log"

cleanup() {
    [[ -n "${CLIENT_PID:-}" ]] && kill "$CLIENT_PID" 2>/dev/null || true
    [[ -n "${XIM_PID:-}" ]] && kill "$XIM_PID" 2>/dev/null || true
    [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
    [[ -n "${DBUS_PID:-}" ]] && kill "$DBUS_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ $KEEP -eq 1 ]]; then
        echo "[menu-e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

export HOME="$WORK/home"
export XDG_CONFIG_HOME="$WORK/xdg/config"
export XDG_DATA_HOME="$WORK/xdg/data"
export XDG_CACHE_HOME="$WORK/xdg/cache"
export LYYIME_ENGINE_STATE_FILE="$WORK/engine-state.json"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$ART"
unset NO_AT_BRIDGE

DBUS_PID=""
if [[ "${LYYIME_MENU_BUS}" == none ]] && command -v dbus-launch >/dev/null 2>&1; then
    eval "$(dbus-launch --sh-syntax)"
    DBUS_PID="${DBUS_SESSION_BUS_PID:-}"
    LYYIME_MENU_BUS=private
fi
PRIVATE_BUS=0
[[ "${LYYIME_MENU_BUS}" == private ]] && PRIVATE_BUS=1

fail() { KEEP=1; echo "MENU-E2E FAIL: $*"; echo "(失败现场保留:$WORK)"; exit 1; }
unverified() { echo "MENU-E2E UNVERIFIED: $*"; }


CORE_SO="${CARGO_TARGET_DIR:-/data/cargo-target/local/codes_apps_lyyIme-93ecbcd94ded0a75}/release/liblyyime_core.so"
[[ -f "$CORE_SO" ]] || fail "真库不存在:$CORE_SO(先跑 cargo build --release)"
nm -D "$CORE_SO" 2>/dev/null | grep -q "lyyime_cand_pinned" \
    || fail "真库缺 lyyime_cand_pinned/候选右键 ABI,F1 不允许跳过"

DICT="$WORK/dict"
mkdir -p "$DICT"
cp "$FIXTURES"/* "$DICT"/

mkdir -p "$XDG_CONFIG_HOME/lyyime"
cat > "$XDG_CONFIG_HOME/lyyime/config.toml" <<'CFG'
commit_after_four = false
commit_first_at_four = false
commit_unique_four = false
phrase_hint = false
learning = true
unknown_sentinel_key = "do-not-touch"

[ai]
api_key = "sk-sentinel-AAAA"
model = "sentinel-model"
# ai section tail comment
CFG
CFG_PATH="$XDG_CONFIG_HOME/lyyime/config.toml"

export XMODIFIERS=@im=lyyime
export GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8

XIM_LOG="$XDG_DATA_HOME/lyyime/logs/xim.log"
XIM_BIN="$XIM_DIR/build/bin/lyyime-xim"
CLIENT_BIN="$XIM_DIR/build/tests/e2e_client"
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
    "check box": Atspi.Role.CHECK_BOX,
    "toggle button": Atspi.Role.TOGGLE_BUTTON,
    "label": Atspi.Role.LABEL,
    "push button": Atspi.Role.PUSH_BUTTON,
}


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


def walk(node, label, roles, depth):
    if depth > 30:
        return None
    try:
        name = node.get_name() or ""
    except Exception:
        name = ""
    try:
        st = node.get_state_set()
        showing = st.contains(Atspi.StateType.SHOWING) and \
            st.contains(Atspi.StateType.VISIBLE)
    except Exception:
        showing = False
    matched = showing and (name == label or (CONTAINS and label in name))
    if matched and match_role(node, roles):
        try:
            comp = node.get_component_iface()
            ext = comp.get_extents(Atspi.CoordType.SCREEN)
            checked = "1" if node.get_state_set().contains(
                Atspi.StateType.CHECKED) else "0"
            return "%d %d %d %d %s" % (
                ext.x + ext.width // 2, ext.y + ext.height // 2,
                ext.width, ext.height, checked)
        except Exception:
            return None
    try:
        count = node.get_child_count()
        for i in range(count):
            r = walk(node.get_child_at_index(i), label, roles, depth + 1)
            if r:
                return r
    except Exception:
        pass
    return None


label = sys.argv[1]
roles = set(sys.argv[2].split(","))
CONTAINS = len(sys.argv) > 3 and sys.argv[3] == "contains"
desktop = Atspi.get_desktop(0)
for i in range(desktop.get_child_count()):
    r = walk(desktop.get_child_at_index(i), label, roles, 0)
    if r:
        print(r)
        sys.exit(0)
sys.exit(1)
PY

buf_now() { cat "$BUFFER" 2>/dev/null || true; }
buf_eq() {
    local want="$1" got
    got="$(buf_now)"
    [[ "$got" == "$want" ]] || fail "缓冲应为[$want]实为[$got]"
}
wait_buf_eq() {
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ "$(buf_now)" == "$want" ]] && return 0
        sleep 0.05
    done
    buf_eq "$want"
}

wait_log() {
    local pat="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        grep -q "$pat" "$XIM_LOG" 2>/dev/null && return 0
        sleep 0.05
    done
    echo "---- xim.log ----"; cat "$XIM_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 超时"
}

log_count() { grep -c "$1" "$XIM_LOG" 2>/dev/null || true; }
wait_log_count() {
    local pat="$1" n="$2" timeout="${3:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        local c
        c="$(log_count "$pat")"
        [[ "$c" =~ ^[0-9]+$ && "$c" -ge "$n" ]] && return 0
        sleep 0.05
    done
    echo "---- xim.log ----"; cat "$XIM_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 计数 >= $n 超时(当前 $(log_count "$pat"))"
}

wait_cfg() {
    local pat="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        grep -q "$pat" "$CFG_PATH" 2>/dev/null && return 0
        sleep 0.05
    done
    echo "---- config.toml ----"; cat "$CFG_PATH" 2>/dev/null || true
    fail "等待配置出现 [$pat] 超时"
}

shot() {
    local name="$1"
    local f="$ART/$name"
    if python3 - "$f.png" 2>"$f.gdk.err" <<'PY'
import sys
import gi
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk
win = Gdk.get_default_root_window()
pb = Gdk.pixbuf_get_from_window(win, 0, 0, win.get_width(), win.get_height())
pb.savev(sys.argv[1], "png", [], [])
PY
    then
        echo "[shot] $f.png"; return 0
    fi
    fail "截图不可用($name;Gdk pixbuf 失败,见 $f.gdk.err)"
}

candwin_geom() {
    local ids w
    ids="$(xwininfo -root -children 2>/dev/null | grep '"lyyime-xim"' \
        | awk '{print $1}' || true)"
    while IFS= read -r w; do
        [[ -n "$w" ]] || continue
        local info
        info="$(xwininfo -id "$w" 2>/dev/null || true)"
        grep -q "Map State: IsViewable" <<<"$info" || continue
        grep -q "Override Redirect State: yes" <<<"$info" || continue
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
    for ((i = 0; i < 120; i++)); do
        candwin_geom && return 0
        sleep 0.1
    done
    shot "candwin-missing"
    fail "候选窗未映射为可见"
}

menu_visible() {
    local known="$1" ids w info
    ids="$(xwininfo -root -children 2>/dev/null | awk '/^ *0x[0-9a-f]+ /{print $1}')"
    while IFS= read -r w; do
        [[ -n "$w" && "$w" != "$known" ]] || continue
        info="$(xwininfo -id "$w" 2>/dev/null || true)"
        grep -q "Map State: IsViewable" <<<"$info" || continue
        grep -q "Override Redirect State: yes" <<<"$info" || continue
        MX="$(awk '/Absolute upper-left X/{print $NF}' <<<"$info")"
        MY="$(awk '/Absolute upper-left Y/{print $NF}' <<<"$info")"
        MW="$(awk '/Width:/{print $NF}' <<<"$info")"
        MH="$(awk '/Height:/{print $NF}' <<<"$info")"
        [[ -n "$MX" && -n "$MW" && "$MW" -gt 40 ]] && return 0
    done <<<"$ids"
    return 1
}

open_general_menu() {
    candwin_geom || fail "候选窗几何不可得(需先处于组合态)"
    xdotool mousemove "$((X + WIDTH / 2))" "$((Y + 6))" click 3
    local i
    for ((i = 0; i < 60; i++)); do
        menu_visible "$CWID" && return 0
        sleep 0.1
    done
    shot "menu-missing"
    fail "右键表头后通用菜单未出现"
}

open_row_menu() {
    candwin_geom || fail "候选窗几何不可得(需先处于组合态)"
    [[ $ATSPI -eq 1 ]] || fail "词行命中需要 AT-SPI(需精确候选词坐标,禁止宽度猜测)"
    local wxy wx wy _ww _wh _ws
    wxy="$(atspi_find "你好" "label")" \
        || fail "AT-SPI 未定位可见候选词 你好(词行右键命中点不可得)"
    read -r wx wy _ww _wh _ws <<<"$wxy"
    xdotool mousemove "$wx" "$wy" click 3
    local i
    for ((i = 0; i < 60; i++)); do
        menu_visible "$CWID" && return 0
        sleep 0.1
    done
    shot "row-menu-missing"
    fail "右键词行后菜单未出现(候选右键 ABI 缺失?"
}

atspi_find() {
    [[ $ATSPI -eq 1 ]] || return 1
    timeout 10 python3 "$ATSPI_PY" "$1" "$2" "${3:-}" 2>/dev/null
}

ACT_N=0
menu_activate() {
    local label="$1" kidx="$2" out fw1 fw2 left atspi_left
    out="$(atspi_find "$label" "menu item,check menu item,radio menu item")" || out=""
    if [[ -n "$out" ]]; then
        read -r cx cy _w _h _s <<<"$out"
        xdotool mousemove "$cx" "$cy" click 1
    else
        xdotool mousemove $((MX + MW / 2)) $((MY + 8))
        sleep 0.2
        local i
        for ((i = 0; i < kidx; i++)); do
            xdotool key Down
            sleep 0.06
        done
        xdotool key Return
    fi
    sleep 0.6
    fw1="$(xdotool getwindowfocus 2>/dev/null || true)"
    left=""
    if menu_visible "$CWID"; then
        left="mapped:${MW}x${MH}@${MX},${MY}"
    fi
    atspi_left=""
    if [[ $ATSPI -eq 1 && -n $left ]]; then
        atspi_find "$label" "menu item,check menu item,radio menu item" >/dev/null             && atspi_left="atspi-still-present" || atspi_left="atspi-gone"
    fi
    ACT_N=$((ACT_N + 1))
    shot "after-toggle-$ACT_N"
    fw2="$(xdotool getwindowfocus 2>/dev/null || true)"
    echo "[menu-e2e] activate[$label] 后:focus $fw1→$fw2 菜单残留=${left:-无} $atspi_left"
    if [[ -n $left ]]; then
        shot "after-toggle-$ACT_N-stuck"
        fail "菜单激活后仍映射($left $atspi_left):生命周期缺陷未修复"
    fi
}

menu_state() {
    local out
    out="$(atspi_find "$1" "check menu item,radio menu item")" || out=""
    [[ -n "$out" ]] || return 2
    [[ "$(awk '{print $NF}' <<<"$out")" == 1 ]]
}

menu_assert_radio() {
    local label="$1" expect="$2"
    local rc=0
    menu_state "$label" || rc=$?
    if [[ $rc -eq 0 ]]; then
        [[ "$expect" == checked ]] || fail "菜单项[$label]已勾选但应为未勾选"
    elif [[ $rc -eq 1 ]]; then
        [[ "$expect" == unchecked ]] || fail "菜单项[$label]未勾选但应为已勾选"
    else
        unverified "菜单项[$label]勾选态无法读取(无 AT-SPI),由后续行为断言覆盖"
    fi
}

focus_client() {
    xdotool windowfocus --sync "$WID"
    xdotool key End
    sleep 0.2
}

reset_comp() {
    focus_client
    xdotool key Escape
    sleep 0.3
}

fresh_comp() {
    reset_comp
    xdotool type --delay 90 "$1"
    wait_candwin
}

echo "== [1/9] 构建产物与显示 =="
[[ -x "$XIM_BIN" && -x "$CLIENT_BIN" ]] \
    || fail "缺构建产物(先 make -C xim all test)"

DISP_FD_FILE="$WORK/xvfb.display"
Xvfb -displayfd 3 -screen 0 1024x768x24 -nolisten tcp 3>"$DISP_FD_FILE" &
XVFB_PID=$!
DISP=""
for _ in $(seq 1 60); do
    [[ -s "$DISP_FD_FILE" ]] && DISP="$(head -1 "$DISP_FD_FILE")" && break
    sleep 0.1
done
[[ -n "$DISP" ]] || fail "Xvfb -displayfd 未给出显示号"
export DISPLAY=":$DISP"
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X${DISP}" ]] && break; sleep 0.1; done
echo "[menu-e2e] Xvfb 就绪 DISPLAY=$DISPLAY(pid=$XVFB_PID)"

ATSPI=0
if [[ $PRIVATE_BUS -eq 1 ]] && timeout 15 python3 -c '
import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi
d = Atspi.get_desktop(0)
d.get_child_count()
' 2>/dev/null; then
    ATSPI=1
else
    unverified "Atspi GI/a11y 不可用(私有总线=$PRIVATE_BUS):菜单项标签与勾选态断言降级为行为断言,人工复核截图"
fi

echo "== [2/9] 启动 lyyime-xim(真库) =="
LYYIME_CORE_LIB="$CORE_SO" LYYIME_RES_DIR="$XIM_DIR/res" \
    LYYIME_DATA_DIR="$DICT" \
    "$XIM_BIN" >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"

echo "== [3/9] 客户端接入 =="
"$CLIENT_BIN" "$BUFFER" 300 >"$CLIENT_LOG" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log "XIM client 已连接"
sleep 0.8
WID="$(xdotool search --onlyvisible --name '^lyyime-e2e-client$' | head -1 || true)"
[[ -n "$WID" ]] || fail "找不到客户端窗口"
focus_client
sleep 0.3

echo "== [4/9] A:nihao 组合 + 右键表头 → 通用菜单 =="
fresh_comp "nihao"
buf_eq ""
open_general_menu
if [[ $ATSPI -eq 1 ]]; then
    atspi_find "纯拼音" "radio menu item" >/dev/null \
        || fail "通用菜单缺少「纯拼音」项"
    atspi_find "固定首位" "menu item" >/dev/null \
        && fail "通用菜单混入词操作「固定首位」"
    atspi_find "删除词组" "menu item" >/dev/null \
        && fail "通用菜单混入词操作「删除词组」"
    atspi_find "反查英文" "menu item" >/dev/null \
        && fail "通用菜单混入词操作「反查英文」"
fi
shot "A-general-menu"
echo "PASS A:表头右键 → 通用菜单映射(纯系统项,无词操作)"

echo "== [5/9] B:选纯拼音 → 持久化 + nihao 空格上屏 =="
menu_activate "纯拼音" 5
wait_cfg 'pinyin_only = true'
wait_log "菜单切换 输入方案 → 1"
fw0="$(log_count "forward keysym")"
reset_comp
xdotool type --delay 90 "nihao"
sleep 0.5
fw1="$(log_count "forward keysym")"
echo "[menu-e2e] B:focus=$(xdotool getwindowfocus 2>/dev/null || true) forward $fw0→$fw1"
shot "B-fresh-comp-focus"
if [[ $fw1 -le $fw0 ]]; then
    echo "==== DIAG B:nihao 未进 forward 通路 ===="
    echo "-- getwindowfocus=$(xdotool getwindowfocus 2>/dev/null || true) activewindow=$(xdotool getactivewindow 2>/dev/null || true)"
    echo "-- root children --"
    xwininfo -root -children 2>/dev/null | tail -20 || true
    echo "-- menu_visible --"
    menu_visible "${CWID:-}" && echo "MENU STILL VISIBLE" || echo "no menu mapped"
    echo "-- atspi focus --"
    if [[ $ATSPI -eq 1 ]]; then
        timeout 10 python3 -c '
import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi
d = Atspi.get_desktop(0)
for i in range(d.get_child_count()):
    a = d.get_child_at_index(i)
    print("app", a.get_name(), "focused_win_state_seen")
' 2>/dev/null || true
        atspi_find "" "menu" 2>/dev/null || true
    fi
    echo "-- client.stderr --"
    cat "$WORK/client.stderr" 2>/dev/null || true
    echo "-- xim.stdout tail --"
    tail -40 "$WORK/xim.stdout" 2>/dev/null || true
    echo "-- xim.log tail --"
    tail -60 "$XIM_LOG" 2>/dev/null || true
    shot "B-forward-missing"
    fail "B:nihao 按键未进入 IM forward 通路(焦点/GDK 事件丢失)"
fi
wait_candwin
sleep 0.3
buf_eq ""
xdotool key space
wait_buf_eq "你好" 8
shot "B-pure-commit"
echo "PASS B:纯拼音持久化 + nihao 组合态直到空格 → 你好"

echo "== [6/9] C:纯拼音下 aa 无五笔候选;重开菜单方案勾选态 =="
xdotool type --delay 90 "aa"
sleep 0.6
buf_eq "你好"
shot "C-pure-aa"
fresh_comp "nihao"
open_general_menu
menu_assert_radio "纯拼音" checked
shot "C-menu-reopen"
menu_activate "五笔/拼音混输" 4
wait_cfg 'pinyin_only = false'
wait_log "菜单切换 输入方案 → 0"
echo "PASS C:aa 纯拼音无五笔候选,单选重开仍为纯拼音"

echo "== [7/9] D:混输 aa+空格 = 式 =="
focus_client
sleep 0.2
xdotool type --delay 90 "aa"
wait_candwin
xdotool key space
wait_buf_eq "你好式" 8
echo "PASS D:混输恢复,aa 空格上屏式"

echo "== [8/9] E:菜单取消不上屏 + 学习开关单键翻转 =="
fresh_comp "nihao"
open_general_menu
xdotool key Escape
sleep 0.4
buf_eq "你好式"
reset_comp
buf_eq "你好式"
cp "$CFG_PATH" "$WORK/cfg.before"
fresh_comp "nihao"
open_general_menu
menu_activate "用户词学习" 7
wait_cfg 'learning = false'
diff <(grep -vE '^[[:space:]]*learning\b' "$WORK/cfg.before") \
     <(grep -vE '^[[:space:]]*learning\b' "$CFG_PATH") \
    || { cat "$CFG_PATH"; fail "学习翻转改动了无关键"; }
grep -q 'api_key = "sk-sentinel-AAAA"' "$CFG_PATH" || fail "[ai] 哨兵被改写"
grep -q 'unknown_sentinel_key' "$CFG_PATH" || fail "未知键被清掉"
grep -q '# ai section tail comment' "$CFG_PATH" || fail "注释被清掉"
shot "E-learning-off"
echo "PASS E:Esc 取消无上屏;learning 单键翻转,[ai]/未知键/注释保留"

echo "== [8.5/9] E2:保存失败 → 单选回滚,重开仍为混输 =="
mv "$CFG_PATH" "$CFG_PATH.bak" && mkdir "$CFG_PATH"
fresh_comp "nihao"
open_general_menu
menu_activate "纯拼音" 5
wait_log "菜单切换 输入方案 中止" 8
[[ -d "$CFG_PATH" ]] || fail "保存失败后 config.toml 被覆盖"
rmdir "$CFG_PATH" && mv "$CFG_PATH.bak" "$CFG_PATH"
fresh_comp "nihao"
open_general_menu
if [[ $ATSPI -eq 1 ]]; then
    menu_assert_radio "纯拼音" unchecked
    menu_assert_radio "五笔/拼音混输" checked
else
    aborts="$(log_count "菜单切换 输入方案 中止")"
    mv "$CFG_PATH" "$CFG_PATH.bak" && mkdir "$CFG_PATH"
    menu_activate "纯拼音" 5
    wait_log_count "菜单切换 输入方案 中止" "$((aborts + 1))" 8
    rmdir "$CFG_PATH" && mv "$CFG_PATH.bak" "$CFG_PATH"
fi
shot "E2-rollback"
xdotool key Escape
sleep 0.3
echo "PASS E2:读取/保存失败单选回滚,重开菜单仍为混输"

echo "== [9/9] F:词行右键固定首位持久 + 输入设置页勾选态 =="
fresh_comp "nihao"
open_row_menu
if [[ $ATSPI -eq 1 ]]; then
    atspi_find "固定首位" "menu item" >/dev/null \
        || fail "词行菜单缺少「固定首位」项"
    atspi_find "设置…" "menu item" >/dev/null \
        && fail "词行菜单混入系统动作「设置…」"
    atspi_find "重载词库" "menu item" >/dev/null \
        && fail "词行菜单混入系统动作「重载词库」"
    atspi_find "纯拼音" "radio menu item,check menu item" >/dev/null \
        && fail "词行菜单混入方案单选「纯拼音」"
fi
menu_activate "固定首位" 0
wait_log "候选右键操作 idx=0 op=1" 8
pins="$(log_count "候选右键操作 idx=0 op=1")"
fresh_comp "nihao"
open_row_menu
if [[ $ATSPI -eq 1 ]]; then
    atspi_find "取消固定首位" "menu item" >/dev/null \
        || fail "固定首位未持久:重开词行菜单仍显示「固定首位」"
    menu_activate "取消固定首位" 0
    wait_log_count "候选右键操作 idx=0 op=1" "$((pins + 1))" 8
else
    xdotool key Escape
    sleep 0.3
    unverified "固定首位持久性标签断言无 AT-SPI;op=1 下发已由日志证实"
fi
reset_comp
fresh_comp "nihao"
open_general_menu
menu_activate "输入设置…" 1
sleep 1.2
SW="$(xdotool search --onlyvisible --name 'lyyIme 输入法设置' 2>/dev/null | head -1 || true)"
[[ -n "$SW" ]] || fail "输入设置窗未出现"
shot "F-settings-input"
if [[ $ATSPI -eq 1 ]]; then
    out="$(atspi_find "纯拼音模式" "check box,toggle button" contains)" || out=""
    [[ -n "$out" ]] || fail "设置窗找不到「纯拼音」复选框"
    [[ "$(awk '{print $NF}' <<<"$out")" == 0 ]] \
        || fail "配置当前为混输,设置页纯拼音框却为勾选态"
    echo "PASS F2:设置页纯拼音复选框 = 未勾选(与磁盘配置一致)"
else
    unverified "pyatspi 不可用:设置页复选框状态未自动断言,人工复核 $ART/F-settings-input.png"
fi
[[ $ATSPI -eq 1 ]] || fail "AT-SPI 不可用:无法物理点击设置窗「取消」按钮"
cxy="$(atspi_find "取消" "push button" || true)"
[[ -n "$cxy" ]] || { shot "F-cancel-missing"; fail "设置窗找不到可见「取消」按钮"; }
read -r ccx ccy _cw _ch <<<"$cxy"
xdotool mousemove "$ccx" "$ccy" click 1
sleep 0.3
for _ in $(seq 1 25); do
    [[ -n "${SW:-}" ]] && xwininfo -id "$SW" 2>/dev/null | grep -q "Map State: IsViewable" \
        || break
    sleep 0.2
done
if [[ -n "${SW:-}" ]] && xwininfo -id "$SW" 2>/dev/null | grep -q "Map State: IsViewable"; then
    fail "设置窗取消后仍可见(隐藏即关闭语义)"
fi
shot "F-settings-hidden"

echo "== [9.5/9] G:行右空白区 → 通用菜单;最右齿轮 → 真实设置 =="
reset_comp
fresh_comp "nihao"
candwin_geom || fail "候选窗几何不可得"
G1_OK=0
for numlbl in "5." "4." "3." "2." "1."; do
    row_xy=""
    if [[ $ATSPI -eq 1 ]]; then
        row_xy="$(atspi_find "$numlbl" "label" || true)"
    fi
    if [[ -n "$row_xy" ]]; then
        read -r rcx rcy rcw rch <<<"$row_xy"
        [[ $((rcx + rcw / 2)) -lt $((X + WIDTH - 8)) ]] || continue
        xdotool mousemove $((X + WIDTH - 4)) "$rcy" click 3
    else
        [[ $ATSPI -eq 1 ]] && continue
        xdotool mousemove $((X + WIDTH - 4)) $((Y + HEIGHT - 4)) click 3
    fi
    for _ in $(seq 1 60); do menu_visible "$CWID" && break; sleep 0.1; done
    if menu_visible "$CWID"; then
        if [[ $ATSPI -eq 1 ]]; then
            if atspi_find "纯拼音" "radio menu item" >/dev/null; then
                atspi_find "固定首位" "menu item" >/dev/null \
                    && fail "行右空白菜单混入词操作「固定首位」"
                G1_OK=1; break
            fi
        else
            G1_OK=1; break
        fi
    fi
    xdotool key Escape; sleep 0.3
done
[[ $G1_OK -eq 1 ]] || fail "行右空白右键未弹通用菜单"
xdotool key Escape
sleep 0.4
echo "PASS G1:行右空白区右键 → 通用菜单(词操作缺失断言过)"

reset_comp
CFG_SHA0="$(sha256sum "$CFG_PATH" | cut -d' ' -f1)"
fresh_comp "nihao"
candwin_geom || fail "候选窗几何不可得"
gear_xy=""
if [[ $ATSPI -eq 1 ]]; then
    gear_xy="$(atspi_find "打开设置" "push button" || atspi_find "打开设置" "any" || true)"
fi
if [[ -n "$gear_xy" ]]; then
    read -r gcx gcy gcw gch <<<"$gear_xy"
    xdotool mousemove "$gcx" "$gcy" click 1
else
    xdotool mousemove $((X + WIDTH - 6)) $((Y + 8)) click 1
fi
for _ in $(seq 1 60); do
    SW="$(xdotool search --onlyvisible --name 'lyyIme 输入法设置' 2>/dev/null | head -1 || true)"
    [[ -n "$SW" ]] && break
    sleep 0.2
done
[[ -n "$SW" ]] || { shot "G-gear-no-settings"; fail "齿轮左键未弹出设置窗"; }
shot "G-gear-settings"
if [[ $ATSPI -eq 1 ]] && atspi_find "取消" "push button" >/dev/null; then
    cxy="$(atspi_find "取消" "push button")"
    read -r ccx ccy _cw _ch <<<"$cxy"
    xdotool mousemove "$ccx" "$ccy" click 1
else
    xdotool windowactivate "$SW" 2>/dev/null || true
    xdotool key Escape
fi
for _ in $(seq 1 25); do
    xdotool search --onlyvisible --name 'lyyIme 输入法设置' >/dev/null 2>&1 || break
    sleep 0.2
done
xdotool search --onlyvisible --name 'lyyIme 输入法设置' >/dev/null 2>&1 \
    && fail "设置窗取消后未关闭"
shot "G-settings-hidden"
CFG_SHA1="$(sha256sum "$CFG_PATH" | cut -d' ' -f1)"
[[ "$CFG_SHA1" == "$CFG_SHA0" ]] \
    || { diff <(cat "$CFG_PATH") /dev/null >/dev/null; fail "设置取消后 config.toml 被改动"; }
focus_client
reset_comp
xdotool type --delay 90 "nihao"
wait_candwin
xdotool key space
wait_buf_eq "你好式你好" 8
echo "PASS G2:齿轮→真实设置窗,取消配置不变,nihao+Space→你好"

cp -f "$XIM_LOG" "$ART/xim.log" 2>/dev/null || true
cp -f "$CFG_PATH" "$ART/config.toml" 2>/dev/null || true
cp -f "$WORK/cfg.before" "$ART/config.before-learning.toml" 2>/dev/null || true
cp -f "$CLIENT_LOG" "$ART/client.log" 2>/dev/null || true
echo ""
echo "ALL PASS:候选条右键菜单 + 纯拼音方案 全链路验收通过"
echo "取证:$ART"
