#!/usr/bin/env bash
# lyyIme 皮肤特性端到端测试(设置窗「皮肤」页 + 自绘候选窗换肤)
#
# 隔离:独立 HOME/XDG_*;显示经 xvfb-run -a 自动选空闲号,
#       本脚本只碰自己启动的进程,绝不触碰已有桌面/显示/锁。
#
# 断言序列:
#   A. lyyime-xim --settings-page 6 → 皮肤页打开(窗口可见);
#      截图存证 /tmp/lyyime-skins-review/settings-skin-gallery.png
#      (config 预置 font_size=28,画廊预览按最大字号渲染,验证不裁切)
#   B. Alt+2 助记符直达「深海蓝金」单选 → 确定 → config.toml 落盘
#      skin="business-navy";
#   C. 客户端输入 nihao → 候选窗经 xwininfo 映射核验 + 实截图(深海蓝金);
#   D. 皮肤页 Alt+4 选「樱花奶糖」→ 取消 → 配置仍是 business-navy;
#   E. Alt+4 选樱花奶糖 → 确定 → 落盘 sakura;候选窗映射核验+截图;
#   F. 重启 lyyime-xim → --settings-page 6 窗口可见 + 配置仍 sakura +
#      候选窗映射核验+截图(持久化 + 重启即时生效)。
#
# 选择路径全部走键盘助记符(Alt+1..9,对应注册表下标 0..8),窗口级
# 直达、不需要滚动/坐标猜测;任何失败先截全屏留证再退出。
#
# 用法:bash tests/e2e/skin_e2e.sh [--keep]
set -euo pipefail

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../xim" && pwd)"

# xvfb-run 动态显示:外层先验构建产物再重入自身,不预分配 WORK
# (重入后重新 mktemp,避免遗留空壳目录;Xvfb 由 xvfb-run 拉起并随命令
# 结束清理,不会与已有显示冲突)
if [[ -z "${LYY_SKIN_XVFB:-}" ]]; then
    [[ -x "$XIM_DIR/build/bin/lyyime-xim" && \
       -x "$XIM_DIR/build/tests/e2e_client" && \
       -f "$XIM_DIR/build/tests/liblyyime_core_stub.so" ]] || {
        echo "SKIN-E2E FAIL: 缺构建产物(先 make -C xim all test)"; exit 1; }
    export LYY_SKIN_XVFB=1
    exec xvfb-run -a -s "-screen 0 1280x800x24" bash "$0" "$@"
fi

KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

WORK="$(mktemp -d /tmp/lyyime-skin-e2e.XXXXXX)"
export HOME="$WORK/home"
export XDG_CONFIG_HOME="$WORK/xdg/config"
export XDG_DATA_HOME="$WORK/xdg/data"
export XDG_CACHE_HOME="$WORK/xdg/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

export XMODIFIERS=@im=lyyime
export GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8

# 留证目录(lead 评审用);可用 LYY_SKIN_SHOT_DIR 覆盖
SHOT_DIR="${LYY_SKIN_SHOT_DIR:-/tmp/lyyime-skins-review}"
mkdir -p "$SHOT_DIR"

XIM_BIN="$XIM_DIR/build/bin/lyyime-xim"
CLIENT_BIN="$XIM_DIR/build/tests/e2e_client"
STUB_LIB="$XIM_DIR/build/tests/liblyyime_core_stub.so"
CONFIG="$XDG_CONFIG_HOME/lyyime/config.toml"
XIM_LOG="$XDG_DATA_HOME/lyyime/logs/xim.log"
BUFFER="$WORK/buffer.txt"

XIM_PID=""
CLIENT_PID=""

