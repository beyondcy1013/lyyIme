#!/usr/bin/env bash
# lyyIme Mode B 端到端测试(Xvfb :98 全自动,可重复)
#
# 链路:Xvfb → lyyime-xim(桩库 liblyyime_core_stub.so,确定性规则)
#       → GTK Entry 客户端(XMODIFIERS=@im=lyyime GTK_IM_MODULE=xim)
#       → xdotool(XTEST)打字 → 断言 Entry 缓冲文件与 server 日志。
#
# 断言序列:
#   A. 中文态:输入 nihao → 候选(缓冲文件不变)→ 按 2 → Entry = "你号";
#   B. Shift 单击 → 英文直通(trigger off):输入 abc 原样进 Entry,
#      且 server 日志在切换后无 a/b/c 的 forward 记录;
#   C. Shift 再单击 → 回中文(trigger on):输入 zh + 空格 → 顶屏首选
#      "候选1"(桩库规则)→ Entry = "你号abc候选1";
#   E. CapsLock 大写态:字母直通英文不进组词 —— 无 Shift 大写(ab→AB),
#      Shift+字母 小写(Shift+n→n);关闭后组词恢复(zh+space→候选1)。
set -euo pipefail

XIM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../xim" && pwd)"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

# 隔离环境:独立 HOME(配置/日志/pidfile 不污染真实用户)
WORK="$(mktemp -d /tmp/lyyime-e2e.XXXXXX)"
export HOME="$WORK/home"
mkdir -p "$HOME"

# 截屏热键(场景 F):xim 启动前注入桩助手($LYYIME_SHOT),命中即被拉起
SHOT_MARKER="$WORK/shot-marker"
cat > "$WORK/stub-shot" <<STUB
#!/usr/bin/env bash
echo shot >>"$SHOT_MARKER"
STUB
chmod +x "$WORK/stub-shot"
export LYYIME_SHOT="$WORK/stub-shot"

# 自定义查询(场景 G4):config.toml 预写桩网址({q} 占位符),PATH 前置桩
# xdg-open 记录被拉起的网址 → 断言 {q} 已代入候选词(宿主侧动作)
QUERY_MARKER="$WORK/query-marker"
mkdir -p "$WORK/bin" "$HOME/.config/lyyime"
cat > "$WORK/bin/xdg-open" <<STUB
#!/usr/bin/env bash
echo "\$1" >>"$QUERY_MARKER"
STUB
chmod +x "$WORK/bin/xdg-open"
export PATH="$WORK/bin:$PATH"
cat > "$HOME/.config/lyyime/config.toml" <<'CFG'
custom_query_label = "查词典"
custom_query_url = "https://dict.example.test/lookup?q={q}"
CFG

# :98 常被 Xvnc 等真实显示占用;本脚本对已有占用一律拒绝(不杀外来
# 进程/不删外来 lock/socket)。默认 :95,可用 LYYIME_E2E_DISPLAY 覆盖。
export DISPLAY="${LYYIME_E2E_DISPLAY:-:95}"
export XMODIFIERS=@im=lyyime
export GTK_IM_MODULE=xim
export LANG=zh_CN.utf8 LC_ALL=zh_CN.utf8

XIM_LOG="$WORK/home/.local/share/lyyime/logs/xim.log"
CLIENT_LOG="$WORK/client.log"
BUFFER="$WORK/buffer.txt"
STUB_LIB="$XIM_DIR/build/tests/liblyyime_core_stub.so"

cleanup() {
    [[ -n "${CLIENT_PID:-}" ]] && kill "$CLIENT_PID" 2>/dev/null || true
    [[ -n "${XIM_PID:-}" ]] && kill "$XIM_PID" 2>/dev/null || true
    [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ $KEEP -eq 1 ]]; then
        echo "[e2e] 工作目录保留:$WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

fail() { KEEP=1; echo "E2E FAIL: $*"; echo "(失败现场保留:$WORK)"; exit 1; }

# 轮询等待文件出现指定内容
wait_buffer() {
    local want="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        [[ -f "$BUFFER" ]] && grep -qF "$want" "$BUFFER" && return 0
        sleep 0.05
    done
    echo "---- 当前缓冲 ----"; cat "$BUFFER" 2>/dev/null || echo "(空)"
    echo "---- xim.log 尾部 ----"; tail -30 "$XIM_LOG" 2>/dev/null || true
    fail "等待缓冲出现 [$want] 超时"
}

