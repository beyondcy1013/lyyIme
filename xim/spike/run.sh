#!/usr/bin/env bash
# lyyIme Mode B 探路石(spike)一键复跑脚本
#
# 在 Xvfb :98 虚拟屏上:
#   1. 启动 spike-server(最小 XIM server,名字 lyyime);
#   2. 启动 spike-client(GTK3 Entry,XMODIFIERS=@im=lyyime + GTK_IM_MODULE=xim);
#   3. xdotool(XTEST)向 client 打字 't';
#   4. 断言:server 日志可见 client 连接与 forward event;
#            client Entry 缓冲出现 server commit 的 "你好尖兵"。
# 脚本自带起停与清理,可重复执行;任何一步失败即非零退出。
#
# 用法:xim/spike/run.sh [--keep](--keep 保留日志目录便于排查)
set -euo pipefail

SPIKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENDOR_DIR="$(cd "$SPIKE_DIR/../vendor/xcb-imdkit" && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

# ---- 环境变量:XIM 客户端接入三要素 + UTF-8 locale(COMPOUND_TEXT 转码依赖) ----
export DISPLAY=:98
export XMODIFIERS=@im=lyyime
export GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8

WORK="$(mktemp -d /tmp/lyyime-spike.XXXXXX)"
SERVER_LOG="$WORK/server.log"
CLIENT_LOG="$WORK/client.log"

cleanup() {
    [[ -n "${CLIENT_PID:-}" ]] && kill "$CLIENT_PID" 2>/dev/null || true
    [[ -n "${SERVER_PID:-}" ]] && kill "$SERVER_PID" 2>/dev/null || true
    [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ $KEEP -eq 1 ]]; then
        echo "[run.sh] 日志保留于 $WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

echo "== [1/6] 清理残留的 Xvfb :98 =="
if [[ -f /tmp/.X98-lock ]]; then
    oldpid="$(cat /tmp/.X98-lock 2>/dev/null || true)"
    [[ -n "$oldpid" ]] && kill "$oldpid" 2>/dev/null || true
    rm -f /tmp/.X98-lock
fi
rm -f /tmp/.X11-unix/X98

echo "== [2/6] 编译 spike(server/client),源码借鉴 xcb-imdkit 官方 test/test_server.c =="
PKGS="$(pkg-config --cflags --libs xcb xcb-util xcb-keysyms)"
gcc -O2 -std=c99 -Wall -Wextra -Werror -D_GNU_SOURCE \
    -I"$VENDOR_DIR/src" \
    "$SPIKE_DIR/spike-server.c" -o "$WORK/spike-server" \
    -L"$VENDOR_DIR" -lxcb-imdkit $PKGS
gcc -O2 -std=c99 -Wall -Wextra -Werror \
    "$SPIKE_DIR/spike-client.c" -o "$WORK/spike-client" \
    $(pkg-config --cflags --libs gtk+-3.0)

echo "== [3/6] 启动 Xvfb :98 与 spike-server =="
Xvfb :98 -screen 0 1024x768x24 -nolisten tcp &
XVFB_PID=$!
for _ in $(seq 1 50); do
    [[ -S /tmp/.X11-unix/X98 ]] && break
    sleep 0.1
done

"$WORK/spike-server" "$SERVER_LOG" lyyime &
SERVER_PID=$!
for _ in $(seq 1 50); do
    grep -q "XIM server ready" "$SERVER_LOG" 2>/dev/null && break
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server 启动失败:"; cat "$SERVER_LOG"; exit 1; }
    sleep 0.1
done
grep -q "XIM server ready" "$SERVER_LOG" || { echo "server 未就绪:"; cat "$SERVER_LOG"; exit 1; }
echo "[run.sh] server 就绪"

echo "== [4/6] 启动 GTK3 客户端(XMODIFIERS=$XMODIFIERS GTK_IM_MODULE=$GTK_IM_MODULE) =="
"$WORK/spike-client" 6 >"$CLIENT_LOG" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
sleep 2.0
grep -q "client connected" "$SERVER_LOG" \
    || { echo "FAIL: server 未见 client 连接。server.log:"; cat "$SERVER_LOG"; echo "client.stderr:"; cat "$WORK/client.stderr"; exit 1; }
echo "[run.sh] server 日志确认 GTK 客户端已连上"

echo "== [5/6] xdotool 向客户端打字 't'(应触发 server commit 你好尖兵) =="
WID="$(xdotool search --name '^lyyime-spike-client$' | head -1 || true)"
[[ -n "$WID" ]] || { echo "FAIL: 找不到客户端窗口"; exit 1; }
xdotool windowfocus "$WID"
sleep 0.3
xdotool type --delay 120 't'

echo "== [6/6] 断言 =="
# 客户端在自定超时后自行关闭并转储 ENTRY_BUFFER,轮询等待其落盘(最多 10s)
BUFFER_SEEN=0
for _ in $(seq 1 40); do
    if grep -q "^ENTRY_BUFFER=" "$CLIENT_LOG" 2>/dev/null; then
        BUFFER_SEEN=1
        break
    fi
    sleep 0.25
done
FAIL=0
if grep -q "committed \"你好尖兵\"" "$SERVER_LOG"; then
    echo "PASS: server 已 commit 你好尖兵"
else
    echo "FAIL: server 未 commit"; FAIL=1
fi
if [[ $BUFFER_SEEN -eq 1 ]] && grep -q "^ENTRY_CHANGED=你好尖兵$" "$CLIENT_LOG" \
    && grep -q "^ENTRY_BUFFER=你好尖兵$" "$CLIENT_LOG"; then
    echo "PASS: GTK Entry 缓冲 = 你好尖兵(changed 流水与关闭转储一致)"
else
    echo "FAIL: Entry 缓冲断言不过。client.log:"; cat "$CLIENT_LOG"; FAIL=1
fi

echo "---- server.log ----"; cat "$SERVER_LOG"
echo "---- client.log ----"; cat "$CLIENT_LOG"
if [[ $FAIL -eq 0 ]]; then
    echo "SPIKE PASS: XIM 全链路(连接/forward/commit/Entry 上屏)打通"
else
    echo "SPIKE FAIL"
    exit 1
fi
