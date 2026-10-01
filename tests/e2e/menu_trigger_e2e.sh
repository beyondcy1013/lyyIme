#!/usr/bin/env bash
# lyyIme 菜单触发端到端测试(2026-09-30 特性验收;独立 Xvfb,全自动)
#
# 与 xim_e2e.sh 的区别:用**真库** $CARGO_TARGET_DIR/release/liblyyime_core.so
# (目录/别名的唯一权威在 Rust menu_trigger.rs,桩库不再复刻),
# 词库用 LYYIME_DATA_DIR 指向的极短五笔夹具,
# HOME/XDG_* 全隔离;显示经 Xvfb -displayfd 自选空闲号(本脚本创建的资源
# 由本脚本清理,绝不碰任何预先存在的显示/锁/进程)。
#
# 断言序列:
#   1. sz+空格 上屏「设置」→ 候选窗可见 + 日志 hint + 截图;>4s 仍存;
#      F7 确认 → 设置窗打开,Entry 缓冲不变。
#   2. 设置页5(菜单触发):启用复选框/F7 组合框/16 黑名单条目(ctypes
#      数目录 + 截图存证;AT-SPI 可用时导出 UI 树);UI 改确认键 F8、
#      勾选禁用 help+settings,确定保存即生效(不重启 IME)。
#   3. 黑名单证明后 UI 重新启用 help → hb/hc 分段「帮+助」续接命中;
#      空缓冲 Enter / CapsLock 直通 / 编辑 / Ctrl+Fn / 焦点边界取消;
#      未命中 Fn 直通;英文确认 → 直通 xy。
#
# 用法:bash tests/e2e/menu_trigger_e2e.sh [--keep]
set -euo pipefail

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../xim" && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

WORK="$(mktemp -d /tmp/lyyime-mt-e2e.XXXXXX)"
# 隔离运行环境:配置/日志/pidfile/用户词库全部落到 WORK 下
export HOME="$WORK/home"
export XDG_CONFIG_HOME="$WORK/xdg/config"
export XDG_DATA_HOME="$WORK/xdg/data"
export XDG_CACHE_HOME="$WORK/xdg/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

# 真库:release 构建产物;缺失直接失败(验收必须过真匹配器)
CORE_SO="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyime}/release/liblyyime_core.so"
[[ -f "$CORE_SO" ]] || { echo "FAIL: 真库不存在:$CORE_SO(先跑 cargo build --release)"; exit 1; }

# 词库夹具:短五笔表(LYYIME_DATA_DIR 被 core 直接当词典目录读 wubi.tsv)
DICT="$WORK/dict"
mkdir -p "$DICT"
cat > "$DICT/wubi.tsv" <<'DIC'
sz	设置	1000
bz	帮助	1000
yw	英文	1000
s	设	800
z	置	800
tj	输入统计	1000
xx	修复输入法	1000
hb	帮	900
hc	助	900
jp	截屏	1000
jt	截图	1000
nihao	你好	1000
DIC

# 配置夹具:关旧快速功能键(sz/bz 等上屏即得中文,不受功能候选干扰);
# 关四码自动上屏(空格显式选词,确定性);关词组提示(避免抢占辅助区)
mkdir -p "$XDG_CONFIG_HOME/lyyime"
cat > "$XDG_CONFIG_HOME/lyyime/config.toml" <<'CFG'
quick_actions_enabled = false
commit_after_four = false
commit_first_at_four = false
commit_unique_four = false
phrase_hint = false
menu_trigger_enabled = true
menu_trigger_key = 7
menu_trigger_disabled = "fix_ime"
shot_hotkey = "ctrl+shift+F9"
CFG

export XMODIFIERS=@im=lyyime
export GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8

XIM_LOG="$XDG_DATA_HOME/lyyime/logs/xim.log"
CLIENT_LOG="$WORK/client.log"
BUFFER="$WORK/buffer.txt"
XIM_BIN="$XIM_DIR/build/bin/lyyime-xim"
CLIENT_BIN="$XIM_DIR/build/tests/e2e_client"

