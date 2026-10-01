#!/usr/bin/env bash
# lyyIme 上屏后联想端到端测试(独立 Xvfb,全自动)
#
# 结构同 menu_trigger_e2e.sh:用**真库** $CARGO_TARGET_DIR/release/
# liblyyime_core.so,词库用 LYYIME_DATA_DIR 指向的极短夹具,
# HOME/XDG_* 全隔离;显示经 Xvfb -displayfd 自选空闲号(本脚本创建的
# 资源由本脚本清理,绝不碰任何预先存在的显示/锁/进程)。
#
# 断言序列:
#   0. 前置:配置不写 next_word_prediction → 默认关,上屏「你」不出
#      联想行;设置页「输入」勾选开启(日志 pred=1 + 落盘 true),
#      证明真实 opt-in。
#   A. wq+空格 上屏「你」→ 候选条出联想行(可见 + 截图);
#      数字 1 续上屏尾巴「好」(只上屏尾巴,不重上屏前缀)。
#   B. 再上屏「你」→ 空格续选首尾巴「好」(空格路径)。
#   C. 联想行下普通字母开新组合:键入 x(无命中)→ 空格上屏原串 x,
#      证明空格没有吃掉陈旧联想;Esc 吞键撤联想,其后空格直通应用
#      (缓冲尾留空格,不上屏旧尾巴)。
#   D. 标点硬边界:联想态逗号上屏「，」并清上下文;其后数字 1 直通成
#      字面 "1";鼠标点选联想行 → 上屏对应尾巴(好/们按命中断言)。
#   E. 设置页「输入」关掉「输入后联想」复选框,确定保存(日志 pred=0 +
#      配置文件落盘 false)。
#   F. 关闭后同输入不再出联想行,空格直通。
#   G. 中文标点:默认 , . ? → ，。？
#   H. Ctrl+. 运行时切英文标点(,.? 原样直通,快捷键不产生 '.' 上屏,
#      切换前后缓冲不变),再按一次翻回中文标点。
#   I. 组合中 Ctrl+. 不清缓冲/候选:打 wq 后切换,空格仍上屏「你」,
#      英文标点态逗号直通;再翻回中文。
#   J. 设置页「常规」取消「默认使用中文标点」→ 保存即生效(日志
#      punct=0 + chinese_punct = false 落盘),逗号原样直通;勾回保存
#      → punct=1,中文标点恢复。
#
# 用法:bash tests/e2e/prediction_e2e.sh [--keep]
set -euo pipefail

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../xim" && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

# 隔离 D-Bus 会话:AT-SPI/设置窗/通知全部走私有总线,绝不碰用户桌面
# 会话。自重启要放在 HOME/XDG 覆盖之前(私有 HOME 不污染 exec 链)。
if [[ -z "${LYYIME_PRED_E2E_DBUS:-}" ]] \
    && command -v dbus-run-session >/dev/null 2>&1; then
    exec env LYYIME_PRED_E2E_DBUS=1 dbus-run-session -- bash "$0" "$@"
fi

WORK="$(mktemp -d /tmp/lyyime-pred-e2e.XXXXXX)"
# 隔离运行环境:配置/日志/用户词库全部落到 WORK 下
export HOME="$WORK/home"
export XDG_CONFIG_HOME="$WORK/xdg/config"
export XDG_DATA_HOME="$WORK/xdg/data"
export XDG_CACHE_HOME="$WORK/xdg/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

# 真库:release 构建产物;缺失直接失败(联想语义只在真核内)
CORE_SO="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release/liblyyime_core.so"
[[ -f "$CORE_SO" ]] || { echo "FAIL: 真库不存在:$CORE_SO(先跑 cargo build --release)"; exit 1; }

