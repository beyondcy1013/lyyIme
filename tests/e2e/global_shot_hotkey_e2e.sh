#!/usr/bin/env bash
# lyyime-xim 全局截屏快捷键(XFCE xfconf)端到端(隔离环境全自动,合同 §13)
#
# 隔离方式:xvfb-run -a(独立 X 显示)+ dbus-run-session(独立会话总线)
# + 临时 HOME/XDG_CONFIG_HOME/XDG_DATA_HOME —— 不碰真实桌面与真实配置,
# /usr/local/bin/lyyime-shot 只读使用,绝不覆盖。
# 失败时自动保留 $WORK 现场(或 --keep 强制保留)。
#
# 断言序列:
#   1. 初始登记 + 幂等:ctrl+alt+a → /commands/custom/<Primary><Alt>a;
#   2. 事务回滚 API:unit_hotkey --global-rollback-test 后全频道属性原样;
#   3. 真实触发:xdotool ctrl+alt+a → lyyime-shot 覆盖窗出现,Esc 取消;
#   4. 改键 ctrl+alt+a → ctrl+alt+s:旧属性移除、新键触发、旧键不再触发;
#   5. <Control>≡<Primary> 别名冲突拒绝,且全部属性原样;
#   6. <Mod4>≡<Super> 别名冲突拒绝(super+f2 ↔ <Mod4>F2);
#   7. xfwm4 窗口管理器绑定冲突拒绝;
#   8. 会话总线缺失 → 非零退出;--remove-shot-hotkey 只摘自有绑定;
#   9. 真实设置窗(--settings-page 2,AT-SPI 驱动):字段改值+点「确定」
#      → 配置与 XFCE 键同步;外部占用冲突 → 弹窗拒绝,配置与登记原样。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# ---- 外层:隔离环境装配,内层(--inner)跑用例 ----
if [[ "${1:-}" != "--inner" ]]; then
    KEEP=0
    for a in "$@"; do [[ "$a" == "--keep" ]] && KEEP=1; done
    WORK="$(mktemp -d /tmp/lyyime-ghk.XXXXXX)"
    export LYY_GH_WORK="$WORK"
    export HOME="$WORK/home"
    export XDG_CONFIG_HOME="$WORK/config" XDG_DATA_HOME="$WORK/data" \
           XDG_CACHE_HOME="$WORK/cache"
    mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"
    export XDG_CURRENT_DESKTOP=XFCE
    # 设置窗测试必须吃到**源码树新 UI**,不能落到已安装的旧资源
    export LYYIME_RES_DIR="$ROOT/xim/res"
    export LYYIME_SHOT_DIR="$WORK/shots"
    mkdir -p "$LYYIME_SHOT_DIR"
    rc=0
    xvfb-run -a dbus-run-session -- bash "$0" --inner || rc=$?
    # 失败现场一律保留供排查;--keep 成功也留
    if [[ "$rc" != 0 || "$KEEP" == 1 ]]; then
        echo "[ghk-e2e] 现场保留(rc=$rc):$WORK"
    else
        rm -rf "$WORK"
    fi
    exit "$rc"
fi

# ================= 内层(隔离会话内) =================
WORK="$LYY_GH_WORK"
XIM_BIN="${LYYIME_XIM_BIN:-$ROOT/xim/build/bin/lyyime-xim}"
UNIT_HOTKEY="${LYYIME_UNIT_HOTKEY:-$ROOT/xim/build/tests/unit_hotkey}"
SHOT_BIN="/usr/local/bin/lyyime-shot"
CHAN="xfce4-keyboard-shortcuts"
CFG="$XDG_CONFIG_HOME/lyyime/config.toml"
XIM_PID=""

fail() { echo "GHK-E2E FAIL: $*" >&2; exit 1; }

[[ -x "$XIM_BIN" ]] || fail "lyyime-xim 未构建:$XIM_BIN(先 make -C xim all)"
[[ -x "$UNIT_HOTKEY" ]] || fail "unit_hotkey 未构建:$UNIT_HOTKEY"
[[ -x "$SHOT_BIN" ]] || fail "$SHOT_BIN 未安装"
for b in xfsettingsd xfconf-query xdotool python3; do
    command -v "$b" >/dev/null || fail "$b 不在 PATH"