cleanup() {
    [[ -n "${CLIENT_PID:-}" ]] && kill "$CLIENT_PID" 2>/dev/null || true
    [[ -n "${XIM_PID:-}" ]] && kill "$XIM_PID" 2>/dev/null || true
    [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ $KEEP -eq 1 ]]; then
        echo "[mt-e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

fail() { KEEP=1; echo "MT-E2E FAIL: $*"; echo "(失败现场保留:$WORK)"; exit 1; }

wait_buffer() {
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ -f "$BUFFER" ]] && grep -qF "$want" "$BUFFER" && return 0
        sleep 0.05
    done
    echo "---- 当前缓冲 ----"; cat "$BUFFER" 2>/dev/null || echo "(空)"
    echo "---- xim.log 尾部 ----"; tail -30 "$XIM_LOG" 2>/dev/null || true
    fail "等待缓冲出现 [$want] 超时"
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

# 等待某日志行数超过基线(新增出现判定用)
wait_log_since() {
    local pat="$1" base="$2" timeout="${3:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ $(log_count "$pat") -gt $base ]] && return 0
        sleep 0.05
    done
    echo "---- xim.log ----"; cat "$XIM_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 新增超时(基线 $base)"
}

# 截图(留证):Gdk pixbuf 直出 PNG 为首选(worker 无 xwd/import);
# 其次 xwd→PNG(convert/magick/PIL),最后 import;全缺记说明不失败
shot() {
    local tag="$1" f="$WORK/shots/$1"
    mkdir -p "$WORK/shots"
    if python3 - "$f.png" 2>"$f.gdk.err" <<'PY'
import sys
import gi
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk
root = Gdk.get_default_root_window()
pb = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
assert pb is not None, "pixbuf_get_from_window 返回空"
pb.savev(sys.argv[1], "png", [], [])
PY
    then
        echo "[shot] $f.png"; return 0
    fi
    if command -v xwd >/dev/null 2>&1 && xwd -silent -root -out "$f.xwd" 2>/dev/null; then
        if command -v convert >/dev/null 2>&1 && convert "$f.xwd" "$f.png" 2>/dev/null; then
            echo "[shot] $f.png"; return 0
        fi
        if command -v magick >/dev/null 2>&1 && magick "$f.xwd" "$f.png" 2>/dev/null; then
            echo "[shot] $f.png"; return 0
        fi
        if python3 -c 'from PIL import Image' 2>/dev/null && \
           python3 -c 'import sys;from PIL import Image;Image.open(sys.argv[1]).save(sys.argv[2])' \
               "$f.xwd" "$f.png" 2>/dev/null; then
            echo "[shot] $f.png"; return 0
        fi
        echo "[shot] $f.xwd(无 PNG 转换器)"; return 0
    fi
    if command -v import >/dev/null 2>&1 && \
       import -window root "$f.png" 2>/dev/null; then
        echo "[shot] $f.png"; return 0
    fi
    echo "[shot] $tag:无截图工具(Gdk/xwd/import 均不可用)" \
        | tee -a "$WORK/shots/NO-SHOTS.txt"
}

# 窗口树取证:聚焦/找窗失败时导出 xwininfo 全树与设置窗映射态
dump_win_tree() {
    local tag="$1"
    local f="$WORK/wintree-$tag.txt"
    {
        echo "=== xwininfo -root -tree ==="
        xwininfo -root -tree
        echo "=== 设置窗(含未映射)详查 ==="
        xdotool search --name '输入法设置' 2>/dev/null | while read -r _w; do
            echo "-- win $_w --"
            xwininfo -id "$_w" 2>&1
            xdotool getwindowgeometry "$_w" 2>&1
        done || true
    } >"$f" 2>&1 || true
    echo "[diag] $f"
}

# 聚焦前先证明窗口已映射可见;失败时落窗口树证据再退出
focus_window() {
    local w="$1" tag="$2"
    if ! xdotool windowactivate "$w" 2>"$WORK/focus-$tag.err"; then
        xdotool windowfocus "$w" 2>>"$WORK/focus-$tag.err" || {
            dump_win_tree "$tag"
            cat "$WORK/focus-$tag.err" 2>/dev/null
            fail "窗口聚焦失败($tag):BadMatch=窗口未映射?"
        }
    fi
    sleep 0.3
}