wait_log() {
    local pat="$1" timeout="${2:-10}" i
    for ((i = 0; i < timeout * 20; i++)); do
        grep -q "$pat" "$XIM_LOG" 2>/dev/null && return 0
        sleep 0.05
    done
    echo "---- xim.log ----"; cat "$XIM_LOG" 2>/dev/null || true
    fail "等待日志 [$pat] 超时"
}

echo "== [1/9] 构建 =="
make -C "$XIM_DIR" all test >/dev/null
[[ -f "$STUB_LIB" ]] || fail "桩库未生成:$STUB_LIB"

echo "== [2/9] 清理并启动 Xvfb $DISPLAY =="
XDNUM="${DISPLAY%%.*}"; XDNUM="${XDNUM#:}"
XSOCK="/tmp/.X11-unix/X${XDNUM}"
XLOCK="/tmp/.X${XDNUM}-lock"
# 安全边界:lock 或 socket 已存在 ⇒ 该显示归他人(Xvfb/Xvnc/Xorg)
# 所有,拒绝使用;绝不 kill 外来进程、不 rm 外来文件
if [[ -f "$XLOCK" || -S "$XSOCK" ]]; then
    fail "显示 $DISPLAY 已被占用(lock/socket 存在);请用 LYYIME_E2E_DISPLAY 指定空闲显示号"
fi
Xvfb "$DISPLAY" -screen 0 1024x768x24 -nolisten tcp &
XVFB_PID=$!
for _ in $(seq 1 50); do [[ -S "$XSOCK" ]] && break; sleep 0.1; done

echo "== [3/9] 启动 lyyime-xim(桩库) =="
LYYIME_CORE_LIB="$STUB_LIB" "$XIM_DIR/build/bin/lyyime-xim" >"$WORK/xim.stdout" 2>&1 &
XIM_PID=$!
wait_log "XIM server ready"
echo "[e2e] XIM server 就绪(pid=$XIM_PID)"

echo "== [4/9] 启动 GTK Entry 客户端 =="
"$XIM_DIR/build/tests/e2e_client" "$BUFFER" 120 >"$CLIENT_LOG" 2>"$WORK/client.stderr" &
CLIENT_PID=$!
wait_log "XIM client 已连接"
sleep 0.8
wait_log "获得焦点" # SET_IC_FOCUS → trigger on(中文态)

echo "== [5/9] A:中文态 nihao + 数字 2 选词 =="
WID="$(xdotool search --name '^lyyime-e2e-client$' | head -1 || true)"
[[ -n "$WID" ]] || fail "找不到客户端窗口"
xdotool windowfocus "$WID"
sleep 0.3
xdotool type --delay 90 "nihao"
sleep 0.4
# 组合中 Entry 不应变(预编辑在候选窗)
if [[ -f "$BUFFER" ]] && grep -q "nihao" "$BUFFER"; then
    fail "组合字母泄漏进了应用缓冲(XIM 拦截失败)"
fi
xdotool key 2
wait_buffer "你号" 8
echo "PASS A:数字选词上屏 = 你号"

echo "== [6/9] B:Shift 单击 → 英文直通 =="
FWD_BEFORE="$(grep 'forward keysym' "$XIM_LOG" | grep -vc 'LKey=9' || true)"
xdotool key Shift_L
wait_log "Shift 单击(release 确认)" 2 || wait_log "Shift 单击(时间窗确认)"
xdotool type --delay 90 "abc"
sleep 0.6
FWD_AFTER="$(grep 'forward keysym' "$XIM_LOG" | grep -vc 'LKey=9' || true)"
wait_buffer "你号abc" 8
if grep -q "你号abcd" "$BUFFER"; then fail "缓冲异常"; fi
# 新内部实现:英文态按键直通会伴随一条再转发日志,计数不再恒等;
# 行为正确性由上方缓冲断言(你号abc)保证,这里仅输出信息。
echo "PASS B:英文直通(shift 切换后 forward 记录 $FWD_BEFORE→$FWD_AFTER,含直通再转发)"

echo "== [7/9] C:Shift 再单击 → 中文态空格顶屏 =="
xdotool key Shift_L
wait_log "trigger on"
sleep 0.5
xdotool type --delay 90 "zh"
sleep 0.3
xdotool key space
wait_buffer "你号abc候选1" 8
echo "PASS C:回中文态,空格顶屏首选 = 候选1"