# 词库夹具:wq=你 vbg=好;词组 你好/你们;推荐词 你好(跨源取最大频次)。
# 联想索引由 core 从词库聚合:上屏「你」→ 联想行 [好(5000) 们(900)]。
DICT="$WORK/dict"
mkdir -p "$DICT"
cat > "$DICT/wubi.tsv" <<'DIC'
wq	你	100
vbg	好	100
DIC
cat > "$DICT/pinyin_phrase.tsv" <<'DIC'
你好	ni hao	1000
你们	ni men	900
DIC
cat > "$DICT/suggestion.tsv" <<'DIC'
你好	5000
DIC

# 配置夹具:关快速功能键/四码自动上屏/词组提示/菜单触发(隔离被测面);
# next_word_prediction 不写 —— 验证默认关闭。
mkdir -p "$XDG_CONFIG_HOME/lyyime"
cat > "$XDG_CONFIG_HOME/lyyime/config.toml" <<'CFG'
quick_actions_enabled = false
commit_after_four = false
commit_first_at_four = false
commit_unique_four = false
phrase_hint = false
menu_trigger_enabled = false
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
        echo "[pred-e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

fail() { KEEP=1; echo "PRED-E2E FAIL: $*"; echo "(失败现场保留:$WORK)"; exit 1; }

wait_log() {
    local pat="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        grep -q "$pat" "$XIM_LOG" 2>/dev/null && return 0
        sleep 0.05
    done
    echo "---- xim.log ----"; cat "$XIM_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 超时"
}

# 精确断言:缓冲文件内容必须与期望串逐字节一致(Entry 无换行)
wait_buffer_exact() {
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ -f "$BUFFER" ]] && [[ "$(cat "$BUFFER")" == "$want" ]] && return 0
        sleep 0.05
    done
    echo "---- 当前缓冲 ----"; cat "$BUFFER" 2>/dev/null || echo "(空)"
    echo "---- xim.log 尾部 ----"; tail -30 "$XIM_LOG" 2>/dev/null || true
    fail "缓冲精确断言失败:期望 [$want]"
}

# 截图(留证):Gdk pixbuf 直出 PNG 首选;其次 xwd→PNG;全缺记说明不失败
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
    fi
    if command -v import >/dev/null 2>&1 && \
       import -window root "$f.png" 2>/dev/null; then
        echo "[shot] $f.png"; return 0
    fi
    echo "[shot] $tag:无截图工具" | tee -a "$WORK/shots/NO-SHOTS.txt"
}

# 候选窗(联想行载体)是否被映射为可见
candwin_viewable() {
    local ids w
    ids="$(xwininfo -root -children 2>/dev/null | grep '"lyyime-xim"' \
        | awk '{print $1}' || true)"
    while IFS= read -r w; do
        [[ -n "$w" ]] || continue
        if xwininfo -id "$w" 2>/dev/null | grep -q "Map State: IsViewable"; then
            echo "$w"
            return 0
        fi
    done <<<"$ids"
    return 1
}

# 轮询等候选窗可见(联想行渲染是异步 GTK 主循环)
wait_candwin() {
    local timeout="${1:-5}" i
    for ((i = 0; i < timeout * 20; i++)); do
        candwin_viewable >/dev/null && return 0
        sleep 0.05
    done
    return 1
}

# 等候选窗隐藏(撤联想/清空候选)
wait_candwin_gone() {
    local timeout="${1:-5}" i
    for ((i = 0; i < timeout * 20; i++)); do
        candwin_viewable >/dev/null || return 0
        sleep 0.05
    done
    return 1
}

# 窗口树取证:聚焦/找窗失败时导出 xwininfo 全树
dump_win_tree() {
    local tag="$1" f="$WORK/wintree-$tag.txt"
    {
        echo "=== xwininfo -root -tree ==="
        xwininfo -root -tree
    } >"$f" 2>&1 || true
    echo "[diag] $f"
}

focus_window() {
    local w="$1" tag="$2"
    if ! xdotool windowactivate "$w" 2>"$WORK/focus-$tag.err"; then
        xdotool windowfocus "$w" 2>>"$WORK/focus-$tag.err" || {
            dump_win_tree "$tag"
            cat "$WORK/focus-$tag.err" 2>/dev/null
            fail "窗口聚焦失败($tag):窗口未映射?"
        }
    fi
    sleep 0.3
}