# 缓冲尾部断言:整串已在历史缓冲中出现过时,用尾部窗口区分新上屏
wait_buffer_tail() {
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        # 严格后缀匹配(Entry 无换行,$(cat) 剥尾换行不影响)
        [[ -f "$BUFFER" ]] && [[ "$(cat "$BUFFER")" == *"$want" ]] && return 0
        sleep 0.05
    done
    echo "---- 当前缓冲尾 ----"; tail -c 200 "$BUFFER" 2>/dev/null
    echo "---- xim.log 尾部 ----"; tail -20 "$XIM_LOG" 2>/dev/null
    fail "等待缓冲尾部出现 [$want] 超时"
}

# 候选窗(菜单提示载体)是否被映射为可见
candwin_viewable() {
    local ids w
    ids="$(xwininfo -root -children 2>/dev/null | grep '"lyyime-xim"' \
        | awk '{print $1}' || true)"
    while IFS= read -r w; do
        [[ -n "$w" ]] || continue
        if xwininfo -id "$w" 2>/dev/null | grep -q "Map State: IsViewable"; then
            return 0
        fi
    done <<<"$ids"
    return 1
}

# xdotool search 默认含未映射窗口:对未映射窗聚焦会 X_SetInputFocus
# BadMatch(12:33 回调实证)。只接受 IsViewable 的可见映射窗口。
find_settings_win() {
    local out w
    out="$(xdotool search --onlyvisible --name '^lyyIme 输入法设置$' \
        2>/dev/null || true)"
    while IFS= read -r w; do
        [[ -n "$w" ]] || continue
        if xwininfo -id "$w" 2>/dev/null | grep -q "Map State: IsViewable"; then
            echo "$w"
            return 0
        fi
    done <<<"$out"
    return 0
}

# 轮询等设置窗真正可见;超时落窗口树诊断再失败
wait_settings_win() {
    local i
    for ((i = 0; i < 120; i++)); do
        SW="$(find_settings_win || true)"
        [[ -n "$SW" ]] && return 0
        sleep 0.1
    done
    dump_win_tree "settings-timeout"
    # 判别"窗口未映射"的根因:抓隔离 XIM 进程全部线程栈
    # (settings show 入口有日志而 present 未返回 → 需要栈证据定性;
    #  仅对本脚本 PID,限 5s,权限不足只记录不 sudo)
    if command -v gdb >/dev/null 2>&1 && [[ -n "${XIM_PID:-}" ]]; then
        timeout 5 gdb -batch -ex 'set debuginfod enabled off' \
            -ex 'thread apply all bt' -p "$XIM_PID" \
            >"$WORK/settings-blocked-backtrace.txt" 2>&1 || \
            echo "(gdb attach/超时:rc=$?)" >>"$WORK/settings-blocked-backtrace.txt"
    fi
    fail "设置窗未成为可见映射窗口"
}

WID=""
focus_client() {
    [[ -n "$WID" ]] || \
        WID="$(xdotool search --onlyvisible --name '^lyyime-e2e-client$' | head -1 || true)"
    [[ -n "$WID" ]] || { dump_win_tree "client-missing"; fail "找不到客户端窗口"; }
    focus_window "$WID" "client"
    sleep 0.1
}

# 候选行点击:实测几何(825x544)——复选框中心 X+37,行区顶 top=Y+H-290,
# 行中心 top+14+i*29(settings=268/help=297/english=326);点完移走指针
# 防悬停 tooltip 干扰后续截图/点击(X/Y/WIDTH/HEIGHT 需先 eval 注入)
row_click() {
    local i="$1" top="$2"
    xdotool mousemove "$((X + 37))" "$((top + 14 + i * 29))" click 1
    sleep 0.25
    xdotool mousemove "$((X + 400))" "$((Y + HEIGHT - 372))"
    sleep 0.15
}

echo "== [1/10] 构建检查、CLI 参数与显示 =="
[[ -x "$XIM_BIN" && -x "$CLIENT_BIN" ]] || fail "缺构建产物(先 make -C xim lyyime-xim test)"