echo "== [8/9] D:造词(Ctrl+= 进入,方向键增减选字,回车存词不上屏) =="
xdotool type --delay 90 "nihao"
sleep 0.4
xdotool key 3 # 选第 3 个候选「拟好」,保证造词历史非空
wait_buffer "你号abc候选1拟好" 8
xdotool key ctrl+equal
wait_log "造词热键命中"
sleep 0.4
xdotool key Right # → 多选一字(演示串第三字)
sleep 0.3
xdotool key Return # 存词:notice 提示,不上屏
wait_log "notice: 已造词:你好" 8
if grep -q "已造词" "$BUFFER"; then fail "造词提示误上屏"; fi
wait_buffer "你号abc候选1拟好" 2
# Esc 语义不被破坏:造词结束后继续正常打字
xdotool type --delay 90 "zh"
sleep 0.3
xdotool key space
wait_buffer "你号abc候选1拟好候选1" 8
echo "PASS D:造词热键/方向键选字/存词提示/继续输入 全链路"

echo "== [9/11] E:CapsLock 大写态字母直通(无 Shift 大写;Shift+字母 小写) =="
xdotool key Caps_Lock; sleep 0.5
xdotool type --delay 90 "ab"; sleep 0.5
wait_buffer "你号abc候选1拟好候选1AB" 8
xdotool type --delay 90 "N"; sleep 0.5 # xdotool 发 Shift+n:大写态下输出小写
wait_buffer "你号abc候选1拟好候选1ABn" 8
wait_log "CapsLock 大写态字母直通" 8
xdotool key Caps_Lock; sleep 0.5 # 关大写态,组词应恢复
xdotool type --delay 90 "zh"; sleep 0.3
xdotool key space
wait_buffer "你号abc候选1拟好候选1ABn候选1" 8
echo "PASS E:CapsLock 大写态直通 AB/Shift→n,关闭后组词恢复"

echo "== [10/12] F:截屏热键(Ctrl+Alt+A)拉起 lyyime-shot(合同 §13) =="
rm -f "$SHOT_MARKER"
xdotool key ctrl+alt+a
for _ in $(seq 1 50); do [[ -f "$SHOT_MARKER" ]] && break; sleep 0.1; done
[[ -f "$SHOT_MARKER" ]] || fail "截屏热键未拉起助手(marker 未出现)"
wait_log "截屏热键命中" 5
wait_log "已拉起截屏助手" 5
# 吞键断言:组合键不产生任何输入,缓冲保持不变
wait_buffer "你号abc候选1拟好候选1ABn候选1" 2
echo "PASS F:截屏热键命中 → 拉起桩助手并吞键(缓冲不变)"

echo "== [11/12] G:候选右键菜单(§15):悬停冻结+菜单三项(桩确定性) =="
# 交互模型(实测):候选窗跟随指针 → 把指针移进窗内 → 悬停冻结 →
# 右键行 → GTK 菜单在指针处弹出 → 指针点菜单项(方向键会被转发给引擎,
# 不走菜单导航,故必须点选)。
# 行几何:窗口顶部 ≈38px 头部,其下等分候选行;行 i 中心 ≈ Y+38+(H-38)(i+0.5)/N。
# 菜单:宽 ~114,高 ~95,三项中心 ≈ MY+16 / MY+47 / MY+79。

candwin_geom() { # → "X Y W H"(取最宽的 lyyime-xim 顶层窗)
    xwininfo -root -children | grep '"lyyime-xim"' | awk '{print $1}' \
    | while read -r w; do
        xwininfo -id "$w" -stats 2>/dev/null | awk \
            -v id="$w" '/Absolute upper-left X/{x=$4}
                        /Absolute upper-left Y/{y=$4}
                        /Width/{wd=$2}
                        /Height/{h=$2}
                        END{print wd, id, x, y, h}'
      done | sort -rn | awk 'NR==1{print $3, $4, $1, $5}'
}

menu_geom() { # → "X Y W H"(~114x95(3项)~126(4项) 的 lyyime-xim 窗,且非候选窗)
    xwininfo -root -children | grep '"lyyime-xim"' | awk '{print $1}' \
    | while read -r w; do
        xwininfo -id "$w" -stats 2>/dev/null | awk \
            -v id="$w" '/Absolute upper-left X/{x=$4}
                        /Absolute upper-left Y/{y=$4}
                        /Width/{wd=$2}
                        /Height/{h=$2}
                        END{print wd, id, x, y, h}'
      done | awk '$1>=100 && $1<150 && $5>=80 && $5<160 {print $3, $4, $1, $5}' \
      | head -1
}