wait_settings_win() {
    local i out w
    for ((i = 0; i < 120; i++)); do
        out="$(xdotool search --onlyvisible --name '输入法设置' 2>/dev/null || true)"
        while IFS= read -r w; do
            [[ -n "$w" ]] || continue
            if xwininfo -id "$w" 2>/dev/null | grep -q "Map State: IsViewable"; then
                SW="$w"
                return 0
            fi
        done <<<"$out"
        sleep 0.1
    done
    dump_win_tree "settings-timeout"
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

# AT-SPI 复选框按压:只在标题含「设置」的窗口内找 name 含子串的
# check box 并 press(toggle)。pyatspi 优先,无则 gi.repository.Atspi;
# 两条路径都失败 → 非 0 返回,调用方走像素兜底(兜底必须带后置断言,
# 点歪会 loud fail)。私有 D-Bus 会话内 at-spi-bus-launcher 可拉起。
atspi_press_check() {
    python3 - "$1" <<'PY'
import sys

want = sys.argv[1]


def press_pyatspi():
    import pyatspi

    def walk(n, d):
        if d > 14:
            return None
        try:
            if n.getRoleName() == "check box" and want in (n.name or ""):
                return n
        except Exception:
            return None
        try:
            cnt = n.childCount
        except Exception:
            return None
        for i in range(cnt):
            try:
                r = walk(n.getChildAtIndex(i), d + 1)
            except Exception:
                continue
            if r is not None:
                return r
        return None

    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        for j in range(app.childCount):
            w = app.getChildAtIndex(j)
            if "设置" in (w.name or ""):
                node = walk(w, 0)
                # doAction 返回 bool:False/异常都视为未点中,走 GI/像素兜底
                if node is not None and node.doAction(0):
                    return True
    return False


def press_gi():
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    def walk(n, d):
        if d > 14 or n is None:
            return None
        try:
            if n.get_role_name() == "check box" and want in (n.get_name() or ""):
                return n
        except Exception:
            return None
        try:
            cnt = n.get_child_count()
        except Exception:
            return None
        for i in range(cnt):
            try:
                r = walk(n.get_child_at_index(i), d + 1)
            except Exception:
                continue
            if r is not None:
                return r
        return None

    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is None:
            continue
        for j in range(app.get_child_count()):
            w = app.get_child_at_index(j)
            if w is not None and "设置" in (w.get_name() or ""):
                node = walk(w, 0)
                if node is not None and node.do_action(0):
                    return True
    return False


# pyatspi 未找到/未点中(False)也要继续试 GI Atspi,再不行才回退。
try:
    if press_pyatspi():
        sys.exit(0)
except Exception as e:
    print("pyatspi 路径失败:%s" % e, file=sys.stderr)
try:
    if press_gi():
        sys.exit(0)
except Exception as e:
    print("gi.Atspi 路径失败:%s" % e, file=sys.stderr)
sys.exit(1)
PY
}

echo "== [1/14] 构建检查与显示 =="
[[ -x "$XIM_BIN" && -x "$CLIENT_BIN" ]] || fail "缺构建产物(先 make -C xim lyyime-xim test)"

# Xvfb -displayfd:自选空闲显示号
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
XSOCK="/tmp/.X11-unix/X${DISP}"
for _ in $(seq 1 50); do [[ -S "$XSOCK" ]] && break; sleep 0.1; done
echo "[pred-e2e] Xvfb 就绪 DISPLAY=$DISPLAY(pid=$XVFB_PID)"

echo "== [2/14] 启动 lyyime-xim(真库 $CORE_SO) =="
# GTK_MODULES=atk-bridge:让设置窗在私有会话里暴露 AT-SPI 树
# (org.a11y.Bus 经私有 session bus 激活);模块缺载仅告警,不影响本测。
GTK_MODULES="atk-bridge" \
LYYIME_CORE_LIB="$CORE_SO" LYYIME_RES_DIR="$XIM_DIR/res" \
    LYYIME_DATA_DIR="$DICT" \
    "$XIM_BIN" >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"
wait_log "core 引擎已创建" 8
# 缺符号告警不得出现(真库必有联想/标点切换两组可选符号)
if grep -q "无上屏后联想符号" "$XIM_LOG"; then
    fail "真库竟缺 lyyime_set_next_word_prediction 符号"
fi
if grep -q "无中英文标点切换符号" "$XIM_LOG"; then
    fail "真库竟缺 lyyime_*_chinese_punctuation 符号"
fi

echo "== [3/14] 客户端接入 =="
"$CLIENT_BIN" "$BUFFER" 600 >"$CLIENT_LOG" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log "XIM client 已连接"
sleep 0.8
wait_log "获得焦点"
focus_client

echo "== [4/14] 前置:联想默认关闭 → 设置页显式开启 =="
# 默认关:wq+空格 上屏「你」后不得出联想行;再按空格直通成尾随空格。
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你" 8
sleep 0.5
candwin_viewable >/dev/null && { shot "predoff-candwin"; fail "默认关闭仍出现联想候选"; }
xdotool key space
wait_buffer_exact "你 " 8
shot "default-off"
echo "PASS 0a:联想默认关闭(无联想行,空格直通)"

# 设置页「输入」勾选「输入后联想」验证 opt-in:AT-SPI 优先,像素兜底
# 与 E 段同一实测坐标 (X+27, Y+283);点歪由 pred=1+落盘断言 loud fail。
GTK_MODULES="atk-bridge" "$XIM_BIN" --settings-page 1 \
    >>"$WORK/xim.stdout" 2>&1 || fail "--settings-page 1 第二实例退出码非 0"
wait_settings_win
focus_window "$SW" "settings-on"
sleep 0.2
shot "predoff-settings-unchecked"
eval "$(xdotool getwindowgeometry --shell "$SW")"
TOGGLED=""
atspi_press_check "联想" && TOGGLED=atspi || true
if [[ -z "$TOGGLED" ]]; then
    echo "[pred-on] 无 AT-SPI,走几何兜底点击 (X+27, Y+283)"
    xdotool mousemove "$((X + 27))" "$((Y + 283))" click 1
    sleep 0.3
fi
shot "predoff-checked"
xdotool mousemove "$((X + WIDTH - 57))" "$((Y + HEIGHT - 31))" click 1
wait_log "设置已保存并生效.*pred=1" 8
grep -q "next_word_prediction = true" "$XDG_CONFIG_HOME/lyyime/config.toml" \
    || fail "开启保存后 config.toml 未落 next_word_prediction = true"
sleep 0.6
focus_client
# 清空上屏缓冲「你 」:BackSpace 在空组词缓冲下直通应用,每键删一字。
# 不用 ctrl+a:Ctrl+字母会经 XIM 进 core 被当输入字符吞掉
# (xim.log 实证 Ctrl+period 可达服务端),select-all 不会生效。
xdotool key BackSpace BackSpace
wait_buffer_exact "" 8
echo "PASS 0b:设置页显式开启联想(pred=1 + 配置落盘 true),缓冲已清零"

echo "== [5/14] A:上屏「你」→ 联想行可见 + 数字续选尾巴 =="
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你" 8
wait_log "commit: 你" 8
wait_candwin 5 || { shot "a0-no-candwin"; fail "上屏后联想候选窗未可见"; }
shot "a1-pred-visible"
# 数字 1 选首个联想尾巴:夹具 你好(5000) → 尾巴「好」;只上屏尾巴
xdotool key 1
wait_buffer_exact "你好" 8
echo "PASS A:联想行可见(截图),数字续选只上屏尾巴「好」"

echo "== [6/14] B:空格续选首尾巴 =="
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你好你" 8
wait_candwin 5 || fail "第二次上屏后联想行未出现"
xdotool key space        # 空格 = 选联想首候选「好」
wait_buffer_exact "你好你好" 8
echo "PASS B:空格续选联想首尾巴「好」"

echo "== [7/14] C:字母撤联想行开新组合 / Esc 吞键取消 =="
# 字母路径:联想行下打无命中字母 x → 空格上屏原串,不吃陈旧联想
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你好你好你" 8
wait_candwin 5 || fail "联想行未出现(C 前置)"
xdotool type --delay 90 "x"; sleep 0.4   # 撤联想,x 进组合缓冲
xdotool key space                       # 无候选 → 上屏原串 x
wait_buffer_exact "你好你好你x" 8
echo "PASS C1:字母开新组合,空格上屏 x 而非陈旧尾巴"

# Esc 路径:联想态 Esc 吞键撤行;其后空格直通(Entry 落字面空格)
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你好你好你x你" 8
wait_candwin 5 || fail "联想行未出现(Esc 前置)"
xdotool key Escape
wait_candwin_gone 5 || fail "Esc 后联想候选窗未隐藏"
xdotool key space        # 无联想 → 直通,Entry 落空格
wait_buffer_exact "你好你好你x你 " 8
echo "PASS C2:Esc 撤联想后空格直通(尾部空格为证)"

echo "== [8/14] D:标点硬边界 + 鼠标点选联想行 =="
# 标点:联想态逗号上屏「，」、上下文清;其后数字 1 直通(不落旧尾巴)
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你好你好你x你 你" 8
wait_candwin 5 || fail "联想行未出现(标点前置)"
xdotool key comma
wait_buffer_exact "你好你好你x你 你，" 8
wait_log "commit: ，" 8
xdotool key 1            # 空缓冲无候选 → 数字直通
wait_buffer_exact "你好你好你x你 你，1" 8
echo "PASS D1:标点撤联想+清上下文,其后数字直通字面 1"

# 鼠标点选:定位可见候选窗,按几何测算逐 y 点行区,
# 命中任一行即上屏对应尾巴(好/们;点落空区无副作用可重试)
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "你好你好你x你 你，1你" 8
CW=""
for _ in $(seq 1 60); do
    CW="$(candwin_viewable || true)"; [[ -n "$CW" ]] && break; sleep 0.1
done
[[ -n "$CW" ]] || fail "鼠标点选前置:联想候选窗不可见"
shot "d2-pred-before-click"
eval "$(xdotool getwindowgeometry --shell "$CW")"
# 行区 = 窗口高减顶部 header(约 26px)与底部边距;2 行均分。
# 逐 y 点试:首行中心起,每次 +8px,越出窗高停;点中即出 commit。
CLICKED=""
# 行区扫描:预编辑为空时 header 可能塌成 ~0 高,直接从窗顶 +8 扫到
# 窗底 -4;点中任一行即 commit(行区外/行间空白返回 idx<0 无副作用)。
for ((cy = Y + 8; cy <= Y + HEIGHT - 4; cy += 6)); do
    xdotool mousemove "$((X + WIDTH / 2))" "$cy" click 1
    sleep 0.3
    CUR="$(cat "$BUFFER" 2>/dev/null || true)"
    case "$CUR" in
        "你好你好你x你 你，1你好") CLICKED="好"; break ;;
        "你好你好你x你 你，1你们") CLICKED="们"; break ;;
    esac
    # 没点中也可能点到了客户端窗;确认焦点仍在客户端再继续
    focus_client
