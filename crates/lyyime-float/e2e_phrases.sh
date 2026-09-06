#!/bin/bash
# lyyime-float(Rust 悬浮窗)自定义短语 e2e — 隔离 Xvfb 虚拟屏(默认 :97)
# 不碰真屏(:11)焦点/剪贴板; 全程临时 HOME, 不读写用户真实配置;
# 清理只 kill 本脚本 spawn 的 PID。
#   ① 词典候选不受影响: wq → 你
#   ② 精确码短语排候选首位: wqvb 定义了短语 → 上屏短语而非词典"你好"
#   ③ 长短语(>40字)直输模式下自动改粘贴并完整上屏: csph
#   ④ 短语管理对话框冒烟(--smoke-phrases): 新增/改码走真实回调链并落盘
#   ⑤ 动态日期触发: jjad(五笔"日期")居首, 数字2选中动态今天日期上屏
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
BIN=${BIN:-/data/cargo-target/local/lyyIme/debug/lyyime-float}
DISP=${E2E_DISP:-:97}
THOME=$(mktemp -d /tmp/lyyime-float-e2e.XXXXXX)
LOG=$THOME/e2e.log
FAIL=0

cleanup() {
  for pid in "${PAD_PID:-}" "${FLOAT_PID:-}" "${WM_PID:-}" "${XVFB_PID:-}"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  rm -rf "$THOME"
}
trap cleanup EXIT

[ -x "$BIN" ] || { echo "未找到 $BIN, 先跑 scripts/build.sh"; exit 1; }
Xvfb $DISP -screen 0 1280x800x24 -nolisten tcp & XVFB_PID=$!
sleep 1.5
xfwm4 --display=$DISP >/dev/null 2>&1 & WM_PID=$!
sleep 2

mkdir -p "$THOME/.config/lyyime"
python3 - "$THOME/.config/lyyime/phrase.json" <<'PY'
import json, sys
long_text = '自定义长短语测试:' + '很长的内容' * 12 + '【结束】'  # >40 字
json.dump({'csph': [long_text], 'wqvb': ['自定义抢占你好']},
          open(sys.argv[1], 'w'), ensure_ascii=False)
PY

export DISPLAY=$DISP
"$BIN" --pad >$LOG 2>&1 & PAD_PID=$!
sleep 2
HOME=$THOME "$BIN" >>$LOG 2>&1 & FLOAT_PID=$!
sleep 4

P=$(xdotool search --name '输入板' | head -1)
L=$(xdotool search --name 'lyyIme 悬浮输入法' | head -1)
echo "pad=$P float=$L"
if [ -z "$P" ] || [ -z "$L" ]; then echo '窗口未出现'; cat $LOG; exit 1; fi

pad_content() {
  xdotool windowactivate $P; sleep 0.5
  xdotool key ctrl+a; xdotool key ctrl+c; sleep 0.6
  python3 -c "
import gi; gi.require_version('Gtk','3.0')
from gi.repository import Gtk, Gdk
Gtk.init()
print(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text() or '')"
}
clear_pad() { xdotool windowactivate $P; sleep 0.4; xdotool key ctrl+a; xdotool key Delete; sleep 0.2; }
type_in_float() { xdotool windowactivate $L; sleep 0.6; xdotool type --delay 80 "$1"; sleep 0.5; xdotool key space; sleep 3; }

# ① 词典不受影响
clear_pad; type_in_float 'wq'
C=$(pad_content); echo "① wq => $C"
echo "$C" | grep -q '^你' && echo '① PASS 词典正常' || { echo '① FAIL'; FAIL=1; }

# ② 短语覆盖同码词典候选
clear_pad; type_in_float 'wqvb'
C=$(pad_content); echo "② wqvb => $C"
[ "$C" = '自定义抢占你好' ] && echo '② PASS 短语置顶上屏' || { echo '② FAIL'; FAIL=1; }

# ③ 长短语自动粘贴
clear_pad; type_in_float 'csph'
C=$(pad_content); echo "③ csph => ${C:0:24}…(len=${#C})"
python3 - "$THOME/.config/lyyime/phrase.json" "$C" <<'PY' && echo '③ PASS 长短语完整上屏' || { echo '③ FAIL'; FAIL=1; }
import json, sys
sys.exit(0 if json.load(open(sys.argv[1]))['csph'][0] == sys.argv[2] else 1)
PY

# ④ 管理对话框冒烟(独立进程, 走真实对话框回调链)
HOME=$THOME "$BIN" --smoke-phrases >$LOG.smoke 2>&1 \
  && grep -q 'DIALOG-OK' $LOG.smoke \
  && echo '④ PASS 管理对话框增/改/改码' || { echo '④ FAIL'; cat $LOG.smoke; FAIL=1; }

# ⑤ 动态日期触发: jjad = 五笔"日期"(真实码表居首), 候选2为动态今天日期
clear_pad
xdotool windowactivate $L; sleep 0.6
xdotool type --delay 80 'jjad'; sleep 0.6
xdotool key 2; sleep 3
C=$(pad_content); echo "⑤ jjad+2 => $C"
TODAY=$(python3 -c "from datetime import date; d=date.today(); print(f'{d.year}年{d.month}月{d.day}日')")
[ "$C" = "$TODAY" ] && echo '⑤ PASS 动态日期触发' || { echo "⑤ FAIL (期望 $TODAY)"; FAIL=1; }

echo "== done (FAIL=$FAIL) =="
exit $FAIL