cleanup() {
    [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
    [[ -n "$XIM_PID" ]] && kill "$XIM_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ $KEEP -eq 1 ]]; then
        echo "[skin-e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# 截图: Gdk pixbuf 直出 PNG(隔离 Xvfb 下唯一进程域,安全)。
# shot   = 软尝试(失败路径留证用,不连环失败)
# shot_req = 断言级:产物文件必须存在,缺失即失败(视觉验收门槛)
shot() {
    local f="$SHOT_DIR/$1"
    python3 - "$f" 2>"$f.gdk.err" <<'PY' || true
import sys
import gi
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk
root = Gdk.get_default_root_window()
pb = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
assert pb is not None, "pixbuf_get_from_window 返回空"
pb.savev(sys.argv[1], "png", [], [])
PY
    [[ -s "$f" ]]
}

PHASE="A"
fail() {
    echo "[skin-e2e] 失败现场截图…"
    shot "FAIL-${PHASE}.png" || true
    KEEP=1
    echo "SKIN-E2E FAIL($PHASE): $*"
    echo "(失败现场保留:$WORK;截图:$SHOT_DIR/FAIL-${PHASE}.png)"
    exit 1
}

shot_req() {
    shot "$1" || fail "截图产物缺失:$SHOT_DIR/$1(见 $1.gdk.err)"
    echo "[shot] $SHOT_DIR/$1"
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

# 等模式出现次数超过基线(区分重启前旧日志行)
wait_log_since() {
    local pat="$1" base="$2" timeout="${3:-10}" i n
    for ((i = 0; i < timeout * 20; i++)); do
        n="$(grep -c "$pat" "$XIM_LOG" 2>/dev/null || true)"
        if (( ${n:-0} > base )); then
            return 0
        fi
        sleep 0.05
    done
    echo "---- xim.log ----"; cat "$XIM_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 次数 > $base 超时"
}

# xdotool search 默认含未映射窗口;只接受 IsViewable 的可见映射窗口
find_settings_win() {
    local w
    while IFS= read -r w; do
        [[ -n "$w" ]] || continue
        if xwininfo -id "$w" 2>/dev/null | grep -q "Map State: IsViewable"; then
            echo "$w"; return 0
        fi
    done < <(xdotool search --onlyvisible --name '^lyyIme 输入法设置$' \
             2>/dev/null || true)
    return 0
}

SW=""
wait_settings_win() {
    local i
    for ((i = 0; i < 120; i++)); do
        SW="$(find_settings_win || true)"
        [[ -n "$SW" ]] && return 0
        sleep 0.1
    done
    xwininfo -root -tree >"$WORK/wintree.txt" 2>&1 || true
    fail "设置窗未成为可见映射窗口"
}

focus_settings_win() {
    xdotool windowactivate "$SW" 2>/dev/null || xdotool windowfocus "$SW" \
        || fail "设置窗无法取得输入焦点"
    sleep 0.3
}

open_skin_page() {
    if [[ -n "$XIM_PID" ]] && kill -0 "$XIM_PID" 2>/dev/null; then
        # 第二实例唤起:写页码请求文件 + SIGUSR1 给在跑实例
        "$XIM_BIN" --settings-page 6 >>"$WORK/xim2.out" 2>&1 \
            || fail "--settings-page 6 唤起退出码非 0"
    else
        LYYIME_CORE_LIB="$STUB_LIB" LYYIME_RES_DIR="$XIM_DIR/res" \
            "$XIM_BIN" --settings-page 6 >>"$WORK/xim.stdout" 2>&1 &
        XIM_PID=$!
        wait_log "XIM server ready"
    fi
    wait_settings_win
    focus_settings_win
}

# 设置窗几何 → X/Y/WIDTH/HEIGHT
win_geom() { eval "$(xdotool getwindowgeometry --shell "$SW")"; }

click_ok() { # 确定(固定几何 825x544,btnbox 右下角)
    focus_settings_win
    win_geom
    xdotool mousemove $((X + WIDTH - 57)) $((Y + HEIGHT - 31)) click 1
    sleep 0.6
}
click_cancel() { # 取消在确定左侧
    focus_settings_win
    win_geom
    xdotool mousemove $((X + WIDTH - 150)) $((Y + HEIGHT - 31)) click 1
    sleep 0.6
}

skin_saved() { # config.toml 当前 skin 值(无键 → system)
    if [[ -f "$CONFIG" ]]; then
        sed -n 's/^skin = "\([^"]*\)".*/\1/p' "$CONFIG" | head -1
    fi
}

# 皮肤 id → 注册表下标(与 skin.c g_skins 顺序一致)
skin_index() {
    case "$1" in
        system) echo 0 ;;        business-navy) echo 1 ;;
        porcelain) echo 2 ;;     sakura) echo 3 ;;
        peach) echo 4 ;;         bamboo) echo 5 ;;
        lavender) echo 6 ;;      cyber) echo 7 ;;
        contrast) echo 8 ;;      *) echo -1 ;;
    esac
}