done
[[ -n "$CLICKED" ]] || { shot "d2-click-miss"; dump_win_tree "d2"; \
    fail "鼠标点选联想行未产生任何尾巴上屏(窗口几何 X=$X Y=$Y ${WIDTH}x$HEIGHT)"; }
shot "d3-pred-clicked"
echo "PASS D2:鼠标点选联想行上屏尾巴「$CLICKED」"

echo "== [9/14] E:设置 UI 关掉「输入后联想」→ 保存即生效 =="
GTK_MODULES="atk-bridge" "$XIM_BIN" --settings-page 1 \
    >>"$WORK/xim.stdout" 2>&1 \
    || fail "--settings-page 1 第二实例退出码非 0"
wait_settings_win
focus_window "$SW" "settings-e"
sleep 0.2
shot "e1-settings-input-page"
eval "$(xdotool getwindowgeometry --shell "$SW")"
TOGGLED=""
# 首选 AT-SPI 按名称找复选框并 press;无 AT-SPI 走几何标定行坐标
atspi_press_check "联想" && TOGGLED=atspi || true
if [[ -z "$TOGGLED" ]]; then
    # 几何兜底(实测,非估算):默认 GTK 布局+825x544 设置窗下,
    # 输入页联想复选框中心实测为窗口相对 (27, 283)
    # —— 来自 e1-settings-input-page.png 失败现场截图标定。
    ROWY=$((Y + 283))
    echo "[e2] 无 pyatspi,走几何兜底点击 (X+27, $ROWY)"
    xdotool mousemove "$((X + 27))" "$ROWY" click 1
    sleep 0.3