# --settings-page 严格解析:非法/缺值/越界一律退出码 2
# (参数在单实例信号与 X 初始化之前解析,不影响任何已运行实例)
for spec in "--settings-page" "--settings-page -1" "--settings-page 7" \
            "--settings-page abc" "--settings-page=foo"; do
    rc=0
    "$XIM_BIN" $spec >"$WORK/cli.out" 2>&1 || rc=$?
    if [[ $rc -ne 2 ]]; then
        cat "$WORK/cli.out" 2>/dev/null || true
        fail "--settings-page 参数 [$spec] 应退出码 2,实得 $rc"
    fi
done
echo "PASS CLI:--settings-page 非法输入(missing/-1/7/abc/=foo)全退 2"

# Xvfb -displayfd:自选空闲显示号;本脚本创建的 Xvfb 由本脚本清理,
# 绝不触碰预先存在的显示/锁/进程
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
XDNUM="$DISP"
XSOCK="/tmp/.X11-unix/X${XDNUM}"
for _ in $(seq 1 50); do [[ -S "$XSOCK" ]] && break; sleep 0.1; done
echo "[mt-e2e] Xvfb 就绪 DISPLAY=$DISPLAY(pid=$XVFB_PID)"

echo "== [2/10] 启动 lyyime-xim(真库 $CORE_SO) =="
LYYIME_CORE_LIB="$CORE_SO" LYYIME_RES_DIR="$XIM_DIR/res" \
    LYYIME_DATA_DIR="$DICT" \
    "$XIM_BIN" >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"
wait_log "菜单触发:enabled=1 key=F7" 8

echo "== [3/10] 客户端接入 =="
"$CLIENT_BIN" "$BUFFER" 600 >"$CLIENT_LOG" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log "XIM client 已连接"
sleep 0.8
wait_log "获得焦点"
focus_client

echo "== [4/10] A:上屏「设置」→ 持续可见提示 → F7 开设置 =="
xdotool type --delay 90 "sz"; sleep 0.5
xdotool key space
wait_buffer "设置" 8
wait_log "menu-trigger hint: .*「设置」.*F7" 8
candwin_viewable || fail "菜单提示未在候选窗可见(IsViewable)"
shot "a1-hint-visible"

# 提示持久性:超过 notice 定时(4s)后提示仍在,不自动撤下
sleep 5
candwin_viewable || fail "菜单提示 <5s 内自行消失(应持续至下次输入)"
shot "a2-hint-after-5s"
echo "PASS A1:提示可见且 >4s 持续(截图存证)"

# F7 确认 → 设置窗打开;已上屏文本不变
xdotool key F7
wait_log "menu-trigger: F7 确认执行" 8
wait_log "确认执行 settings" 8
wait_settings_win   # 轮询到 IsViewable 才算"打开"(未映射窗会 BadMatch)
wait_buffer "设置" 3
shot "a3-settings-open"

# F7 已被吞:配对 press/release 都不泄给应用(无 forward F7)
if grep -q "forward keysym=0xffc4" "$XIM_LOG"; then
    fail "被吞 F7 泄漏到了应用(forward F7)"
fi
echo "PASS A2:F7 打开设置窗且吞键,Entry 缓冲不变"

echo "== [5/10] B:菜单触发页(启用框/F7 组合框/16 条目)=="
# 目录经 ctypes 从真库直读:16 项是 UI 勾选行的唯一来源
python3 - "$CORE_SO" <<'PY'
import ctypes, sys
lib = ctypes.CDLL(sys.argv[1])
n = lib.lyyime_menu_trigger_count()
assert n == 16, f"目录应为 16 项,实得 {n}"
buf = ctypes.create_string_buffer(128)
ids = []
for i in range(n):
    lib.lyyime_menu_trigger_id(i, buf, 128)
    ids.append(buf.value.decode())
