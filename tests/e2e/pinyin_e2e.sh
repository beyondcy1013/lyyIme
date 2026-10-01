#!/usr/bin/env bash
# lyyIme 拼音前缀候选(缺词兜底)端到端测试(独立 Xvfb,全自动)
#
# 结构同 prediction_e2e.sh:真库 $CARGO_TARGET_DIR/release/
# liblyyime_core.so,词库用 LYYIME_DATA_DIR 指向的极短夹具,
# HOME/XDG_* 全隔离;Xvfb -displayfd 自选空闲号(本脚本创建的资源
# 由本脚本清理,绝不碰任何预先存在的显示/锁/进程)。
#
# 夹具要点:pinyin_char.tsv 只有 jie/ping/pin/ni/hao 单字,
# base 词组文件只有「你好」;内嵌补充表(data/pinyin_supplement.tsv)
# 本身带「截屏 jie ping」,因此夹具 HOME 下的 blocked.tsv 屏蔽词面
# 「截屏」——词面屏蔽等价于该词不可用,由此稳定进入前缀候选路径
# (「截」consumed=3)。注:与截屏功能热键无关,这里只测文本上屏。
#
# 断言序列:
#   A. 键入 jieping:全程不上屏(Entry 保持空),候选窗可见
#      (前缀候选 截/接 在列,截图留证)。
#   B. 空格选「截」:Entry 精确为「截」,候选窗仍可见(预编辑=ping,
#      候选=屏,截图留证);xim.log 记 commit: 截。
#   C. 再空格选「屏」:Entry 精确为「截屏」,候选窗隐藏;
#      日志中 commit: 截 与 commit: 屏 是两次独立上屏,
#      绝不存在 commit: 截ping / commit: 截屏 这类整串提交。
#   D. 整词对照:nihao + 空格 → Entry 追加「你好」(整词候选
#      consumed=0,一次上屏),验证全词路径不受前缀改动影响。
#   E. 无损收尾:再清场后打 jieping 直接按逗号 → Entry 追加
#      「截ping，」(前缀文本+原始后缀+中文标点;逗号默认映射
#      全角「，」,不猜后缀字)。
#
# 用法:bash tests/e2e/pinyin_e2e.sh [--keep]
set -euo pipefail

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../xim" && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

# 隔离 D-Bus 会话:候选窗/通知全部走私有总线,绝不碰用户桌面会话。
if [[ -z "${LYYIME_PINYIN_E2E_DBUS:-}" ]] \
    && command -v dbus-run-session >/dev/null 2>&1; then
    exec env LYYIME_PINYIN_E2E_DBUS=1 dbus-run-session -- bash "$0" "$@"
fi

WORK="$(mktemp -d /tmp/lyyime-pinyin-e2e.XXXXXX)"
export HOME="$WORK/home"
export XDG_CONFIG_HOME="$WORK/xdg/config"
export XDG_DATA_HOME="$WORK/xdg/data"
export XDG_CACHE_HOME="$WORK/xdg/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

# 真库:release 构建产物;缺失直接失败(前缀消费语义只在真核内)
CORE_SO="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release/liblyyime_core.so"
[[ -f "$CORE_SO" ]] || { echo "FAIL: 真库不存在:$CORE_SO(先跑 cargo build --release)"; exit 1; }

# 词库夹具(缺「截屏」词组是前缀消费路径的确定性来源):
#   jie→截/接、ping→屏、pin→品;ni hao→你好 词组(整词对照)。
DICT="$WORK/dict"
mkdir -p "$DICT"
cat > "$DICT/pinyin_char.tsv" <<'DIC'
jie	截	6000
jie	接	3000
ping	屏	5000
pin	品	2000
ni	你	6000
hao	好	5000
DIC
cat > "$DICT/pinyin_phrase.tsv" <<'DIC'
你好	ni hao	9000
DIC
: > "$DICT/wubi.tsv"