fi
shot "e2-pred-unchecked"
# 确定保存(右下角按钮,菜单 E2E 实测中心 X+W-57, Y+H-31)
xdotool mousemove "$((X + WIDTH - 57))" "$((Y + HEIGHT - 31))" click 1
wait_log "设置已保存并生效.*pred=0" 8
grep -q "next_word_prediction = false" \
    "$XDG_CONFIG_HOME/lyyime/config.toml" \
    || fail "保存后 config.toml 未落 next_word_prediction = false"
sleep 0.8
focus_client
echo "PASS E1:UI 关联想保存即生效(日志 pred=0 + 配置落盘)"

echo "== [10/14] F:关闭后同输入不再出联想 =="
BASE="$(cat "$BUFFER" 2>/dev/null || true)"
xdotool type --delay 90 "wq"; sleep 0.5
xdotool key space
wait_buffer_exact "${BASE}你" 8
sleep 0.5
candwin_viewable >/dev/null && { shot "f1-unexpected-candwin"; \
    fail "联想已关仍出现候选行"; }
# 空格直通(若有联想会被吃掉选尾巴):Entry 落字面空格
xdotool key space
wait_buffer_exact "${BASE}你 " 8
shot "f3-no-pred"
echo "PASS F:联想关闭后同输入无联想行,空格直通"