assert ids[0] == "settings" and ids[15] == "settings_menu", ids
# 越界(含负数)取空串
lib.lyyime_menu_trigger_id(-1, buf, 128)
assert buf.value == b"", "负下标应取空串"
print(f"[ctypes] 目录 16 项 OK: {ids}")
PY
# 设置窗当前在常规页;Ctrl+Page_Down ×5 到菜单触发页并截图存证
dump_win_tree "b-before-focus"   # 聚焦前取证(BadMatch 问题点)
focus_window "$SW" "settings-b"
sleep 0.1
for _ in 1 2 3 4 5; do xdotool key ctrl+Page_Down; sleep 0.15; done
sleep 0.4
shot "b1-menu-page-top"
# 滚到底看尾部条目(settings_menu 等),再滚回顶部
eval "$(xdotool getwindowgeometry --shell "$SW" || true)"
# 行/组合框坐标按实测 825x544 几何推导,尺寸不符先截图再失败
[[ $WIDTH -eq 825 && $HEIGHT -eq 544 ]] \
    || { shot "b0-unexpected-geom"; \
         fail "设置窗几何 ${WIDTH}x${HEIGHT} 非预期 825x544,需复核行坐标"; }
SCROLL_X=$((X + WIDTH / 2))
SCROLL_TOP=$((Y + HEIGHT - 290))   # 实测行区顶=254:settings 268/help 297/english 326
xdotool mousemove "$SCROLL_X" "$((SCROLL_TOP + 110))" click 5 click 5 click 5 click 5
sleep 0.3
shot "b2-menu-page-bottom"
xdotool mousemove "$SCROLL_X" "$((SCROLL_TOP + 110))" \
    click 4 click 4 click 4 click 4 click 4 click 4 click 4 click 4   # wheel4x8 回顶部
sleep 0.3
# AT-SPI 可用时导出菜单触发页可访问对象树(名/角色/勾选态)作 UI 证据;
# 无 at-spi 总线则记说明,不失败
if python3 -c 'import pyatspi' 2>/dev/null; then
    python3 - "$WORK/atspi-menu-page.txt" <<'PY' || true
import sys
import pyatspi
out = open(sys.argv[1], "w", encoding="utf-8")
def walk(n, d):
    if d > 12:
        return
    try:
        st = n.getState()
        checked = st.contains(pyatspi.STATE_CHECKED)
        out.write("%s%s role=%s checked=%s\n"
                  % ("  " * d, n.name or "", n.getRoleName(), checked))
    except Exception:
        pass
    try:
        cnt = n.childCount
    except Exception:
        return
    for i in range(cnt):
        try:
            walk(n.getChildAtIndex(i), d + 1)
        except Exception:
            pass
try:
    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        for j in range(app.childCount):
            w = app.getChildAtIndex(j)
            if "lyyIme" in (w.name or "") or "设置" in (w.name or ""):
                out.write("== window %s ==\n" % w.name)
                walk(w, 0)
except Exception as e:
    out.write("AT-SPI 枚举失败:%s\n" % e)
out.close()
PY
    [[ -s "$WORK/atspi-menu-page.txt" ]] \
        && echo "[atspi] $WORK/atspi-menu-page.txt" \
        || echo "(AT-SPI 无可用节点)" > "$WORK/atspi-menu-page.txt"
else
    echo "(python3 无 pyatspi,跳过 AT-SPI 树导出)" > "$WORK/atspi-menu-page.txt"
fi
echo "PASS B:菜单触发页可见(16 条目 ctypes 核对 + 顶/底截图存证)"

echo "== [6/10] C:UI 内改 F8 + 禁用帮助/设置,保存即生效 =="
# 勾选目录行(勾选=禁止文字触发):行 0=settings,行 1=help
row_click 0 "$SCROLL_TOP"
row_click 1 "$SCROLL_TOP"
shot "c1-rows-checked"
# 确认键组合框:实测中心 (X+112, Y+H-372=172);点击后键盘选 F8
# (当前 F7 → Down → Return)
xdotool mousemove "$((X + 112))" "$((Y + HEIGHT - 372))" click 1
sleep 0.4
shot "c2-combo-open"
xdotool key Down; sleep 0.2; xdotool key Return
sleep 0.3
shot "c3-combo-f8"
# 确定保存(btnbox=end:实测中心 X+W-57, Y+H-31)
xdotool mousemove "$((X + WIDTH - 57))" "$((Y + HEIGHT - 31))" click 1
wait_log "设置已保存并生效" 8
wait_log "mtkey=F8" 8
grep -q 'mtdis=\[[^]]*settings' "$XIM_LOG" || fail "保存后 mtdis 未含 settings"
grep -q 'mtdis=\[[^]]*help' "$XIM_LOG" || fail "保存后 mtdis 未含 help"
wait_log "菜单触发:enabled=1 key=F8" 8
sleep 0.6
focus_client
echo "PASS C:F8 + help/settings 黑名单 经 UI 保存即生效(未重启 IME)"

