#!/usr/bin/env bash
# deploy-ibus-engine.sh — 仅替换 IBus 引擎二进制(窄部署,不动 daemon/面板/XIM)
#
# 用法:
#   EXPECT_IBUS_SHA=<新构建 sha> EXPECT_OLD_IBUS_SHA=<现役安装 sha> \
#       bash scripts/deploy-ibus-engine.sh
#
# 流程:双 sha 强校验 → 冻结 XIM 会话环境 → 只读核验(引擎唯一、选中
# lyyime、面板有主、无可见设置窗)→ cp .bak + install .new + mv 原子替换
# → TERM 停引擎让 daemon 按选中组件自动拉起新二进制 → 读回核验 →
# repeat-settings 真机验证;任一失败经 ERR trap 自动回滚旧二进制。
# 全程不重启 ibus-daemon/ibus-ui-gtk3/lyyime-xim,不改 config.toml。
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$ROOT/scripts/lib-live-pids.sh"
SRC="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release/ibus-engine-lyyime"
DST=/home/root/.local/share/lyyime/ibus/engine/ibus-engine-lyyime
CONFIG=/home/root/.config/lyyime/config.toml
: "${EXPECT_IBUS_SHA:?required verified build SHA}"
: "${EXPECT_OLD_IBUS_SHA:?required installed version SHA}"
VERIFY_MODE="${LYYIME_IBUS_VERIFY_MODE:-repeat-settings}"
case "$VERIFY_MODE" in
    repeat-settings|paging|all) ;;
    *) echo "未知验证模式:$VERIFY_MODE" >&2; exit 2 ;;
esac

MUTATED=0   # mv 完成才置位;此前 die 直接退出不回滚
ROLLING=0

sha() { sha256sum "$1" | cut -d' ' -f1; }
# /proc/<pid>/stat:comm 可含空格/括号,按最后一个 ") " 切分;
# 之后 field1=state,field20=starttime
proc_state() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $1}'; }
proc_start() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }

# 面板属主:当前私有总线 GetNameOwner(GetConnectionUnixProcessID 在私有 bus
# 上为 UnknownMethod,不用);unique owner 名足以识别换主/重注册。
panel_owner() {
    local raw owner
    raw="$(timeout 3 gdbus call --address "$ADDR" --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.GetNameOwner \
        org.freedesktop.IBus.Panel 2>/dev/null)" || return 1
    owner="$(grep -o "':[0-9][0-9.]*'" <<<"$raw" | tr -d "'")"
    [[ -n $owner ]] || return 1
    printf '%s\n' "$owner"
}

# 过滤出本会话的引擎进程:exe 指向 DST(含 (deleted))且 DISPLAY/DBUS 与
# 冻结会话逐字节一致(防同 uid 其它桌面会话的同名引擎被误选)。
engine_pids() {
    local p exe
    pgrep -u "$(id -u)" -f '^/[^ ]*/ibus-engine-lyyime( |$)' 2>/dev/null \
        | while read -r p; do
            exe="$(readlink "/proc/$p/exe" 2>/dev/null || true)"
            [[ $exe == "$DST" || $exe == "$DST (deleted)" ]] || continue
            grep -zFxq -- "DISPLAY=$DISPLAY" "/proc/$p/environ" 2>/dev/null || continue
            grep -zFxq -- "DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS" \
                "/proc/$p/environ" 2>/dev/null || continue
            echo "$p"
        done
}

# 只杀"新旧两个已知 sha"的引擎进程;杀前复核 pid+starttime 身份;
# 有界 5s 等该身份消失或成 zombie。无 pkill/SIGKILL。
stop_engine() {
    local p start exe_sha state
    local -a pids=()
    mapfile -t pids < <(engine_pids)
    for p in "${pids[@]}"; do
        start="$(proc_start "$p")"; [[ -n $start ]] || return 1
        exe_sha="$(sha "/proc/$p/exe" 2>/dev/null || true)"
        [[ $exe_sha == "$EXPECT_IBUS_SHA" || $exe_sha == "$EXPECT_OLD_IBUS_SHA" ]] \
            || { echo "  !! 引擎 $p sha=$exe_sha 非已知版本,拒绝 kill" >&2; return 1; }
        [[ "$(proc_start "$p")" == "$start" ]] || return 1
        kill -TERM "$p" || return 1
        for _ in $(seq 1 50); do
            state="$(proc_state "$p")"
            [[ "$(proc_start "$p")" != "$start" || -z $state || $state == Z ]] && break
            sleep 0.1
        done
        state="$(proc_state "$p")"
        [[ "$(proc_start "$p")" == "$start" && -n $state && $state != Z ]] \
            && { echo "  !! 引擎 $p 未在 5s 内退出" >&2; return 1; }
    done
    return 0
}

