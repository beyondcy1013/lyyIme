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

echo "== [0/7] 构建 =="
( cd "$ROOT" && export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme \
  && cargo build -p lyyime-core --release >/dev/null 2>&1 \
  && cargo build -p lyyime-ibus --release >/dev/null 2>&1 \
  && cargo build -p lyyime-ai --release >/dev/null 2>&1 \
  && cargo build -p lyyime-shot --release >/dev/null 2>&1 )
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
    # 只 wait 已知 pid:裸 wait 会等住常驻的 mock AI 服务(见 stop_ai_mock)。
    local wpids=()
    for p in "${CLIENT_PID:-}" "${XIM_PID:-}" "${IBUS_PID:-}" "${XVFB_PID:-}"; do
        [[ -n "$p" ]] && wpids+=("$p")
    done
    ((${#wpids[@]})) && wait "${wpids[@]}" 2>/dev/null || true
    [[ $KEEP = 1 && -n "${WORK:-}" ]] && echo "[run] 现场保留:$WORK" || rm -rf "${WORK:-/nonexistent}"
}
stop_ai_mock() { # mock 跨两个 Part 复用,只在最终退出回收(由 trap 调)
    [[ -n "${AI_MOCK_PID:-}" ]] && kill "$AI_MOCK_PID" 2>/dev/null || true
    rm -rf "${AI_WORK:-/nonexistent}"
}
cleanup_all() {
    cleanup_work
    stop_ai_mock
}
start_xvfb() { # $1=display
    local d="$1"
    [[ -f /tmp/.X$d-lock ]] && { kill "$(cat /tmp/.X$d-lock 2>/dev/null)" 2>/dev/null || true; rm -f /tmp/.X$d-lock; }
    rm -f "/tmp/.X11-unix/X$d"
    Xvfb ":$d" -screen 0 1024x768x24 -nolisten tcp & XVFB_PID=$!
    for _ in $(seq 1 50); do [[ -S /tmp/.X11-unix/X$d ]] && return 0; sleep 0.1; done
    fail "Xvfb :$d 启动失败"
}
# ---- /AI 功能公共件:mock OpenAI 服务 + 隔离 HOME 的 [ai] 配置 ----
AI_WORK="$(mktemp -d /tmp/lyyime-ai.XXXXXX)"
AI_DUMP="$AI_WORK/dump.jsonl"; AI_PORT_FILE="$AI_WORK/port"
MOCK_AI_DUMP="$AI_DUMP" python3 "$ROOT/tests/e2e/mock_ai_server.py" \
    --port-file "$AI_PORT_FILE" >"$AI_WORK/mock.log" 2>&1 & AI_MOCK_PID=$!
for _ in $(seq 1 50); do [[ -s "$AI_PORT_FILE" ]] && break; sleep 0.1; done
[[ -s "$AI_PORT_FILE" ]] || fail "mock AI 服务未就绪"
AI_PORT="$(cat "$AI_PORT_FILE")"
write_ai_config() { # $1=HOME(隔离);开启 AI 指向 mock 服务
    mkdir -p "$1/.config/lyyime"
    cat > "$1/.config/lyyime/config.toml" <<EOF
[ai]
enabled = true
api_base = "http://127.0.0.1:$AI_PORT/v1"
api_key = "sk-e2e"
model = "e2e-model"
EOF
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

trap cleanup_all EXIT

############################################
echo "== [1/7] Mode B:真库 + 真实词库(Xvfb :97) =="
new_work
export DISPLAY=:97 XMODIFIERS=@im=lyyime GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8
export LYYIME_DATA_DIR="$DATA_DIR" LYYIME_CORE_LIB="$CORE_LIB"
export LYYIME_AI_HELPER="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release/lyyime-ai"
export LYYIME_RES_DIR="$ROOT/xim/res"   # e2e 测仓库自带设置界面/样式
write_ai_config "$HOME"
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
# 词组效率提示(合同 §6):nihao 5 键上屏「你好」→ 提示更省键的 wqvb 词组;
# 提示展示在候选条并保留到下一次输入(xim 日志记录展示动作)。
grep -q 'hint: 词组提示:「你好」可用 wqvb 打出' "$XIM_LOG" || fail "B1 后未见词组提示(wqvb)"
echo "PASS B1c:词组提示「你好」= wqvb 已在候选条展示"

# 四码唯一上屏(默认开启):mqxt 真库唯一候选「网络」,第 4 键免空格直接上屏
xdotool type --delay 80 "mqxt"; sleep 0.5
wait_buffer "你好网络" 8
# 同码抑制:mqxt 4 键直上「网络」= 词组编码本身,不得重复提示。
grep -q '词组提示:「网络」' "$XIM_LOG" && fail "四码直上「网络」不应再次提示"
echo "PASS B1d:同码词组打过不重复提示"

# 新契约(v1.2):Shift 按下时有缓冲 → 上屏英文原串;随后单击确认切到英文态
xdotool type --delay 80 "the"; sleep 0.3; xdotool key Shift_L
wait_buffer "你好网络the" 8
echo "PASS B2:Shift 按下上屏英文原串 the"

xdotool type --delay 80 "abc"
wait_buffer "你好网络theabc" 8
echo "PASS B3:英文态直通 abc"

xdotool key Shift_L; sleep 0.6
xdotool type --delay 80 "zhongguo"; sleep 0.4; xdotool key space
wait_buffer "你好网络theabc中国" 8
echo "PASS B4:Shift 回中文,zhongguo 顶屏 中国"

# ---- /AI:触发 → 提示词采集(拉丁) → 回车 → mock 服务回复上屏 ----
xdotool type --delay 80 "/ai"; sleep 0.4
xdotool type --delay 80 "hi"; sleep 0.4
xdotool key Return
wait_buffer "AI回复OK" 20
grep -q '"path": "/v1/chat/completions"' "$AI_DUMP" || fail "mock 未收到请求"
grep -q '"content": "hi"' "$AI_DUMP" || fail "提示词内容不符:$(cat "$AI_DUMP")"
grep -q 'Bearer sk-e2e' "$AI_DUMP" || fail "鉴权头未携带"
echo "PASS B5:/AI hi → mock 回复 AI回复OK 已上屏"
echo "Mode B 最终缓冲: $(cat "$BUFFER")"
kill "$CLIENT_PID" "$XIM_PID" 2>/dev/null || true
cleanup_work; trap cleanup_all EXIT

############################################
echo "== [2/7] Mode A:ibus 引擎(隔离会话,Xvfb :96) =="
new_work
export DISPLAY=:96 GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
export LYYIME_DATA_DIR="$DATA_DIR" LYYIME_CORE_LIB="$CORE_LIB"
write_ai_config "$HOME"
# 截屏热键桩(A4):引擎经 ibus-daemon 继承 LYYIME_SHOT,命中即拉起写 marker
SHOT_MARKER="$WORK/shot-marker"
cat > "$WORK/stub-shot" <<STUB
#!/usr/bin/env bash
echo shot >>"$SHOT_MARKER"
STUB
chmod +x "$WORK/stub-shot"
export LYYIME_SHOT="$WORK/stub-shot"
# 把仓库引擎同步进隔离 HOME 并注册组件:否则 ibus 会拉起系统安装位的旧引擎,
# e2e 就测不到本次代码(含 /AI)。ibus 1.5.29 只认 IBUS_COMPONENT_PATH 覆盖。
ENGINE_HOME="$HOME/.local/share/lyyime/ibus/engine"
ICON_HOME="$HOME/.local/share/lyyime/ibus/icons"
mkdir -p "$ENGINE_HOME" "$ICON_HOME" "$HOME/.local/share/ibus/component"
install -m 755 "${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release/ibus-engine-lyyime" "$ENGINE_HOME/"
install -m 644 "$ROOT"/ibus-engine/icons/*.svg "$ICON_HOME/"
sed -e "s|@ENGINE_EXEC@|$ENGINE_HOME/ibus-engine-lyyime|g" -e "s|@ICON_DIR@|$ICON_HOME|g" \
    -e "s|@SETUP@|/nonexistent|g" "$ROOT/ibus-engine/lyyime.xml" \
    > "$HOME/.local/share/ibus/component/lyyime.xml"
export IBUS_COMPONENT_PATH="$HOME/.local/share/ibus/component:/usr/share/ibus/component"
export LYYIME_DEBUG=1   # 引擎 DEBUG 日志(e2e 排障用)
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
    # 注:ibus engine <name> 的返回码存在竞态误报(设置应答先于引擎工厂完成,
    # 预载与切换并发时还会瞬时取消);带重试切换,以查询结果为准。
    for _ in $(seq 1 10); do
        ibus engine lyyime 2>/dev/null || true
        [[ "$(ibus engine 2>/dev/null)" == "lyyime" ]] && break
        sleep 1
    done
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
    # ---- 回中文后先验证四码唯一上屏,再触发 /AI(须在内层会话存活时输入) ----
    xdotool key Shift_L; sleep 0.8
    xdotool type --delay 90 "mqxt"; sleep 0.4
    xdotool type --delay 90 "/ai"; sleep 0.4
    xdotool type --delay 90 "hi"; sleep 0.4
    xdotool key Return
    sleep 1.5
    # ---- CapsLock 大写态(§6):字母直通英文不进组词缓冲 ----
    xdotool key Caps_Lock; sleep 0.5
    xdotool type --delay 90 "ab"; sleep 0.4 # 无 Shift:应用输出大写 AB
    xdotool type --delay 90 "N"; sleep 0.4  # Shift+n:应用输出小写 n
    xdotool key Caps_Lock; sleep 0.5
    # ---- 截屏热键(合同 §13):命中吞键并拉起 $LYYIME_SHOT 桩 ----
    xdotool key ctrl+alt+a
    sleep 1
'
wait_buffer "你好ok" 10
IBUS_LOG="$WORK/home/.local/share/lyyime/logs/ibus.log"
grep -q 'hint: 词组提示:「你好」可用 wqvb 打出' "$IBUS_LOG" || fail "Mode A 未见词组提示(wqvb):$(tail -5 "$IBUS_LOG" 2>/dev/null)"
echo "PASS A1c:Mode A 词组提示「你好」= wqvb 已在辅助区展示"
echo "PASS A1:ibus 引擎 nihao+space → 你好,Shift 后 ok 直通"
# 四码唯一上屏:mqxt 真库唯一候选「网络」,第 4 键免空格直接上屏
wait_buffer "你好ok网络" 10
echo "PASS A1b:四码唯一 mqxt 免空格直上 网络"
wait_buffer "AI回复OK" 20
grep -q '"content": "hi"' "$AI_DUMP" || fail "Mode A 提示词内容不符:$(cat "$AI_DUMP")"
echo "PASS A2:/AI hi → mock 回复 AI回复OK 已上屏"
wait_buffer "ABn" 10
grep -q "ABn" "$BUFFER" || fail "缓冲异常"
echo "PASS A3:CapsLock 大写态直通 AB/Shift→n"
[[ -f "$SHOT_MARKER" ]] || fail "Mode A 截屏热键未拉起助手(marker 未出现)"
grep -q "已拉起截屏助手" "$IBUS_LOG" || fail "ibus 日志无截屏拉起记录"
echo "PASS A4:Mode A 截屏热键 ctrl+alt+a → 拉起 lyyime-shot(桩)并吞键"
echo "Mode A 最终缓冲: $(cat "$BUFFER")"

############################################
echo "== [3/7] 截屏助手 lyyime-shot(Xvfb :95) =="
LYYIME_SHOT_BIN="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release/lyyime-shot" bash "$ROOT/tests/e2e/shot_e2e.sh"

echo "== [4/7] 汇总 =="
echo "E2E-ALL-PASS: Mode B(8 断言)+ Mode A(6 断言)+ /AI 全链路 + lyyime-shot 通过 ✅"
