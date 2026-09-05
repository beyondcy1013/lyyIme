#!/usr/bin/env bash
# lyyIme M6 全链路端到端(真库 + 真实词库,双模式,Xvfb 隔离,可重复)
#
#   Part 1  Mode B:lyyime-xim(XIM server)+ GTK Entry,真 liblyyime_core.so + data/runtime
#   Part 2  Mode A:ibus-daemon(隔离 dbus 会话)+ lyyime 引擎 + GTK Entry,同真库
#
# 用法:bash tests/e2e/run.sh [--keep]        # --keep 保留失败现场
# 前置:cargo build -p lyyime-core --release;make -C xim;dicttool 已生成 data/runtime
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CORE_LIB="${CORE_LIB:-/data/cargo-target/local/lyyIme/release/liblyyime_core.so}"
DATA_DIR="${DATA_DIR:-$ROOT/data/runtime}"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

fail() { KEEP=1; echo "RUN-FAIL: $*"; exit 1; }

echo "== [0/6] 构建 =="
( cd "$ROOT" && export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme \
  && cargo build -p lyyime-core --release >/dev/null 2>&1 )
[[ -f "$CORE_LIB" ]] || fail "真库不存在:$CORE_LIB(先 cargo build -p lyyime-core --release)"
[[ -f "$DATA_DIR/meta.json" ]] || fail "词库不存在:$DATA_DIR(先 dicttool convert/fetch)"
make -C "$ROOT/xim" all >/dev/null
XIM_BIN="$ROOT/xim/build/bin/lyyime-xim"
CLIENT="$ROOT/xim/build/tests/e2e_client"
[[ -x "$XIM_BIN" && -x "$CLIENT" ]] || fail "lyyime-xim / e2e_client 未构建"

new_work() {
    WORK="$(mktemp -d /tmp/lyyime-run.XXXXXX)"
    export HOME="$WORK/home"; mkdir -p "$HOME"
    BUFFER="$WORK/buffer.txt"; XIM_LOG="$WORK/home/.local/share/lyyime/logs/xim.log"
}
cleanup_work() {
    for p in "${CLIENT_PID:-}" "${XIM_PID:-}" "${IBUS_PID:-}" "${XVFB_PID:-}"; do
        [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
    done
    # 注意:绝不全局 pkill ibus-daemon(会误杀真实会话)。
    # 隔离会话的 ibus 进程随 dbus-run-session 的 session bus 退出而自然消亡。
    wait 2>/dev/null || true
    [[ $KEEP = 1 && -n "${WORK:-}" ]] && echo "[run] 现场保留:$WORK" || rm -rf "${WORK:-/nonexistent}"
}
start_xvfb() { # $1=display
    local d="$1"
    [[ -f /tmp/.X$d-lock ]] && { kill "$(cat /tmp/.X$d-lock 2>/dev/null)" 2>/dev/null || true; rm -f /tmp/.X$d-lock; }
    rm -f "/tmp/.X11-unix/X$d"
    Xvfb ":$d" -screen 0 1024x768x24 -nolisten tcp & XVFB_PID=$!
    for _ in $(seq 1 50); do [[ -S /tmp/.X11-unix/X$d ]] && return 0; sleep 0.1; done
    fail "Xvfb :$d 启动失败"
}
wait_buffer() { # $1=期望 $2=超时秒
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ -f "$BUFFER" ]] && grep -qF "$want" "$BUFFER" && return 0
        sleep 0.05
    done
    echo "---- 当前缓冲 ----"; cat "$BUFFER" 2>/dev/null || echo "(空)"
    echo "---- xim.log 尾部 ----"; tail -20 "${XIM_LOG:-/dev/null}" 2>/dev/null || true
    fail "等待缓冲 [$want] 超时"
}
focus_client() {
    local wid; wid="$(xdotool search --name '^lyyime-e2e-client$' | head -1 || true)"
    [[ -n "$wid" ]] || fail "找不到 e2e 客户端窗口"
    xdotool windowfocus "$wid"; sleep 0.4
}

trap cleanup_work EXIT

############################################
echo "== [1/6] Mode B:真库 + 真实词库(Xvfb :97) =="
new_work
export DISPLAY=:97 XMODIFIERS=@im=lyyime GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8
start_xvfb 97

LYYIME_CORE_LIB="$CORE_LIB" LYYIME_DATA_DIR="$DATA_DIR" "$XIM_BIN" >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
for _ in $(seq 1 100); do grep -q "XIM server ready" "$XIM_LOG" 2>/dev/null && break; sleep 0.1; done
grep -q "XIM server ready" "$XIM_LOG" || { cat "$WORK/xim.stdout"; fail "lyyime-xim 未就绪"; }