# 词面屏蔽「截屏」(用户数据区,非词库):补充表存在时 base 缺词不再
# 是缺词状态,须用屏蔽模拟"该词不可用"逼出前缀兜底;隔离 HOME 下
# 的用户数据文件,绝不碰真实 $HOME。
mkdir -p "$HOME/.local/share/lyyime"
printf '截屏\n' > "$HOME/.local/share/lyyime/blocked.tsv"

# 配置夹具:关快速功能键/四码自动上屏/四码顶屏/词组提示/菜单触发
# (隔离被测面;前缀候选绝不触发四码路径,这里关掉只是消除干扰项)。
mkdir -p "$XDG_CONFIG_HOME/lyyime"
cat > "$XDG_CONFIG_HOME/lyyime/config.toml" <<'CFG'
quick_actions_enabled = false
commit_after_four = false
commit_first_at_four = false
commit_unique_four = false
commit_on_extra_after_four = false
phrase_hint = false
menu_trigger_enabled = false
next_word_prediction = false
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
        echo "[pinyin-e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

fail() { KEEP=1; echo "PINYIN-E2E FAIL: $*"; echo "(失败现场保留:$WORK)"; exit 1; }

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

# 截图(留证):Gdk pixbuf 直出 PNG;全缺记说明不失败
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
    if command -v import >/dev/null 2>&1 && \
       import -window root "$f.png" 2>/dev/null; then
        echo "[shot] $f.png"; return 0
    fi
    echo "[shot] $tag:无截图工具" | tee -a "$WORK/shots/NO-SHOTS.txt"
}

# 候选窗是否被映射为可见
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

wait_candwin() {
    local timeout="${1:-5}" i
    for ((i = 0; i < timeout * 20; i++)); do
        candwin_viewable >/dev/null && return 0
        sleep 0.05
    done
    return 1
}

wait_candwin_gone() {
    local timeout="${1:-5}" i
    for ((i = 0; i < timeout * 20; i++)); do
        candwin_viewable >/dev/null || return 0
        sleep 0.05
    done
    return 1
}

dump_win_tree() {
    local tag="$1" f="$WORK/wintree-$tag.txt"
    { echo "=== xwininfo -root -tree ==="; xwininfo -root -tree; } >"$f" 2>&1 || true
    echo "[diag] $f"
}

WID=""
focus_client() {
    [[ -n "$WID" ]] || \
        WID="$(xdotool search --onlyvisible --name '^lyyime-e2e-client$' | head -1 || true)"
    [[ -n "$WID" ]] || { dump_win_tree "client-missing"; fail "找不到客户端窗口"; }
    if ! xdotool windowactivate "$WID" 2>"$WORK/focus.err"; then
        xdotool windowfocus "$WID" 2>>"$WORK/focus.err" || {
            dump_win_tree "client"
            cat "$WORK/focus.err" 2>/dev/null
            fail "客户端窗口聚焦失败"
        }
    fi
    sleep 0.3
}

# 上屏 commit 行数与顺序取证(xim.log 每次 commit 一行 "commit: X")
commit_lines() { grep -n '^.*commit: ' "$XIM_LOG" 2>/dev/null || true; }

echo "== [1/8] 构建检查与显示 =="
[[ -x "$XIM_BIN" && -x "$CLIENT_BIN" ]] || fail "缺构建产物(先 make -C xim lyyime-xim test)"

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
echo "[pinyin-e2e] Xvfb 就绪 DISPLAY=$DISPLAY(pid=$XVFB_PID)"

echo "== [2/8] 启动 lyyime-xim(真库 $CORE_SO) =="
LYYIME_CORE_LIB="$CORE_SO" LYYIME_RES_DIR="$XIM_DIR/res" \
    LYYIME_DATA_DIR="$DICT" \
    "$XIM_BIN" >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"
wait_log "core 引擎已创建" 8