echo "== [7/10] D:新配置生效(帮助/设置无提示;F7 不再确认)=="
xdotool type --delay 90 "bz"; sleep 0.5; xdotool key space
wait_buffer "设置帮助" 8
sleep 0.5
if grep -q "menu-trigger hint: .*「帮助」" "$XIM_LOG"; then
    fail "help 已禁用仍出菜单提示"
fi
xdotool type --delay 90 "sz"; sleep 0.5; xdotool key space
wait_buffer "设置帮助设置" 8
sleep 0.5
if [[ $(log_count "menu-trigger hint: .*「设置」") -gt 1 ]]; then
    fail "settings 已禁用仍出菜单提示"
fi
# F7 不再是确认键(无待执行 + 非配置键):按下应直通
FWD7="$(log_count 'forward keysym=0xffc4')"
xdotool key F7
wait_log_since "forward keysym=0xffc4" "$FWD7" 5
echo "PASS D:禁用项无提示,F7 直通(配置键已改 F8)"

echo "== [8/10] D2:UI 重新启用 help(黑名单已证,还原匹配验证面) =="
# 第二实例请求直达菜单触发页:反勾选 help 行(行 1),settings 保持禁用
"$XIM_BIN" --settings-page 5 >>"$WORK/xim.stdout" 2>&1 \
    || fail "D2:--settings-page 5 第二实例退出码非 0"
wait_settings_win
[[ -n "$SW" ]] || fail "D2:第二实例未唤起设置窗"
dump_win_tree "d2-before-focus"
focus_window "$SW" "settings-d2"
sleep 0.1
eval "$(xdotool getwindowgeometry --shell "$SW" || true)"
[[ $WIDTH -eq 825 && $HEIGHT -eq 544 ]] \
    || { shot "d2-unexpected-geom"; \
         fail "设置窗几何 ${WIDTH}x${HEIGHT} 非预期 825x544,需复核行坐标"; }
SCROLL_X=$((X + WIDTH / 2))
SCROLL_TOP=$((Y + HEIGHT - 290))
xdotool mousemove "$SCROLL_X" "$((SCROLL_TOP + 110))" \
    click 4 click 4 click 4 click 4 click 4 click 4 click 4 click 4   # wheel4x8 回顶部
sleep 0.3
row_click 1 "$SCROLL_TOP"
shot "d2-help-unchecked"
# 确定保存(btnbox=end:实测中心 -57/-31);保存前记基线,防止命中上一轮日志
SAVE_N="$(log_count '设置已保存并生效')"
xdotool mousemove "$((X + WIDTH - 57))" "$((Y + HEIGHT - 31))" click 1
wait_log_since "设置已保存并生效" "$SAVE_N" 8
LAST_MTDIS="$(grep 'mtdis=' "$XIM_LOG" | tail -1 || true)"
echo "D2 mtdis: $LAST_MTDIS"
echo "$LAST_MTDIS" | grep -q 'settings' || fail "D2:重存后 mtdis 不含 settings"
if echo "$LAST_MTDIS" | grep -q 'help'; then
    fail "D2:help 行反勾选未生效(仍禁用)"
fi
wait_log "菜单触发:enabled=1 key=F8" 8
sleep 0.6
focus_client
echo "PASS D2:help 经 UI 重新启用(settings 仍禁用),未重启 IME"