# 让 daemon 按选中组件正常拉起引擎;不看 setter rc,以"读回选中 + 唯一进程
# + exe sha 匹配 + 非 zombie"为准。
select_engine() {
    local want="$1" sel p exe_sha state
    local -a pids=()
    for _ in $(seq 1 20); do
        timeout 3 ibus engine lyyime >/dev/null 2>&1 || true
        sel="$(timeout 3 ibus engine 2>/dev/null || true)"
        if [[ $sel == lyyime ]]; then
            mapfile -t pids < <(engine_pids)
            if [[ ${#pids[@]} -eq 1 ]]; then
                p="${pids[0]}"
                exe_sha="$(sha "/proc/$p/exe" 2>/dev/null || true)"
                state="$(proc_state "$p")"
                [[ $exe_sha == "$want" && -n $state && $state != Z ]] && return 0
            fi
        fi
        sleep .5
    done
    return 1
}

rollback() {
    (( ROLLING == 1 )) && return 1
    ROLLING=1
    trap - ERR
    set +e
    local ok=1 csha
    echo "== 回滚:还原 $DST ==" >&2
    if [[ -f $DST.bak ]]; then
        cp -f "$DST.bak" "$DST.new" && mv -fT "$DST.new" "$DST" \
            || { echo "  !! 还原文件失败" >&2; ok=0; }
    else
        echo "  !! $DST.bak 不存在,无法还原" >&2; ok=0
    fi
    stop_engine || { echo "  !! 回滚:停止引擎失败" >&2; ok=0; }
    select_engine "$EXPECT_OLD_IBUS_SHA" \
        || { echo "  !! 回滚:引擎未回到旧 sha" >&2; ok=0; }
    [[ "$(proc_start "$XIM_PID")" == "$XIM_START" ]] \
        || { echo "  !! XIM 进程身份变化" >&2; ok=0; }
    [[ "$(panel_owner || true)" == "$PANEL_BASE" ]] \
        || { echo "  !! 面板属主变化" >&2; ok=0; }
    csha="ABSENT"; [[ -f $CONFIG ]] && csha="$(sha "$CONFIG")"
    [[ $csha == "$CONFIG_SHA" ]] || { echo "  !! config.toml 被改动" >&2; ok=0; }
    if (( ok == 1 )); then
        echo "== 回滚完成:旧引擎已还原并运行 ==" >&2
    else
        echo "== 回滚未完整成功(见上方 !! 步骤,需人工核查) ==" >&2
    fi
    return 0
}

die() {
    echo "!! $*" >&2
    if (( MUTATED == 1 && ROLLING == 0 )); then rollback; fi
    exit 1
}
trap 'die "line $LINENO 命令失败(rc=$?)"' ERR

# ---- 预校验(零改动,失败直接 abort)----
[[ $EXPECT_IBUS_SHA =~ ^[0-9a-f]{64}$ ]] || die "EXPECT_IBUS_SHA 非 64 位小写 hex"
[[ $EXPECT_OLD_IBUS_SHA =~ ^[0-9a-f]{64}$ ]] || die "EXPECT_OLD_IBUS_SHA 非 64 位小写 hex"
[[ -x $SRC ]] || die "源工件不存在/不可执行:$SRC"
[[ "$(sha "$SRC")" == "$EXPECT_IBUS_SHA" ]] || die "源工件 sha 不符(非已验证构建)"
[[ -x $DST ]] || die "安装目标不存在/不可执行:$DST"
[[ "$(sha "$DST")" == "$EXPECT_OLD_IBUS_SHA" ]] || die "现役安装 sha 不符(版本已漂移?)"

XIM_PID="$(session_xim_pid)" || die "当前用户下需有且仅有一个已安装的 XIM 进程"
XIM_START="$(proc_start "$XIM_PID")"
[[ -n $XIM_START ]] || die "无法冻结 XIM 进程身份"

# 只收割白名单会话变量(不落日志,防泄)
while IFS= read -r -d '' kv; do
    case "$kv" in
        HOME=*|DISPLAY=*|XDG_RUNTIME_DIR=*|DBUS_SESSION_BUS_ADDRESS=*|\
        XAUTHORITY=*|PATH=*|LANG=*|LC_ALL=*) export "$kv" ;;
    esac
done < "/proc/$XIM_PID/environ"
unset IBUS_ADDRESS
[[ -n ${DISPLAY:-} && -n ${DBUS_SESSION_BUS_ADDRESS:-} ]] \
    || die "XIM 会话缺 DISPLAY/DBUS_SESSION_BUS_ADDRESS"

CONFIG_SHA="ABSENT"
[[ -f $CONFIG ]] && CONFIG_SHA="$(sha "$CONFIG")"
[[ "$(timeout 3 ibus engine 2>/dev/null)" == lyyime ]] \
    || die "当前 ibus engine 非 lyyime,拒绝部署"
ADDR="$(timeout 3 ibus address 2>/dev/null)"
[[ -n $ADDR ]] || die "无法获取当前私有总线地址"
PANEL_BASE="$(panel_owner)" || die "IBus.Panel 无主,拒绝部署"

mapfile -t PIDS < <(engine_pids)
[[ ${#PIDS[@]} -eq 1 ]] || die "会话内引擎进程数=${#PIDS[@]}(要求唯一)"
OLD_ENGINE_PID="${PIDS[0]}"
OLD_ENGINE_START="$(proc_start "$OLD_ENGINE_PID")"
[[ -n $OLD_ENGINE_START ]] || die "无法冻结引擎进程身份"
[[ "$(sha "/proc/$OLD_ENGINE_PID/exe")" == "$EXPECT_OLD_IBUS_SHA" ]] \
    || die "运行中引擎 sha 与安装文件不一致"

# 用户正开着设置窗时不部署(避免 WM_DELETE 验证误关用户窗口)
[[ -z "$(xdotool search --all --onlyvisible --pid "$XIM_PID" --name '^lyyIme 输入法设置$' 2>/dev/null || true)" ]] \
    || die "存在可见设置窗(XIM pid $XIM_PID),拒绝部署"

echo "预校验通过:XIM=$XIM_PID engine=$OLD_ENGINE_PID panel=$PANEL_BASE"
echo "新引擎 sha=$EXPECT_IBUS_SHA 旧 sha=$EXPECT_OLD_IBUS_SHA"

# ---- 原子替换(从此处起任何失败自动回滚)----
cp -a "$DST" "$DST.bak" || die "备份失败:$DST.bak"
install -m755 "$SRC" "$DST.new" || die "暂存失败:$DST.new"
[[ "$(sha "$DST.new")" == "$EXPECT_IBUS_SHA" ]] || { rm -f "$DST.new"; die "暂存 sha 不符"; }
MUTATED=1
mv -fT "$DST.new" "$DST" || die "原子替换失败"
echo "已原子安装 $DST(备份 $DST.bak)"

stop_engine || die "旧引擎未在时限内退出"
select_engine "$EXPECT_IBUS_SHA" || die "新引擎未被 daemon 拉起/接管"

# 周边不变核验
[[ "$(proc_start "$XIM_PID")" == "$XIM_START" ]] || die "XIM 进程身份变化"
[[ "$(panel_owner || true)" == "$PANEL_BASE" ]] || die "面板属主变化"
CFG_NOW="ABSENT"; [[ -f $CONFIG ]] && CFG_NOW="$(sha "$CONFIG")"
[[ $CFG_NOW == "$CONFIG_SHA" ]] || die "config.toml 被改动"

# 真机验证:3 轮「ymlf→设置→F7 开设置窗→WM_DELETE 关闭」;
# 失败经 ERR trap 回滚引擎二进制。
if [[ $VERIFY_MODE == all ]]; then
    bash "$ROOT/scripts/verify-desktop-input.sh" ymlf 设置 '匹配.*设置.*F7' paging
    bash "$ROOT/scripts/verify-desktop-input.sh" ymlf 设置 '匹配.*设置.*F7' repeat-settings
else
    bash "$ROOT/scripts/verify-desktop-input.sh" ymlf 设置 '匹配.*设置.*F7' "$VERIFY_MODE"
fi

# 终核验:运行体/落盘均为新 sha
mapfile -t PIDS < <(engine_pids)
[[ ${#PIDS[@]} -eq 1 ]] || die "终核验:引擎进程数异常"
NEW_ENGINE_PID="${PIDS[0]}"
[[ "$(sha "/proc/$NEW_ENGINE_PID/exe")" == "$EXPECT_IBUS_SHA" ]] || die "运行引擎 sha 不符"
[[ "$(sha "$DST")" == "$EXPECT_IBUS_SHA" ]] || die "落盘引擎 sha 不符"

MUTATED=0
trap - ERR
echo "PASS: 引擎部署+真机验证通过"
echo "  引擎进程:${OLD_ENGINE_PID}(旧 sha ${EXPECT_OLD_IBUS_SHA:0:12}…) → ${NEW_ENGINE_PID}(新 sha ${EXPECT_IBUS_SHA:0:12}…)"
echo "  config.toml 未变;XIM $XIM_PID 与面板 $PANEL_BASE 未动"