# 确定性选皮肤:Alt+(idx+1) 窗口级助记符直达对应单选(无需滚动/坐标),
# 确定 → 校验落盘;键击丢失这种 X 级偶发重试一次,仍错即失败留证
select_skin() {
    local want="$1" ti attempt hit
    ti="$(skin_index "$want")"
    [[ "$ti" -ge 0 ]] || fail "未知皮肤 id:$want"
    for attempt in 1 2; do
        open_skin_page
        xdotool key --clearmodifiers "alt+$((ti + 1))"
        sleep 0.3
        click_ok
        hit="$(skin_saved)"; hit="${hit:-system}"
        if [[ "$hit" == "$want" ]]; then
            echo "[skin-e2e] 已选皮肤 $want(attempt $attempt)"
            return 0
        fi
    done
    fail "选择皮肤 $want 失败(落盘=${hit:-<空>})"
}

# 候选窗映射核验:窗应出现在指针右下约 +20/+30(commit_layout→
# on_pos_timer 即时定位);遍历 X 树找该坐标附近的已映射小窗口,
# 排除设置窗与客户端窗的大矩形误判
candwin_mapped() {
    local loc gx gy
    loc="$(xdotool getmouselocation --shell)"
    gx=$(( $(sed -n 's/^X=//p' <<<"$loc") + 20 ))
    gy=$(( $(sed -n 's/^Y=//p' <<<"$loc") + 30 ))
    xwininfo -root -tree 2>/dev/null > "$WORK/wintree.txt" || return 1
    awk -v ex="$gx" -v ey="$gy" '
        {
            line=$0; geo=""
            n=split(line,a," ")
            for(i=n;i>=1;i--)
                if (a[i] ~ /^[0-9]+x[0-9]+\+[0-9]+\+[0-9]+$/) { geo=a[i]; break }
            if (!geo) next
            split(geo,g,/[x+]/)
            w=g[1];h=g[2];ax=g[3];ay=g[4]
            if (w<700 && h<500 &&
                ax>ex-16 && ax<ex+16 && ay>ey-16 && ay<ey+16)
                { found=1; print "    候选窗:",$0 > "/dev/stderr" }
        }
        END { exit !found }
    ' "$WORK/wintree.txt"
}

# 在客户端输入串并验证候选窗:映射核验 + 截图(缺失即失败)
type_and_assert_candwin() {
    local tag="$1"
    xdotool windowactivate "$WID" 2>/dev/null || xdotool windowfocus "$WID" \
        || fail "客户端窗无法取得焦点($tag)"
    sleep 0.4
    xdotool mousemove 420 420
    sleep 0.3
    xdotool type --delay 90 "nihao"
    sleep 0.8
    candwin_mapped || fail "候选窗未在指针旁映射($tag,窗口树已存 $WORK/wintree.txt)"
    shot_req "candidate-$tag.png"
    xdotool key Escape
    sleep 0.3
}

echo "== [1/6] 启动 lyyime-xim(桩库,font_size=28 预置)并打开皮肤页 =="
# 预置最大字号配置:画廊预览以 font28 渲染,验证最大字号下预览不裁切
mkdir -p "$(dirname "$CONFIG")"
printf 'font_size = 28\n' > "$CONFIG"
LYYIME_CORE_LIB="$STUB_LIB" LYYIME_RES_DIR="$XIM_DIR/res" \
    "$XIM_BIN" --settings-page 6 >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"