echo "== [9/10] E:续接命中/英文直通/边界取消(匹配器实证) =="
# 分段上屏:hb→帮 + hc→助 → 尾串「帮助」命中(help 已启用)
HB="$(log_count 'menu-trigger hint: .*「帮助」')"
xdotool type --delay 90 "hb"; sleep 0.3; xdotool key space
wait_buffer_tail "帮" 8
xdotool type --delay 90 "hc"; sleep 0.3; xdotool key space
wait_buffer_tail "帮助" 8
wait_log_since "menu-trigger hint: .*「帮助」" "$HB" 8
candwin_viewable || fail "帮+助 命中提示未在候选窗可见(IsViewable)"
shot "e1-split-hint"
# F8 确认 → help 动作;Entry 缓冲不变
xdotool key F8
wait_log "确认执行 help" 8
wait_buffer_tail "帮助" 3
echo "PASS E1:帮+助 分段续接命中,F8 确认执行 help"

# 取消 1:普通键取消待执行(尾串保留),F8 直通
HB="$(log_count 'menu-trigger hint: .*「帮助」')"
xdotool type --delay 90 "bz"; sleep 0.5; xdotool key space
wait_buffer_tail "帮助帮助" 8
wait_log_since "menu-trigger hint: .*「帮助」" "$HB" 8
xdotool key a; sleep 0.3    # 普通键取消待执行(a 进组词缓冲)
xdotool key Escape; sleep 0.3
FWD8="$(log_count 'forward keysym=0xffc5')"
xdotool key F8
wait_log_since "forward keysym=0xffc5" "$FWD8" 5
echo "PASS E2:普通键取消待执行后 F8 直通"

# 取消 2:Ctrl+F9(修饰组合=硬边界,不是确认)
EB="$(log_count 'menu-trigger hint: .*「英文」')"
xdotool type --delay 90 "yw"; sleep 0.5; xdotool key space
wait_buffer_tail "英文" 8
wait_log_since "menu-trigger hint: .*「英文」" "$EB" 8
FWD9="$(log_count 'forward keysym=0xffc6')"
xdotool key ctrl+F9
wait_log_since "forward keysym=0xffc6" "$FWD9" 5
FWD8="$(log_count 'forward keysym=0xffc5')"
xdotool key F8
wait_log_since "forward keysym=0xffc5" "$FWD8" 5
echo "PASS E3:Ctrl+F9 硬复位后 F8 直通"

# 取消 3:BackSpace 编辑键硬边界
EB="$(log_count 'menu-trigger hint: .*「英文」')"
xdotool type --delay 90 "yw"; sleep 0.5; xdotool key space
wait_buffer_tail "英文英文" 8
wait_log_since "menu-trigger hint: .*「英文」" "$EB" 8
xdotool key BackSpace
sleep 0.3
FWD8="$(log_count 'forward keysym=0xffc5')"
xdotool key F8
wait_log_since "forward keysym=0xffc5" "$FWD8" 5
echo "PASS E4:BackSpace 取消待执行"

# 未命中 Fn 直通:待执行期间 F6 不确认、按键放行;其后待执行已取消
EB="$(log_count 'menu-trigger hint: .*「英文」')"
xdotool type --delay 90 "yw"; sleep 0.5; xdotool key space
wait_log_since "menu-trigger hint: .*「英文」" "$EB" 8
FWD6="$(log_count 'forward keysym=0xffc3')"
xdotool key F6
wait_log_since "forward keysym=0xffc3" "$FWD6" 5
FWD8="$(log_count 'forward keysym=0xffc5')"
xdotool key F8
wait_log_since "forward keysym=0xffc5" "$FWD8" 5
echo "PASS E5:待执行期间 F6 直通(普通键取消后 F8 亦直通)"

# 直通边界 1:空缓冲 Enter(pass)复位尾串:帮 → Enter → 助 不命中
HB="$(log_count 'menu-trigger hint')"
xdotool type --delay 90 "hb"; sleep 0.3; xdotool key space
wait_buffer_tail "帮" 8
xdotool key Return   # 空缓冲 → pass → 匹配器复位
sleep 0.3
xdotool type --delay 90 "hc"; sleep 0.3; xdotool key space
wait_buffer_tail "帮助" 8   # 文本仍上屏,只是尾串已断
sleep 0.5
[[ $(log_count 'menu-trigger hint') -eq $HB ]] || {
    echo "---- xim.log ----"; tail -40 "$XIM_LOG"
    fail "Enter 复位后「助」仍补全命中(hint 数异常)"
}
echo "PASS E6:空缓冲 Enter 直通复位尾串,帮+助 不命中"