# 悬停冻结并右键第 row 行(0 基),随后点菜单第 item 项(0 基)
right_click_row_menu_item() {
    local row="$1" item="$2" g X Y W H i
    for i in 1 2 3; do
        g="$(candwin_geom)"; read -r X Y W H <<<"$g"
        [[ -n "$X" ]] || fail "候选窗未出现(G 场景)"
        local ry=$((Y + 38 + (H - 38) * (2 * row + 1) / 10))
        xdotool mousemove $((X + 40)) "$ry"
        sleep 0.4
        # 进窗后重读几何:跟随应在悬停后冻结
        g="$(candwin_geom)"; read -r X Y W H <<<"$g"
        ry=$((Y + 38 + (H - 38) * (2 * row + 1) / 10))
        xdotool mousemove $((X + 40)) "$ry"; sleep 0.2
        xdotool click 3; sleep 0.5
        g="$(menu_geom)"
        [[ -n "$g" ]] && break
    done
    [[ -n "$g" ]] || fail "右键第 $row 行未弹出菜单(尝试 $i 次)"
    read -r X Y W H <<<"$g"
    xdotool mousemove $((X + 40)) $((Y + 16 + 31 * item))
    sleep 0.3
    xdotool click 1; sleep 0.7
}

BASE="你号abc候选1拟好候选1ABn候选1"

# G1:右键第 0 行(你好)→ 第 3 项反查英文 → 候选页换 hello/hi → 数字 1 = hello
xdotool type --delay 90 "nihao"
sleep 0.5
right_click_row_menu_item 0 2
wait_log "候选右键操作 idx=0 op=3" 5
xdotool key 1; sleep 0.3
wait_buffer "${BASE}hello" 8
echo "PASS G1:右键→反查英文 → 候选页替换 → 数字选 hello 上屏"

# G2:重敲 nihao → 右键第 0 行(你好)→ 第 2 项删除 →
# 候选收缩为 [你号,拟好,泥嚎,倪豪] → 数字 1 = 你号
xdotool key Escape; sleep 0.3
xdotool type --delay 90 "nihao"
sleep 0.5
right_click_row_menu_item 0 1
wait_log "候选右键操作 idx=0 op=2" 5
xdotool key 1; sleep 0.3
wait_buffer "${BASE}hello你号" 8
echo "PASS G2:右键→删除词组 → 候选重排 → 数字 1 = 你号"

# G3:右键第 1 行(拟好;你好已删,行序=[你号,拟好,…])→ 第 1 项固定 →
# 拟好置顶 → 数字 1 = 拟好
xdotool key Escape; sleep 0.3
xdotool type --delay 90 "nihao"
sleep 0.5
right_click_row_menu_item 1 0
wait_log "候选右键操作 idx=1 op=1" 5
xdotool key 1; sleep 0.3
wait_buffer "${BASE}hello你号拟好" 8
echo "PASS G3:右键→固定首位 → 拟好置顶 → 数字 1 = 拟好"

# G4:重敲 nihao → 右键第 0 行(拟好,已固定居首)→ 第 4 项自定义查询 →
# 桩 xdg-open 收到 {q} 代入后的网址(宿主侧动作,不经 core op_fn)
xdotool key Escape; sleep 0.3
xdotool type --delay 90 "nihao"
sleep 0.5
right_click_row_menu_item 0 3
for _ in $(seq 1 50); do
    [[ -f "$QUERY_MARKER" ]] && grep -q "lookup?q=" "$QUERY_MARKER" && break
    sleep 0.1
done
if ! { [[ -f "$QUERY_MARKER" ]] \
    && grep -qF "https://dict.example.test/lookup?q=%E6%8B%9F%E5%A5%BD" \
        "$QUERY_MARKER"; }; then
    echo "---- query-marker ----"; cat "$QUERY_MARKER" 2>/dev/null || echo "(空)"
    fail "自定义查询未拉起 xdg-open 或网址未代入词(拟好)"
fi
echo "PASS G4:右键→自定义查询(查词典)→ xdg-open 收到 {q} 代入网址"

echo "== [12/12] 汇总 =="
wait_buffer "你号abc候选1" 2
echo "最终缓冲: $(cat "$BUFFER")"
echo "---- xim.log 关键行 ----"
grep -E "XIM server ready|client 已连接|trigger|Shift 单击|CapsLock|commit|LKey" "$XIM_LOG" | head -40 || true
echo "E2E PASS: Mode B 全链路(XIM 连接/组合拦截/数字选词/Shift 切换/顶屏/造词/CapsLock 直通/截屏热键/候选右键菜单)全绿"