done
python3 -c 'import gi; gi.require_version("Atspi","2.0");
from gi.repository import Atspi' 2>/dev/null ||
    fail "python3 GI Atspi 不可用(设置窗驱动依赖)"
[[ -n "${DISPLAY:-}" ]] || fail "无 DISPLAY"
[[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] || fail "无会话总线"
# 防误伤:隔离环境标记必须生效(双保险,拒绝在真实 HOME 上跑)
[[ "$HOME" == "$WORK/home" ]] || fail "HOME 未隔离:$HOME"

dbus-update-activation-environment --all >/dev/null 2>&1 || true

# 真 xfsettingsd(本机构建以前台为默认,无 --no-daemon;-D 跳过 WM 等待,
# 隔离显示里没有窗口管理器)
xfsettingsd -D >"$WORK/xfsettingsd.log" 2>&1 &
XSD_PID=$!
cleanup() {
    local rc=$? # 保留用例失败码,绝不让清理动作吞掉
    # 只清我们自己拉起的进程:lyyime-xim、尚在的覆盖窗属主、xfsettingsd
    [[ -n "${XIM_PID:-}" ]] && kill "$XIM_PID" 2>/dev/null || true
    local w p
    w="$({ xdotool search --onlyvisible --name '^lyyIme 截屏$' 2>/dev/null || true; } | head -1)"
    if [[ -n "$w" ]]; then
        p="$(xdotool getwindowpid "$w" 2>/dev/null || true)"
        [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
    fi
    kill "$XSD_PID" 2>/dev/null || true
    [[ -n "${XIM_PID:-}" ]] && wait "$XIM_PID" 2>/dev/null || true
    wait "$XSD_PID" 2>/dev/null || true
    exit "$rc"
}
trap cleanup EXIT
sleep 1.5
kill -0 "$XSD_PID" 2>/dev/null ||
    fail "xfsettingsd 启动失败:$(tail -5 "$WORK/xfsettingsd.log")"

# ---- xfconf 工具(读失败不静默:只有窗口轮询用 || true) ----
xq() { xfconf-query -c "$CHAN" "$@" 2>/dev/null; }
xget() { xq -p "$1"; }
xexists() { xfconf-query -c "$CHAN" -p "$1" >/dev/null 2>&1; }
xset() { # $1=prop $2=type $3=value(存在则直写,否则 --create)
    xfconf-query -c "$CHAN" -p "$1" -s "$3" 2>/dev/null ||
        xfconf-query -c "$CHAN" -p "$1" --create -t "$2" -s "$3" 2>/dev/null
}
set_shot() { # 改隔离配置里的 shot_hotkey
    mkdir -p "$(dirname "$CFG")"
    if [[ -f "$CFG" ]] && grep -q '^shot_hotkey' "$CFG"; then
        sed -i "s/^shot_hotkey = .*/shot_hotkey = \"$1\"/" "$CFG"
    else
        printf 'shot_hotkey = "%s"\n' "$1" >>"$CFG"
    fi
}
shot_cfg() { sed -n 's/^shot_hotkey = "\([^"]*\)".*/\1/p' "$CFG"; }

# ---- 截屏覆盖窗判定:只认当前 DISPLAY 上『可见且名恰为 lyyIme 截屏』的窗口
#      (不全局 pgrep —— 可能与真实会话同名进程串扰);空结果=普通轮询态,
#      各管道内建 || true 防 pipefail 把「暂无窗口」误判为脚本错误
shot_win() {
    { xdotool search --onlyvisible --name '^lyyIme 截屏$' 2>/dev/null ||
        true; } | head -1
}
wait_shot() { # 等覆盖窗出现(≤10s),stdout 输出窗口 ID
    for _ in $(seq 1 40); do
        local w; w="$(shot_win)"
        if [[ -n "$w" ]]; then echo "$w"; return 0; fi
        sleep 0.25
    done
    return 1
}
key_fires_shot() { # $1=xdotool 键序列;拉起覆盖窗则 stdout 输出窗口 ID
    xdotool key --clearmodifiers "$1"
    wait_shot
}
cancel_shot() { # $1=覆盖窗 ID;先聚焦再发 Esc(隔离显示无 WM,主动置焦)
    [[ -n "$(shot_win)" ]] || return 0
    xdotool windowfocus --sync "$1" 2>/dev/null || true
    xdotool key --clearmodifiers Escape
    for _ in $(seq 1 20); do
        [[ -z "$(shot_win)" ]] && return 0
        sleep 0.25
    done
    fail "截屏覆盖窗未随 Esc 退出"
}

# ---- 设置窗观察(驱动走 AT-SPI,这里只做窗口级断言) ----
SETW=""
settings_win() {
    { xdotool search --onlyvisible --name '^lyyIme 输入法设置$' 2>/dev/null ||
        true; } | head -1
}
msg_dlg() { # 热键校验/保存错误弹窗
    { xdotool search --onlyvisible --name '^lyyIme 快捷键设置$' 2>/dev/null ||
        true; } | head -1
}
wait_save_outcome() { # 点「确定」后 5s 内等:窗口关闭(成功)或错误弹窗(拒绝)
    for _ in $(seq 1 20); do
        [[ -z "$(settings_win)" || -n "$(msg_dlg)" ]] && return 0
        sleep 0.25
    done
    return 1
}

# ui_save OLD NEW:AT-SPI 驱动当前 XIM_PID 的设置窗——把显示 OLD 的可编辑
# 文本框改写成 NEW 并回读校验,再点「确定」按钮;10s 内找不到即报错,
# 绝不静默跳过。范围限定目标进程的无障碍树,只看 SHOWING 控件。
ui_save() {
    LYY_GH_XIM_PID="$XIM_PID" LYY_GH_OLD="$1" LYY_GH_NEW="$2" \
        python3 - <<'PYEOF'
import os, sys, time
import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

pid = int(os.environ["LYY_GH_XIM_PID"])
old = os.environ["LYY_GH_OLD"]
new = os.environ["LYY_GH_NEW"]
deadline = time.monotonic() + 10.0
read_errors = []  # 接口候选上的读异常,失败时并入诊断

def die(msg):
    if read_errors:
        msg += "(候选读异常: %s)" % "; ".join(read_errors[-3:])
    sys.stderr.write("ui_save: %s\n" % msg)
    sys.exit(1)

def walk(node):
    try:
        n = node.get_child_count()
    except Exception:
        return
    for i in range(n):
        try:
            c = node.get_child_at_index(i)
        except Exception:
            continue
        if c is None:
            continue
        yield c
        for g in walk(c):
            yield g

def showing(w):
    try:
        st = w.get_state_set()
    except Exception:
        return False
    return st is not None and st.contains(Atspi.StateType.SHOWING)

def text_of(w):
    # Accessible.get_text 是废弃包装(与 Text.get_text 撞名);
    # 判支持用 get_text_iface,真读走接口静态函数
    try:
        ti = w.get_text_iface()
    except Exception as e:
        read_errors.append("get_text_iface: %s" % e)
        return None
    if ti is None:
        return None
    try:
        return Atspi.Text.get_text(w, 0, -1)
    except Exception as e:
        read_errors.append("Text.get_text: %s" % e)
        return None

def editable(w):
    try:
        return w.get_editable_text_iface() is not None
    except Exception as e:
        read_errors.append("get_editable_text_iface: %s" % e)
        return False

def find_app():
    desk = Atspi.get_desktop(0)
    for i in range(desk.get_child_count()):
        try:
            app = desk.get_child_at_index(i)
        except Exception:
            continue
        if app is None:
            continue
        try:
            if app.get_process_id() == pid:
                return app
        except Exception:
            continue
    return None

# 1. 找截屏快捷键输入框:文本恰为 OLD 且可编辑的 SHOWING 控件
entry = None
while time.monotonic() < deadline:
    app = find_app()
    if app is not None:
        for w in walk(app):
            if showing(w) and editable(w) and text_of(w) == old:
                entry = w
                break
    if entry is not None:
        break
    time.sleep(0.25)
if entry is None:
    die("10s 内未找到文本为 %r 的可编辑输入框(pid=%d)" % (old, pid))

if entry.get_editable_text_iface() is None:
    die("候选框无 EditableText 接口")
if not Atspi.EditableText.set_text_contents(entry, new):
    die("EditableText.set_text_contents(%r) 返回失败" % new)
got = text_of(entry)
if got != new:
    die("改写后回读为 %r,期望 %r" % (got, new))

# 2. 找「确定」按钮(SHOWING 的 PUSH_BUTTON,名恰为 确定)
ok = None
while time.monotonic() < deadline:
    app = find_app()
    if app is not None:
        for w in walk(app):
            if not showing(w):
                continue
            try:
                if w.get_role() != Atspi.Role.PUSH_BUTTON:
                    continue
                if w.get_name() == "确定":
                    ok = w
                    break
            except Exception:
                continue
    if ok is not None:
        break
    time.sleep(0.25)
if ok is None:
    die("10s 内未找到「确定」按钮(pid=%d)" % pid)
if ok.get_action_iface() is None:
    die("「确定」按钮无 Action 接口")
if not Atspi.Action.do_action(ok, 0):
    die("「确定」Action.do_action(0) 返回失败")
print("ui_save: 字段 %s → %s,已点「确定」" % (old, new))
PYEOF
}

# ---- 用例 ----
mkdir -p "$(dirname "$CFG")"
printf 'shot_hotkey = "ctrl+alt+a"\n' >"$CFG"

echo "== [1/9] 初始登记 + 幂等 =="
xset '/commands/custom/override' bool true || fail "override 预置失败"
xset '/xfwm4/custom/override' bool true || fail "xfwm4 override 预置失败"
# 无关绑定:全程必须原样保留
xset '/commands/custom/<Alt>F2' string 'xfce4-appfinder --collapsed' ||
    fail "无关绑定预置失败"
"$XIM_BIN" --sync-shot-hotkey >"$WORK/sync1.out" 2>"$WORK/sync1.err" ||
    fail "首次 --sync-shot-hotkey 失败:$(cat "$WORK/sync1.err")"
[[ "$(xget '/commands/custom/<Primary><Alt>a')" == "$SHOT_BIN" ]] ||
    fail "目标属性未登记:$(xget '/commands/custom/<Primary><Alt>a')"
"$XIM_BIN" --sync-shot-hotkey >/dev/null 2>&1 ||
    fail "重复同步(幂等)失败"
[[ "$(xget '/commands/custom/<Primary><Alt>a')" == "$SHOT_BIN" ]] ||
    fail "幂等同步后属性异常"
echo "PASS 1:初始登记与幂等"

echo "== [2/9] 事务回滚 API:apply→rollback 后频道原样 =="
xq -lv | sort >"$WORK/snap_before.txt"
"$UNIT_HOTKEY" --global-rollback-test >"$WORK/rb.out" 2>"$WORK/rb.err" ||
    fail "global-rollback-test 失败:$(cat "$WORK/rb.err")"
xq -lv | sort >"$WORK/snap_after.txt"
diff -u "$WORK/snap_before.txt" "$WORK/snap_after.txt" ||
    fail "回滚后频道属性与回滚前不一致"
echo "PASS 2:回滚 API 全量还原"

echo "== [3/9] 真实触发:ctrl+alt+a → 覆盖窗 → Esc 取消 =="
sleep 1 # 等 xfsettingsd 收到 PropertyChanged 并完成抓取
W="$(key_fires_shot ctrl+alt+a)" ||
    fail "ctrl+alt+a 未拉起截屏(覆盖窗未出现)"
cancel_shot "$W"
echo "PASS 3:全局键拉起并取消"

echo "== [4/9] 改键 ctrl+alt+a → ctrl+alt+s =="
set_shot 'ctrl+alt+s'
"$XIM_BIN" --sync-shot-hotkey >/dev/null 2>"$WORK/sync4.err" ||
    fail "改键同步失败:$(cat "$WORK/sync4.err")"
xexists '/commands/custom/<Primary><Alt>a' &&
    fail "旧绑定 <Primary><Alt>a 未移除"
[[ "$(xget '/commands/custom/<Primary><Alt>s')" == "$SHOT_BIN" ]] ||
    fail "新绑定 <Primary><Alt>s 未登记"
sleep 1
W="$(key_fires_shot ctrl+alt+s)" || fail "ctrl+alt+s 未拉起截屏"
cancel_shot "$W"
xdotool key --clearmodifiers ctrl+alt+a
sleep 1.2
[[ -z "$(shot_win)" ]] ||
    { cancel_shot "$(shot_win)"; fail "旧组合 ctrl+alt+a 仍触发截屏"; }
echo "PASS 4:旧键移除、新键生效"

echo "== [5/9] <Control>≡<Primary> 别名冲突拒绝 =="
xset '/commands/custom/<Control><Alt>w' string 'echo occupied' ||
    fail "别名占用预置失败"
set_shot 'ctrl+alt+w'
if "$XIM_BIN" --sync-shot-hotkey >"$WORK/c5.out" 2>"$WORK/c5.err"; then
    fail "别名占用未被拒绝"
fi
grep -q 'custom/<Control><Alt>w' "$WORK/c5.err" ||
    fail "报错未点名冲突属性:$(cat "$WORK/c5.err")"
[[ "$(xget '/commands/custom/<Control><Alt>w')" == 'echo occupied' ]] ||
    fail "占用方属性被改写"
xexists '/commands/custom/<Primary><Alt>w' &&
    fail "冲突时仍写入了目标属性"
[[ "$(xget '/commands/custom/<Primary><Alt>s')" == "$SHOT_BIN" ]] ||
    fail "既有自有绑定被牵连改动"
echo "PASS 5:Control/Primary 别名冲突拒绝且属性原样"

echo "== [6/9] <Mod4>≡<Super> 别名冲突拒绝 =="
xset '/commands/custom/<Mod4>F2' string 'echo occupied-mod4' ||
    fail "Mod4 占用预置失败"
set_shot 'super+f2'
if "$XIM_BIN" --sync-shot-hotkey >"$WORK/c6.out" 2>"$WORK/c6.err"; then
    fail "Mod4 别名占用未被拒绝"
fi
grep -q 'custom/<Mod4>F2' "$WORK/c6.err" ||
    fail "报错未点名 Mod4 冲突属性:$(cat "$WORK/c6.err")"
xexists '/commands/custom/<Super>F2' &&
    fail "Mod4 冲突时仍写入了目标属性"
echo "PASS 6:Mod4/Super 别名冲突拒绝"

echo "== [7/9] xfwm4 窗口管理器绑定冲突拒绝 =="
xset '/xfwm4/custom/<Primary><Alt>e' string 'workspace_2_key' ||
    fail "xfwm4 占用预置失败"
set_shot 'ctrl+alt+e'
if "$XIM_BIN" --sync-shot-hotkey >/dev/null 2>"$WORK/c7.err"; then
    fail "xfwm4 占用未被拒绝"
fi
xexists '/commands/custom/<Primary><Alt>e' &&
    fail "WM 冲突时仍写入了命令属性"
echo "PASS 7:xfwm4 冲突拒绝"

echo "== [8/9] 会话总线缺失 → 非零;remove 只摘自有 =="
set_shot 'ctrl+alt+s'
if DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-lyyime-ghk' \
    "$XIM_BIN" --sync-shot-hotkey >/dev/null 2>&1; then
    fail "无会话总线仍返回成功"
fi
"$XIM_BIN" --sync-shot-hotkey >/dev/null 2>&1 || fail "恢复同步失败"
"$XIM_BIN" --remove-shot-hotkey >"$WORK/rm.out" 2>"$WORK/rm.err" ||
    fail "--remove-shot-hotkey 失败:$(cat "$WORK/rm.err")"
xexists '/commands/custom/<Primary><Alt>s' &&
    fail "自有绑定未被移除"
[[ "$(xget '/commands/custom/<Alt>F2')" == 'xfce4-appfinder --collapsed' ]] ||
    fail "无关绑定 <Alt>F2 被改动"
[[ "$(xget '/commands/custom/<Control><Alt>w')" == 'echo occupied' ]] ||
    fail "他人别名绑定 <Control><Alt>w 被改动"
[[ "$(xget '/commands/custom/<Mod4>F2')" == 'echo occupied-mod4' ]] ||
    fail "他人别名绑定 <Mod4>F2 被改动"
echo "PASS 8:总线缺失非零;remove 仅摘除自有绑定"

echo "== [9/9] 设置窗保存(UI):改字段+确定 → 配置与 XFCE 键同步;冲突拒绝 =="
# 启动真实主程序:隔离 HOME/XDG_*/DISPLAY/总线;IM 关闭(不抢键),
# LYYIME_RES_DIR 指向源码树确保吃的是本次评审的 settings.ui。
# 启动时按当前配置(ctrl+alt+s)自动登记,--settings-page 2 开在快捷键页
GTK_IM_MODULE=none XMODIFIERS='@im=none' \
    "$XIM_BIN" --settings-page 2 >"$WORK/xim.out" 2>"$WORK/xim.err" &
XIM_PID=$!
for _ in $(seq 1 40); do
    SETW="$(settings_win)"; [[ -n "$SETW" ]] && break; sleep 0.25
done
[[ -n "$SETW" ]] ||
    fail "设置窗未出现(xim.err):$(tail -5 "$WORK/xim.err" 2>/dev/null)"
[[ "$(xget '/commands/custom/<Primary><Alt>s')" == "$SHOT_BIN" ]] ||
    fail "主程序启动未自动登记 <Primary><Alt>s"

# 9a.字段 ctrl+alt+s → ctrl+alt:x,点「确定」→ 配置与 XFCE 键一起切换
ui_save "$(shot_cfg)" 'ctrl+alt+x'
wait_save_outcome || fail "点「确定」后窗口未关闭且无弹窗(点击未生效)"
[[ -z "$(msg_dlg)" ]] ||
    { xdotool key --clearmodifiers Escape; fail "合法保存被错误弹窗拒绝"; }
[[ -z "$(settings_win)" ]] || fail "保存成功后设置窗未关闭"
[[ "$(shot_cfg)" == 'ctrl+alt+x' ]] ||
    fail "配置未更新:$(shot_cfg)"
[[ "$(xget '/commands/custom/<Primary><Alt>x')" == "$SHOT_BIN" ]] ||
    fail "XFCE 未登记 <Primary><Alt>x"
xexists '/commands/custom/<Primary><Alt>s' &&
    fail "旧键 <Primary><Alt>s 未随保存移除"

# 9b.外部占用 <Primary><Alt>y → UI 改 ctrl+alt+y 保存被拒,配置与登记原样
xset '/commands/custom/<Primary><Alt>y' string 'echo busy-y' ||
    fail "占用预置失败"
printf '2' >"$XDG_DATA_HOME/lyyime/settings-page.req"
kill -USR1 "$XIM_PID"
for _ in $(seq 1 40); do
    SETW="$(settings_win)"; [[ -n "$SETW" ]] && break; sleep 0.25
done
[[ -n "$SETW" ]] || fail "设置窗未重新弹出"
ui_save "$(shot_cfg)" 'ctrl+alt+y'
wait_save_outcome || fail "冲突保存后无弹窗反馈"
D="$(msg_dlg)"
[[ -n "$D" ]] || fail "冲突保存未被拒绝(无错误弹窗出现)"
xdotool windowfocus --sync "$D" 2>/dev/null || true
xdotool key --clearmodifiers Escape # 关闭错误弹窗
sleep 0.5
[[ "$(xget '/commands/custom/<Primary><Alt>y')" == 'echo busy-y' ]] ||
    fail "占用方 <Primary><Alt>y 被改写"
[[ "$(xget '/commands/custom/<Primary><Alt>x')" == "$SHOT_BIN" ]] ||
    fail "既有登记 <Primary><Alt>x 被牵连"
[[ "$(shot_cfg)" == 'ctrl+alt+x' ]] ||
    fail "冲突拒绝后配置被改动:$(shot_cfg)"
kill "$XIM_PID" 2>/dev/null || true
wait "$XIM_PID" 2>/dev/null || true
XIM_PID=""
echo "PASS 9:设置窗保存同步与冲突拒绝"

echo "GHK-E2E-PASS: 登记/幂等/回滚/触发/改键/双别名冲突/WM 冲突/总线缺失/remove/UI 保存 全部通过 ✅"