echo "== [3/8] 客户端接入 =="
# 客户端仅在 changed 时写盘,预置空文件代表初始空 Entry。
: > "$BUFFER"
"$CLIENT_BIN" "$BUFFER" 600 >"$CLIENT_LOG" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log "XIM client 已连接"
sleep 0.8
wait_log "获得焦点"
focus_client

echo "== [4/8] A:键入 jieping —— 全程不上屏,前缀候选在列 =="
xdotool type --delay 90 "jieping"; sleep 0.6
wait_buffer_exact "" 8
[[ -z "$(commit_lines)" ]] || fail "键入期意外上屏"
wait_candwin 5 || { shot "a-no-candwin"; fail "jieping 后候选窗未可见"; }
shot "a-jieping-cands"   # 候选窗应显示 截/接 + 预编辑 jieping
echo "PASS A:键入期零上屏,候选窗可见(前缀候选行)"

echo "== [5/8] B:空格选前缀「截」 —— 只上屏前缀,组合续 ping =="
xdotool key space
wait_buffer_exact "截" 8
wait_log "commit: 截" 8
# 组合未结束:候选窗仍可见,预编辑=ping、候选=屏(截图留证)。
wait_candwin 5 || { shot "b-no-candwin"; fail "选截后候选窗应仍可见(组合续 ping)"; }
shot "b-preedit-ping"
sleep 0.2
# 若旧行为(整串消费)仍在,此时缓冲已清,下一空格会把字面空格打进 Entry
# —— C 段的「截屏」精确断言与日志双 commit 行能确定性地抓出回归。
echo "PASS B:「截」单独上屏,后缀组合存活(候选窗仍开)"

echo "== [6/8] C:再空格选「屏」 —— Entry 精确 截屏 =="
xdotool key space
wait_buffer_exact "截屏" 8
wait_log "commit: 屏" 8
wait_candwin_gone 5 || { shot "c-candwin-stuck"; fail "整词收齐后候选窗未隐藏"; }
shot "c-done"
# 上屏历史取证:必须是 commit: 截 在前、commit: 屏 在后两行,
# 不得出现整串提交(截ping/截屏)或被吞的截。
grep -q "commit: 截ping" "$XIM_LOG" && fail "出现拼接上屏 commit: 截ping"
grep -q "commit: 截屏" "$XIM_LOG" && fail "出现整词上屏 commit: 截屏"
CJ="$(grep -c 'commit: 截$' "$XIM_LOG" || true)"
CP="$(grep -c 'commit: 屏$' "$XIM_LOG" || true)"
[[ "$CJ" == "1" && "$CP" == "1" ]] || { commit_lines; fail "commit 行数异常:截=$CJ 屏=$CP"; }
echo "PASS C:两次独立上屏 截→屏,Entry 精确「截屏」"

echo "== [7/8] D:整词对照 nihao —— 词组一次上屏 =="
xdotool type --delay 90 "nihao"; sleep 0.6
wait_buffer_exact "截屏" 8    # 键入期仍不上屏
wait_candwin 5 || fail "nihao 后候选窗未可见"
xdotool key space
wait_buffer_exact "截屏你好" 8
wait_log "commit: 你好" 8
wait_candwin_gone 5 || fail "你好上屏后候选窗未隐藏"
shot "d-nihao"
echo "PASS D:整词候选 consumed=0 一次上屏「你好」"

echo "== [8/8] E:无损收尾 —— jieping 直接逗号 = 截ping， =="
xdotool type --delay 90 "jieping"; sleep 0.6
xdotool key comma
# 中文标点默认开:逗号映射为全角「，」;wait_buffer_exact 已逐字节校验。
wait_buffer_exact "截屏你好截ping，" 8
wait_candwin_gone 5 || fail "标点收尾后候选窗未隐藏"
shot "e-punct"
echo "PASS E:标点收尾无损拼接 截ping，(未猜后缀字)"

echo "PINYIN-E2E PASS:全部断言通过(工作目录 $WORK)"
