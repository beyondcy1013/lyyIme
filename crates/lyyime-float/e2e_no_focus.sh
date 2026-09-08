#!/bin/bash
# lyyime-float 不抢焦点模式(keep_target_focus)e2e — 隔离 Xvfb(默认 :98)+ xfwm4
# 验收(用户可见行为): 开启后打字/选词/上屏全程, 活动窗口始终是目标窗口
# (目标文本框光标保持), 悬浮窗绝不夺取焦点; 键盘经 GdkSeat 抓取送达悬浮窗;
# 上屏注入落到仍持有焦点的目标窗口; 上屏解抓后重抓, 可连续输入。
# 判别力: 抓取失效 → 编码字母漏进目标(内容多出 'wq');重抓失效 → 第二词丢失。
# 全程临时 HOME, 不读写用户真实配置; 清理只 kill 本脚本 spawn 的 PID。
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
BIN=${BIN:-/data/cargo-target/local/lyyIme/debug/lyyime-float}
DISP=${E2E_DISP:-:93}
THOME=$(mktemp -d /tmp/lyyime-float-nofocus.XXXXXX)
LOG=$THOME/e2e.log

cleanup() {
  for pid in "${PAD_PID:-}" "${FLOAT_PID:-}" "${WM_PID:-}" "${XVFB_PID:-}"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  rm -rf "$THOME"
}
trap cleanup EXIT

fail() { echo "NOFOCUS-FAIL: $*"; [ -f "$LOG" ] && sed -n '1,40p' "$LOG"; exit 1; }

[ -x "$BIN" ] || fail "未找到 $BIN, 先跑 scripts/build.sh"
for t in xdotool xprop xfwm4 Xvfb python3; do
  command -v "$t" >/dev/null || fail "缺少 $t"
done

Xvfb $DISP -screen 0 1280x800x24 -nolisten tcp & XVFB_PID=$!
sleep 1.5
xfwm4 --display=$DISP >/dev/null 2>&1 & WM_PID=$!
sleep 2

mkdir -p "$THOME/.config/lyyime"
echo '{"keep_target_focus": true}' > "$THOME/.config/lyyime/config.json"

export DISPLAY=$DISP
"$BIN" --pad >$LOG 2>&1 & PAD_PID=$!
sleep 2
HOME=$THOME "$BIN" >>$LOG 2>&1 & FLOAT_PID=$!
sleep 5   # 等码表异步加载完成, 否则空格无候选可上

P=$(xdotool search --name '输入板' | head -1)
L=$(xdotool search --name 'lyyIme 悬浮输入法' | head -1)
echo "pad=$P float=$L"
[ -n "$P" ] && [ -n "$L" ] || { cat $LOG; fail "窗口未出现"; }

# ① WM_HINTS: 悬浮窗不参与焦点(input=False), 点击/映射不夺焦
HINTS=$(xprop -id "$L" WM_HINTS 2>/dev/null || true)
echo "$HINTS" | grep -q 'accepts input or input focus: False' \
  || fail "WM_HINTS 应为不接受焦点: $HINTS"

# ② 激活目标(打字板)后打编码: 活动窗口必须始终是目标
xdotool windowactivate --sync "$P"; sleep 0.6
xdotool type --delay 60 'wq'; sleep 0.8
ACT=$(xdotool getactivewindow)
[ "$ACT" = "$P" ] || fail "打编码时活动窗口 $ACT ≠ 目标 $P(焦点被抢)"

# ③ 空格上屏: 悬浮窗收键→commit→注入到仍持焦点的目标
xdotool key --delay 60 space; sleep 1.5
ACT=$(xdotool getactivewindow)
[ "$ACT" = "$P" ] || fail "上屏后活动窗口 $ACT ≠ 目标 $P"

# ④ 连续第二个词: 证明上屏解抓后重抓成功
xdotool type --delay 60 'wq'; sleep 0.5
xdotool key --delay 60 space; sleep 1.5
ACT=$(xdotool getactivewindow)
[ "$ACT" = "$P" ] || fail "第二词后活动窗口 $ACT ≠ 目标 $P"

# ⑤ 空缓冲 Ctrl+Z/Ctrl+Y 直通(编码框无可撤销历史): 键经直通落到仍持焦点
#    的目标, 撤销/重做的是目标里刚上屏的文本。目标打字板自身撤销栈
#    ['','你','你你'] → 撤两次到空 → 重做一次回 '你'
xdotool key --delay 60 ctrl+z; sleep 1.2
xdotool key --delay 60 ctrl+z; sleep 1.2
xdotool key --delay 60 ctrl+y; sleep 1.2
ACT=$(xdotool getactivewindow)
[ "$ACT" = "$P" ] || fail "直通撤销后活动窗口 $ACT ≠ 目标 $P"

# ⑥ 杀悬浮窗(X server 自动释放 grab)后读目标内容断言
kill "$FLOAT_PID" 2>/dev/null; wait "$FLOAT_PID" 2>/dev/null; FLOAT_PID=""
sleep 1
CONTENT=$(xdotool windowactivate "$P" >/dev/null 2>&1; sleep 0.5; \
  xdotool key ctrl+a; xdotool key ctrl+c; sleep 0.6; \
  python3 -c "
import gi; gi.require_version('Gtk','3.0')
from gi.repository import Gtk, Gdk
Gtk.init()
print(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text() or '')")
echo "pad content: [$CONTENT]"
[ "$CONTENT" = "你" ] || fail "直通撤销/重做后目标内容应为『你』, 实际: [$CONTENT]"

echo "NOFOCUS-OK"
