#!/usr/bin/env bash
# Mode C 悬浮窗(lyyime-float)E2E: Xvfb + openbox(提供 EWMH 激活)+ xterm(目标)。
# 覆盖: 中文上屏基线 / 空缓冲中文标点直上 / 有缓冲标点(先首选后标点, 合并一次发送) /
#       空缓冲退格直通删目标字符 / 引号开合交替。
# 用法: bash tests/e2e/float_e2e.sh [--keep]
# 前置: cargo build -p lyyime-float --release; xdotool/xterm/openbox 已安装
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FLOAT_BIN="${FLOAT_BIN:-/data/cargo-target/local/lyyIme/release/lyyime-float}"
DISP="${DISP:-:98}"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1
fail() { KEEP=1; echo "FLOAT-E2E-FAIL: $*"; exit 1; }

for t in Xvfb openbox xterm xdotool; do
    command -v "$t" >/dev/null 2>&1 || fail "缺依赖: $t"
done
[[ -x "$FLOAT_BIN" ]] || fail "悬浮窗二进制不存在: $FLOAT_BIN(先 cargo build -p lyyime-float --release)"

WORK="$(mktemp -d /tmp/lyyime-float-e2e.XXXXXX)"
export HOME="$WORK/home"
mkdir -p "$HOME/.config/lyyime"
OUT="$WORK/out.txt"

cleanup() {
    for p in "${FLOAT_PID:-}" "${XTERM_PID:-}" "${OB_PID:-}" "${XVFB_PID:-}"; do
        [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
    done
    if [[ $KEEP = 1 ]]; then
        echo "[e2e] 现场保留: $WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# 自定义短语做固定候选: 断言不依赖码表内容(精确码置顶)
printf '{"aa": ["浮窗短语"]}\n' > "$HOME/.config/lyyime/phrase.json"

Xvfb "$DISP" -screen 0 1280x800x24 -nolisten tcp & XVFB_PID=$!
for _ in $(seq 50); do
    timeout 3 xdpyinfo -display "$DISP" >/dev/null 2>&1 && break
    sleep 0.1
done
# openbox 提供 _NET_ACTIVE_WINDOW 与窗口激活(悬浮窗目标追踪/回焦依赖 EWMH)
DISPLAY="$DISP" openbox & OB_PID=$!
sleep 0.8

# 目标窗口: xterm 跑 cat, 敲 Return 后逐行落盘
DISPLAY="$DISP" xterm -e "cat > '$OUT'" & XTERM_PID=$!
XTERM_WID=""
for _ in $(seq 50); do
    XTERM_WID="$(DISPLAY="$DISP" xdotool search --onlyvisible --class '[Xx][Tt]erm' 2>/dev/null | head -1 || true)"
    [[ -n "$XTERM_WID" ]] && break
    sleep 0.2
done
[[ -n "$XTERM_WID" ]] || fail "xterm 目标窗口未出现"

DISPLAY="$DISP" "$FLOAT_BIN" & FLOAT_PID=$!
FLOAT_WID=""
for _ in $(seq 50); do
    FLOAT_WID="$(DISPLAY="$DISP" xdotool search --name '^lyyIme 悬浮输入法$' 2>/dev/null | head -1 || true)"
    [[ -n "$FLOAT_WID" ]] && break
    sleep 0.2
done
[[ -n "$FLOAT_WID" ]] || fail "悬浮窗未出现"

act() { DISPLAY="$DISP" xdotool windowactivate "$1" >/dev/null 2>&1 || true; }
type_in_float() { DISPLAY="$DISP" xdotool type --delay 70 "$1"; sleep 0.6; }
key_in_float() { DISPLAY="$DISP" xdotool key "$1"; sleep 0.8; }

# 目标登记: 先激活 xterm, 等 poll_target(250ms) 记住它, 再回悬浮窗
act "$XTERM_WID"; sleep 1.2
act "$FLOAT_WID"; sleep 0.6

# 1) 基线: 短语码 aa + 空格 → 「浮窗短语」
type_in_float "aa"
key_in_float space
# 2) 空缓冲标点: , → 「，」
type_in_float ","
# 3) 有缓冲标点: aa + , → 「浮窗短语，」(首选与标点合并一次发送)
type_in_float "aa"
type_in_float ","
# 4) 空缓冲退格直通: 删掉上一步尾部「，」(目标 tty 行内删字)
key_in_float BackSpace
# 5) 引号开合交替: 两个单引号 → 「''」
type_in_float "'"
type_in_float "'"

# 收尾: 回车让 xterm 的 cat 落盘
act "$XTERM_WID"; sleep 0.5
DISPLAY="$DISP" xdotool key Return; sleep 0.8
kill "$XTERM_PID" 2>/dev/null || true

[[ -f "$OUT" ]] || fail "目标输出文件不存在"
CONTENT="$(cat "$OUT")"
echo "[e2e] 目标窗口收到: $CONTENT"

# tty 行缓冲推演: 浮窗短语 | ，| 浮窗短语， | 退格删掉尾部，| 弯引号对 → 一行
EXPECTED="浮窗短语，浮窗短语"$'\u2018\u2019'
[[ "$CONTENT" == *"$EXPECTED"* ]] || fail "上屏内容不符: 期望含 [$EXPECTED]"
if grep -q 'aa' "$OUT"; then fail "字母 aa 裸上屏泄漏"; fi
if grep -qF ',' "$OUT"; then fail "出现半角逗号(中文标点未生效)"; fi

echo "FLOAT-E2E-OK"
