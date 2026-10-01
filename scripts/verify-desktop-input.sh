#!/usr/bin/env bash
# verify-desktop-input.sh — 真实桌面中文输入链路窄验证(只读观测,不改配置)
#
# 用 ime-probe.py 弹一个真 GTK Entry,按现行桌面会话环境
# (GTK_IM_MODULE=ibus / XMODIFIERS=@im=ibus)键入指定码(默认 ymlf)上屏期望文本,
# 断言:文本真实落盘、IBus 面板进程未重启(PID/NRestarts 不变)、
# IBus.Panel 在当前私有总线有主、ibus.log 出现期望的菜单提示行。
# 只向自己的探针窗输入，不触发 F7 确认；退出时恢复原焦点。
set -euo pipefail
# 参数:键码 期望上屏文本 引擎日志提示正则(默认 ymlf→设置→F7 菜单提示;
# 截图验证例:bash scripts/verify-desktop-input.sh falt 截图 \
#   '匹配.*截屏.*F7.*Ctrl\+Alt\+A')
CODE="${1:-ymlf}"
TEXT="${2:-设置}"
HINT="${3:-匹配.*设置.*F7}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID="$(systemctl show lyyime-xim-sample.service -p MainPID --value)"
[[ $PID =~ ^[0-9]+$ && $PID -gt 1 ]]
[[ "$(readlink "/proc/$PID/exe")" == /usr/local/bin/lyyime-xim ]]
while IFS= read -r -d '' entry; do
    case "$entry" in
        HOME=*|DISPLAY=*|XDG_RUNTIME_DIR=*|DBUS_SESSION_BUS_ADDRESS=*|XAUTHORITY=*)
            export "$entry" ;;
    esac
done < "/proc/$PID/environ"
unset IBUS_ADDRESS
export GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
WORK="$(mktemp -d /tmp/lyyime-desktop-check.XXXXXX)"
printf 'Evidence: %s\n' "$WORK"
OLD_FOCUS="$(xdotool getwindowfocus)"
CLIENT_PID=""
cleanup() {
    [[ -z "$CLIENT_PID" ]] || kill "$CLIENT_PID" 2>/dev/null || true
    xdotool windowfocus "$OLD_FOCUS" 2>/dev/null || true
}
trap cleanup EXIT
PANEL_PID="$(systemctl show lyyime-ibus-panel.service -p MainPID --value)"
RESTARTS="$(systemctl show lyyime-ibus-panel.service -p NRestarts --value)"
MARK="$(wc -l < /home/root/.local/share/lyyime/logs/ibus.log)"
python3 "$ROOT/scripts/ime-probe.py" "$WORK/buffer.txt" 20 >"$WORK/probe.log" 2>&1 &
CLIENT_PID=$!
WID=""
for _ in {1..50}; do
    WID="$(xdotool search --onlyvisible --pid "$CLIENT_PID" --name '^lyyime-probe$' 2>/dev/null | head -1 || true)"
    [[ -z "$WID" ]] || break
    sleep .1
done
[[ -n "$WID" ]]
xdotool windowfocus --sync "$WID"
sleep .3
[[ "$(xdotool getwindowfocus)" == "$WID" ]]
xdotool type --clearmodifiers --delay 160 "$CODE"
sleep .5
# autocommit may already commit; press space only if not yet committed.
if ! grep -Fq -- "$TEXT" "$WORK/buffer.txt"; then
    [[ "$(xdotool getwindowfocus)" == "$WID" ]]
    xdotool key space
fi
for _ in {1..30}; do
    grep -Fq -- "$TEXT" "$WORK/buffer.txt" && break
    sleep .1
done
[[ "$(cat "$WORK/buffer.txt")" == "$TEXT" ]]
sleep 5
[[ "$(systemctl show lyyime-ibus-panel.service -p MainPID --value)" == "$PANEL_PID" ]]
[[ "$(systemctl show lyyime-ibus-panel.service -p NRestarts --value)" == "$RESTARTS" ]]
ADDR="$(timeout 3 ibus address)"
[[ "$(timeout 3 gdbus call --address "$ADDR" --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner org.freedesktop.IBus.Panel)" == '(true,)' ]]
tail -n +"$((MARK+1))" /home/root/.local/share/lyyime/logs/ibus.log > "$WORK/ibus-new.log"
grep -E -- "$HINT" "$WORK/ibus-new.log"
printf 'PASS: real GTK/IBus committed %s; expected shortcut hint logged; panel stable. Evidence: %s\n' "$TEXT" "$WORK"