"$CLIENT" "$BUFFER" 60 >"$WORK/client.log" 2>&1 & CLIENT_PID=$!
sleep 1.2
for _ in $(seq 1 50); do grep -q "XIM client 已连接" "$XIM_LOG" 2>/dev/null && break; sleep 0.1; done
focus_client

xdotool type --delay 80 "nihao"; sleep 0.5
xdotool key 1
wait_buffer "你好" 8
echo "PASS B1:真库数字选词 nihao→你好"

# 新契约(v1.2):Shift 按下时有缓冲 → 上屏英文原串;随后单击确认切到英文态
xdotool type --delay 80 "the"; sleep 0.3; xdotool key Shift_L
wait_buffer "你好the" 8
echo "PASS B2:Shift 按下上屏英文原串 the"

xdotool type --delay 80 "abc"
wait_buffer "你好theabc" 8
echo "PASS B3:英文态直通 abc"

xdotool key Shift_L; sleep 0.6
xdotool type --delay 80 "zhongguo"; sleep 0.4; xdotool key space
wait_buffer "你好theabc中国" 8
echo "PASS B4:Shift 回中文,zhongguo 顶屏 中国"
echo "Mode B 最终缓冲: $(cat "$BUFFER")"
kill "$CLIENT_PID" "$XIM_PID" 2>/dev/null || true
cleanup_work; trap cleanup_work EXIT

############################################
echo "== [2/6] Mode A:ibus 引擎(隔离会话,Xvfb :96) =="
new_work
export DISPLAY=:96 GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
export LYYIME_DATA_DIR="$DATA_DIR" LYYIME_CORE_LIB="$CORE_LIB"
start_xvfb 96

# 注意:VAR=x bash -c '...' 形式传的是位置参数而非环境变量,内层 set -u 会炸;
# 改为显式 export(dbus-run-session 与内层 bash 均继承)。
export CLIENT BUFFER WORK
dbus-run-session -- bash -c '
    set -uo pipefail
    ibus-daemon -drx >/dev/null 2>&1 || true
    # 就绪判据:地址文件出现且含 IBUS_ADDRESS=("ibus address" 对未就绪也返回 0,不可靠)
    ready=0
    for _ in $(seq 1 100); do
        f=$(ls "$HOME"/.config/ibus/bus/*-unix-* 2>/dev/null | head -1)
        [[ -n "$f" ]] && grep -q "^IBUS_ADDRESS=" "$f" && { ready=1; break; }
        sleep 0.2
    done
    [[ "$ready" = 1 ]] || { echo "RUN-FAIL: ibus 地址文件未就绪"; exit 1; }
    for _ in $(seq 1 100); do
        ibus list-engine 2>/dev/null | grep -q "lyyime - " && break
        sleep 0.2
    done
    ibus list-engine 2>/dev/null | grep -q "lyyime - " || { echo "RUN-FAIL: 引擎未注册"; exit 1; }
    gsettings set org.freedesktop.ibus.general preload-engines "['\''lyyime'\'']" 2>/dev/null || \
        dconf write /desktop/ibus/general/preload-engines "['\''lyyime'\'']" || true
    # 注:ibus engine <name> 的返回码存在竞态误报(设置应答先于引擎工厂完成),
    # 以其后轮询 ibus engine 查询结果为准。
    ibus engine lyyime 2>/dev/null || true
    for _ in $(seq 1 50); do [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] && break; sleep 0.2; done
    [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] || { echo "RUN-FAIL: 引擎状态未同步"; exit 1; }
    echo "[run] Mode A 引擎已激活"
    "$CLIENT" "$BUFFER" 60 >"$WORK/client.log" 2>&1 &
    sleep 1.5
    # Xvfb 无窗口管理器,必须显式聚焦客户端窗口(否则按键无处投递)
    WID="$(xdotool search --name '^lyyime-e2e-client$' | head -1 || true)"
    [[ -n "$WID" ]] && xdotool windowfocus "$WID"
    sleep 0.5
    xdotool type --delay 90 "nihao"; sleep 0.5
    xdotool key space
    sleep 0.5
    xdotool key Shift_L; sleep 0.6
    xdotool type --delay 90 "ok"
    sleep 1
'
wait_buffer "你好ok" 10
echo "PASS A1:ibus 引擎 nihao+space → 你好,Shift 后 ok 直通"
echo "Mode A 最终缓冲: $(cat "$BUFFER")"

############################################
echo "== [3/6] 汇总 =="
echo "E2E-ALL-PASS: Mode B(真库 4 断言)+ Mode A(ibus 真会话)全链路通过 ✅"
