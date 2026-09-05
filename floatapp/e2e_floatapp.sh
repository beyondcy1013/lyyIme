#!/bin/bash
# floatapp 端到端验证: 直输 & 粘贴 两种发送模式
set -x
export DISPLAY=:11.0
APP=/home/codes/apps/lyyIme/floatapp/lyyime_float.py
CFG=/home/root/.config/lyyime/config.json

pkill -f lyyime_float.py; sleep 1

L=$(xdotool search --name 'lyyIme 悬浮输入法' | head -1)
P=$(xdotool search --name '输入板' | while read w; do xdotool getwindowgeometry --shell $w | grep -q 'WIDTH=560' && echo $w; done | head -1)
echo "TARGET pad=$P (existing)"

run_one() {  # $1=发送模式  -> 输出打字板最终内容
  python3 - "$1" <<'PYEOF'
import json,sys
p='/home/root/.config/lyyime/config.json'
cfg=json.load(open(p)); cfg['send']=sys.argv[1]
json.dump(cfg,open(p,'w'),ensure_ascii=False)
PYEOF
  nohup python3 $APP >>/tmp/floatapp.log 2>&1 &
  APPPID=$!
  sleep 6
  L=$(xdotool search --name 'lyyIme 悬浮输入法' | head -1)
  echo "  app pid=$APPPID win=$L"
  xdotool windowactivate $P; sleep 0.6; xdotool key ctrl+a; xdotool key Delete; sleep 0.3
  xdotool windowactivate $L; sleep 0.6
  xdotool type --delay 80 'wqvb'; sleep 0.6; xdotool key space; sleep 2
  xdotool windowactivate $P; sleep 0.4; xdotool key ctrl+a; xdotool key ctrl+c; sleep 0.6
  python3 -c "
import gi; gi.require_version('Gtk','3.0')
from gi.repository import Gtk, Gdk
Gtk.init()
print('  RESULT:', repr(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()))"
  kill $APPPID 2>/dev/null; sleep 1
}

echo "== 直输模式 =="
run_one type
echo "== 粘贴模式 =="
run_one paste
echo "== done =="