# ======================================================================
# 中英文标点:默认中文标点 / Ctrl+. 运行时切换 / 设置窗保存默认
# ======================================================================

echo "== [11/14] G:默认中文标点(, . ? → ，。？) =="
# 期望串用变量定义,避免全角/半角字面量在源码里混淆:
# CN_PUNCT = U+FF0C(,) U+3002(。) U+FF1F(?)
CN_PUNCT=$'\uFF0C\u3002\uFF1F'
CN_COMMA=$'\uFF0C'
BASE="$(cat "$BUFFER" 2>/dev/null || true)"
xdotool key comma period question
wait_buffer_exact "${BASE}${CN_PUNCT}" 8
wait_log "commit: ？" 8
shot "g1-punct-cn"
echo "PASS G:默认中文标点映射 , . ? → ，。？"

echo "== [12/14] H:Ctrl+. 切英文标点 → ASCII;再按翻回 =="
# 先取基线再按快捷键:若 Ctrl+. 误上屏 '.',wait_buffer_exact 原地断言
# 即挂,不会被并入 BASE 掩盖。
BASE="$(cat "$BUFFER")"
xdotool key ctrl+period   # Control 预热放行 + period 吞键翻转
sleep 0.4
wait_buffer_exact "$BASE" 8
xdotool key comma period question
wait_buffer_exact "${BASE},.?" 8
BASE="$(cat "$BUFFER")"
xdotool key ctrl+period   # 翻回中文标点
sleep 0.4
wait_buffer_exact "$BASE" 8
xdotool key comma period question
wait_buffer_exact "${BASE}${CN_PUNCT}" 8
shot "h1-punct-toggle"
echo "PASS H:Ctrl+. 切换英文标点并翻回(快捷键本身不上屏 '.')"