# 直通边界 2:CapsLock 字母直通同为硬边界
HB="$(log_count 'menu-trigger hint')"
xdotool type --delay 90 "hb"; sleep 0.3; xdotool key space
wait_buffer_tail "帮" 8
xdotool key Caps_Lock; sleep 0.3
xdotool key a; sleep 0.3    # CapsLock 态 'a'→'A' 直通进 Entry
wait_buffer_tail "A" 8
xdotool key Caps_Lock; sleep 0.3
xdotool type --delay 90 "hc"; sleep 0.3; xdotool key space
wait_buffer_tail "A助" 8
sleep 0.5
[[ $(log_count 'menu-trigger hint') -eq $HB ]] || {
    echo "---- xim.log ----"; tail -40 "$XIM_LOG"
    fail "CapsLock 直通后「助」仍补全命中(hint 数异常)"
}
echo "PASS E7:CapsLock 字母直通复位尾串"

# 英文确认:F8 → trigger off,xy 直通
EB="$(log_count 'menu-trigger hint: .*「英文」')"
xdotool type --delay 90 "yw"; sleep 0.5; xdotool key space
wait_buffer_tail "英文" 8
wait_log_since "menu-trigger hint: .*「英文」.*F8" "$EB" 8
xdotool key F8
wait_log "menu-trigger: F8 确认执行" 8
wait_log "确认执行 english" 8
sleep 0.6
xdotool type --delay 90 "xy"
wait_buffer_tail "xy" 8
echo "PASS E8:英文确认 → 英文直通态,xy 直达应用"

# 回中文(Shift 单击;Shift press 本身是硬边界,此处无待执行)
xdotool key Shift_L
wait_log "trigger on" 8
sleep 0.5

echo "== [10/10] F:焦点边界 + 请求文件路径(第二实例 --settings-page 5) =="
# 先造一个待执行,再让设置窗抢焦点 → focus-out 复位;确认 F8 不再执行
xdotool type --delay 90 "tj"; sleep 0.5; xdotool key space
wait_buffer_tail "输入统计" 8
wait_log "menu-trigger hint: .*「输入统计」" 8
"$XIM_BIN" --settings-page 5 >>"$WORK/xim.stdout" 2>&1 \
    || fail "--settings-page 5 第二实例退出码非 0"
wait_settings_win
[[ -n "$SW" ]] || fail "第二实例 --settings-page 5 未唤起设置窗"
dump_win_tree "f-settings-page5"
focus_window "$SW" "f-settings"   # 真实焦点转移:focus-out 复位路径
shot "f1-settings-page5"
# 取消按钮(btnbox end:确定最右,取消在其左)
eval "$(xdotool getwindowgeometry --shell "$SW" || true)"
xdotool mousemove "$((X + WIDTH - 150))" "$((Y + HEIGHT - 31))" click 1
sleep 0.5
focus_client
FWD8="$(log_count 'forward keysym=0xffc5')"
xdotool key F8
wait_log_since "forward keysym=0xffc5" "$FWD8" 5
echo "PASS F:焦点切换复位待执行,第二实例页码请求生效,关闭经取消按钮"

echo "== [G] 截屏/截图提示带实际快捷键(UI 保存不改夹具 shot_hotkey) =="
focus_client
xdotool type --delay 90 "jp"; sleep 0.5; xdotool key space
wait_buffer_tail "截屏" 8
wait_log 'menu-trigger hint: .*「截屏」.*F8.*Ctrl+Shift+F9' 8
candwin_viewable || fail "截屏快捷键提示未显示"
shot "g1-shot-shortcut"
xdotool type --delay 90 "jt"; sleep 0.5; xdotool key space
wait_buffer_tail "截图" 8
wait_log 'menu-trigger hint: .*「截屏」.*F8.*Ctrl+Shift+F9' 8
echo "PASS G:截屏/截图提示包含确认 F8 和实际 Ctrl+Shift+F9"

echo "== 全部通过 =="
echo "[mt-e2e] 证据目录:$WORK(shots/ 截图,xim.stdout,client.log,buffer.txt,xim.log)"