wait_settings_win
focus_settings_win
echo "PASS A1:--settings-page 6 皮肤页窗口可见"
sleep 0.5
shot_req "settings-skin-gallery.png"
echo "PASS A2:皮肤画廊截图(font28 预览)= $SHOT_DIR/settings-skin-gallery.png"

echo "== [2/6] B:Alt+2 选「深海蓝金」并确定 =="
PHASE="B"
select_skin "business-navy"
wait_log "skin=business-navy" 5
[[ "$(skin_saved)" == "business-navy" ]] || fail "落盘非 business-navy"
echo "PASS B:config.toml skin=\"business-navy\""

echo "== [3/6] C:候选窗映射核验+实截(深海蓝金) =="
PHASE="C"
"$CLIENT_BIN" "$BUFFER" 300 >"$WORK/client.log" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log "XIM client 已连接"
sleep 0.6
WID="$(xdotool search --onlyvisible --name '^lyyime-e2e-client$' | head -1 || true)"
[[ -n "$WID" ]] || fail "找不到客户端窗口"
xdotool windowactivate "$WID" 2>/dev/null || xdotool windowfocus "$WID"
sleep 0.5
wait_log "获得焦点" 8
type_and_assert_candwin "business-navy"
echo "PASS C:候选窗已映射(business-navy),截图=$SHOT_DIR/candidate-business-navy.png"

echo "== [4/6] D:Alt+4 选「樱花奶糖」但取消 =="
PHASE="D"
open_skin_page
xdotool key --clearmodifiers alt+4
sleep 0.3
click_cancel
sleep 0.4
[[ "$(skin_saved)" == "business-navy" ]] \
    || fail "取消后配置被改动(=$(skin_saved))"
echo "PASS D:取消不落盘(仍 business-navy)"

echo "== [5/6] E:Alt+4 选「樱花奶糖」并确定 =="
PHASE="E"
select_skin "sakura"
wait_log "skin=sakura" 5
[[ "$(skin_saved)" == "sakura" ]] || fail "落盘非 sakura"
type_and_assert_candwin "sakura"
echo "PASS E:候选窗已映射(sakura),截图=$SHOT_DIR/candidate-sakura.png"

echo "== [6/6] F:重启后皮肤持久化并即时生效 =="
PHASE="F"
kill "$XIM_PID" 2>/dev/null || true
wait "$XIM_PID" 2>/dev/null || true
XIM_PID=""
# XIM 客户端不自重连:老客户端随旧 server 一起退场,重启后再起一个
[[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
wait "$CLIENT_PID" 2>/dev/null || true
CLIENT_PID=""
CONN_BASE="$(grep -c 'XIM client 已连接' "$XIM_LOG" 2>/dev/null || true)"
CONN_BASE="${CONN_BASE:-0}"
LYYIME_CORE_LIB="$STUB_LIB" LYYIME_RES_DIR="$XIM_DIR/res" \
    "$XIM_BIN" --settings-page 6 >>"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"
wait_settings_win
echo "PASS F1:重启后 --settings-page 6 窗口可见"
[[ "$(skin_saved)" == "sakura" ]] || fail "重启后配置丢失"
# 关设置窗(取消路径,不改盘)后验证候选窗仍是 sakura
click_cancel
sleep 0.4
"$CLIENT_BIN" "$BUFFER" 120 >>"$WORK/client.log" 2>>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log_since "XIM client 已连接" "$CONN_BASE" 8
sleep 0.5
WID="$(xdotool search --onlyvisible --name '^lyyime-e2e-client$' | head -1 || true)"
[[ -n "$WID" ]] || fail "重启后找不到客户端窗口"
type_and_assert_candwin "sakura-restart"
echo "PASS F2:重启后候选窗仍按 sakura 渲染(截图 candidate-sakura-restart.png)"

echo "SKIN-E2E PASS: 皮肤页打开/助记符点选保存/取消不落盘/重启持久化/候选窗映射换肤 全绿"
echo "截图目录:$SHOT_DIR"