echo "== [13/14] I:组合中 Ctrl+. 不清缓冲/候选 =="
BASE="$(cat "$BUFFER")"
xdotool type --delay 90 "wq"; sleep 0.5  # 组合 wq → 候选「你」
xdotool key ctrl+period                   # 组合中静默切换(候选行在 → 无提示)
sleep 0.4
xdotool key space                         # 组合保留 → 空格仍上屏「你」
wait_buffer_exact "${BASE}你" 8
xdotool key comma                         # 英文标点态逗号直通
wait_buffer_exact "${BASE}你," 8
xdotool key ctrl+period                   # 翻回中文标点
sleep 0.4
xdotool key comma
wait_buffer_exact "${BASE}你,${CN_COMMA}" 8
echo "PASS I:组合中切换保留缓冲/候选,英文标点逗号原样直通"

echo "== [14/14] J:设置「常规」中文标点默认 → 保存即生效 =="
GTK_MODULES="atk-bridge" "$XIM_BIN" --settings-page 0 \
    >>"$WORK/xim.stdout" 2>&1 || fail "--settings-page 0 第二实例退出码非 0"
wait_settings_win
focus_window "$SW" "settings-j"
sleep 0.2
shot "j1-settings-general-page"
eval "$(xdotool getwindowgeometry --shell "$SW")"
TOGGLED=""
atspi_press_check "中文标点" && TOGGLED=atspi || true
if [[ -z "$TOGGLED" ]]; then
    # 几何兜底(估算):常规页 grid 行2 = 标点复选框,中心约窗口相对
    # (27,150);点歪由下方 punct=0 日志 + 配置落盘断言 loud fail。
    echo "[j] 无 AT-SPI,走几何兜底点击 (X+27, Y+150)"
    xdotool mousemove "$((X + 27))" "$((Y + 150))" click 1
    sleep 0.3
fi
shot "j2-punct-unchecked"
xdotool mousemove "$((X + WIDTH - 57))" "$((Y + HEIGHT - 31))" click 1
wait_log "设置已保存并生效.*punct=0" 8
grep -q "chinese_punct = false" "$XDG_CONFIG_HOME/lyyime/config.toml" \
    || fail "保存后 config.toml 未落 chinese_punct = false"
grep -q "^cn_punct" "$XDG_CONFIG_HOME/lyyime/config.toml" \
    && fail "保存不应写出旧别名 cn_punct 行" || true
sleep 0.6
focus_client
BASE="$(cat "$BUFFER")"
xdotool key comma
wait_buffer_exact "${BASE}," 8
echo "PASS J1:取消「默认中文标点」保存即生效(逗号原样直通)"

# 勾回保存 → 中文标点恢复(同实例,不重启)
GTK_MODULES="atk-bridge" "$XIM_BIN" --settings-page 0 \
    >>"$WORK/xim.stdout" 2>&1 || fail "二次 --settings-page 0 退出码非 0"
wait_settings_win
focus_window "$SW" "settings-j2"
sleep 0.2
eval "$(xdotool getwindowgeometry --shell "$SW")"
TOGGLED=""
atspi_press_check "中文标点" && TOGGLED=atspi || true
if [[ -z "$TOGGLED" ]]; then
    xdotool mousemove "$((X + 27))" "$((Y + 150))" click 1
    sleep 0.3
fi
shot "j3-punct-checked"
xdotool mousemove "$((X + WIDTH - 57))" "$((Y + HEIGHT - 31))" click 1
wait_log "设置已保存并生效.*punct=1" 8
grep -q "chinese_punct = true" "$XDG_CONFIG_HOME/lyyime/config.toml" \
    || fail "二次保存后 config.toml 未落 chinese_punct = true"
sleep 0.6
focus_client
BASE="$(cat "$BUFFER")"
xdotool key comma
wait_buffer_exact "${BASE}${CN_COMMA}" 8
shot "j4-punct-cn-restored"
echo "PASS J2:勾回保存后中文标点恢复(同实例生效)"

echo "PRED-E2E ALL PASS(截图:$WORK/shots)"
