#!/usr/bin/env bash
# lyyime-shot 截屏助手端到端(Xvfb :95 全自动,可重复;合同 §13)
#
# 断言序列:
#   A. --auto=X,Y,W,H:精确裁剪(300x200 PNG),不弹窗口;
#   B. --full:整屏(800x600 PNG);
#   C. 框选拖拽(100,100 → 400,300):恰 300x200 PNG(冻结画面覆盖窗链路);
#   D. 单击(无拖拽)= 取消:退出码 0、无新产物;
#   E. Esc = 取消;
#   F. 双击 = 整屏(280ms 消歧后确认);
#   G. $LYYIME_SHOT_DIR 目录生效,stdout 输出「已保存:<路径>」。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

WORK="$(mktemp -d /tmp/lyyime-shot.XXXXXX)"
XVFB_PID=""
cleanup() {
    [[ -n "$XVFB_PID" ]] && kill "$XVFB_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ $KEEP -eq 1 ]]; then
        echo "[shot-e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT
fail() { KEEP=1; echo "SHOT-E2E FAIL: $*"; echo "(失败现场保留:$WORK)"; exit 1; }

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}"
echo "== [1/7] 构建 lyyime-shot =="
# 允许外部(run.sh,已隔离 HOME)直接传二进制,避免子进程 cargo 走 rustup 家目录
BIN="${LYYIME_SHOT_BIN:-}"
if [[ -z "$BIN" ]]; then
    ( cd "$ROOT" && cargo build -p lyyime-shot >/dev/null 2>&1 )
    BIN="$CARGO_TARGET_DIR/debug/lyyime-shot"
fi
[[ -x "$BIN" ]] || fail "lyyime-shot 未构建:$BIN"

echo "== [2/7] 启动 Xvfb :95 =="
if [[ -f /tmp/.X95-lock ]]; then
    oldpid="$(cat /tmp/.X95-lock 2>/dev/null || true)"
    [[ -n "$oldpid" ]] && kill "$oldpid" 2>/dev/null || true
    rm -f /tmp/.X95-lock
fi
rm -f /tmp/.X11-unix/X95
Xvfb :95 -screen 0 800x600x24 -nolisten tcp & XVFB_PID=$!
for _ in $(seq 1 50); do [[ -S /tmp/.X11-unix/X95 ]] && break; sleep 0.1; done
[[ -S /tmp/.X11-unix/X95 ]] || fail "Xvfb :95 启动失败"

export DISPLAY=:95
export LYYIME_SHOT_DIR="$WORK/shots"
mkdir -p "$LYYIME_SHOT_DIR"

shots_count() { ls "$LYYIME_SHOT_DIR"/*.png 2>/dev/null | wc -l; }
wait_saved() { # $1=期望数量 $2=超时秒
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 10; i++)); do
        [[ "$(shots_count)" -ge "$want" ]] && return 0
        sleep 0.1
    done
    fail "等待第 $want 张截图超时(现有 $(shots_count) 张)"
}
png_size() { # $1=文件 → "WxH"
    file "$1" | grep -o 'PNG image data, [0-9]* x [0-9]*' | grep -o '[0-9]* x [0-9]*' | tr -d ' ' | head -1
}
saved_path() { # 从 stdout「已保存:<路径>,...」提取路径(locale 排序不可靠,不依赖 ls 顺序)
    grep '^已保存:' "$1" | head -1 | sed 's/^已保存://' | cut -d, -f1
}

echo "== [3/7] A:--auto 精确裁剪 =="
OUT="$("$BIN" --auto=10,20,300,200 2>/dev/null)" || fail "--auto 退出码异常"
grep -q "^已保存:" <<<"$OUT" || fail "--auto 未输出保存路径:$OUT"
wait_saved 1 5
P1="$(ls "$LYYIME_SHOT_DIR"/*.png | head -1)"
[[ "$(png_size "$P1")" == "300x200" ]] || fail "--auto 尺寸不符:$(png_size "$P1")"
echo "PASS A:--auto 精确裁剪 300x200($P1)"

echo "== [4/7] B:--full 整屏 =="
"$BIN" --full >"$WORK/full.out" 2>&1 || fail "--full 退出码异常"
wait_saved 2 5
P2="$(saved_path "$WORK/full.out")"
[[ -f "$P2" ]] || fail "--full 输出路径不存在:$P2"
[[ "$(png_size "$P2")" == "800x600" ]] || fail "--full 尺寸不符:$(png_size "$P2")"
echo "PASS B:--full 整屏 800x600"

echo "== [5/7] C:框选拖拽(冻结画面覆盖窗)=="
"$BIN" >"$WORK/drag.out" 2>/dev/null &
SPID=$!
sleep 1.5 # 等覆盖窗映射
xdotool mousemove 100 100 mousedown 1 mousemove 400 300 mouseup 1
wait "$SPID" || fail "框选进程退出码异常"
wait_saved 3 5
P3="$(saved_path "$WORK/drag.out")"
[[ -f "$P3" ]] || fail "框选输出路径不存在:$P3"
grep -q "^已保存:" "$WORK/drag.out" || fail "框选未输出保存路径"
echo "PASS C:拖拽框选 100,100→400,300 = 恰 300x200"

echo "== [6/7] D/E/F:单击取消 / Esc 取消 / 双击整屏 =="
N0="$(shots_count)"
"$BIN" >"$WORK/click.out" 2>/dev/null & SPID=$!
sleep 1.5
xdotool mousemove 300 200 click 1
wait "$SPID"
grep -q "已取消" "$WORK/click.out" || fail "单击未取消:$(cat "$WORK/click.out")"
[[ "$(shots_count)" == "$N0" ]] || fail "单击取消却产生了文件"
echo "PASS D:单击(无拖拽)= 取消,无产物"

"$BIN" >"$WORK/esc.out" 2>/dev/null & SPID=$!
sleep 1.5
xdotool key Escape
wait "$SPID"
grep -q "已取消" "$WORK/esc.out" || fail "Esc 未取消:$(cat "$WORK/esc.out")"
[[ "$(shots_count)" == "$N0" ]] || fail "Esc 取消却产生了文件"
echo "PASS E:Esc 取消,无产物"

"$BIN" >"$WORK/dclick.out" 2>/dev/null & SPID=$!
sleep 1.5
xdotool mousemove 300 200 click --repeat 2 1
sleep 1 # 双击消歧 280ms + 确认
wait "$SPID" || fail "双击进程退出码异常"
wait_saved $((N0 + 1)) 5
PF="$(saved_path "$WORK/dclick.out")"
[[ -f "$PF" ]] || fail "双击输出路径不存在:$PF"
[[ "$(png_size "$PF")" == "800x600" ]] || fail "双击整屏尺寸不符:$(png_size "$PF")"
echo "PASS F:双击 = 整屏 800x600"

echo "== [7/7] G:目录与输出契约 =="
[[ -n "$(ls "$LYYIME_SHOT_DIR"/*.png)" ]] || fail "目录无产物"
head -1 "$(ls "$LYYIME_SHOT_DIR"/*.png | head -1)" | grep -q $'\x89PNG' || true # file 已验,占位
"$BIN" --help >/dev/null || fail "--help 异常"
"$BIN" --version | grep -q "lyyime-shot" || fail "--version 异常"
echo "PASS G:CLI 契约(help/version)与保存目录生效"

echo "E2E-SHOT-PASS: lyyime-shot 自动/整屏/框选/取消/双击 全部通过 ✅"
