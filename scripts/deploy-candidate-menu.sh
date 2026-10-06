#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-/data/cargo-target/local/codes_apps_lyyIme-93ecbcd94ded0a75}"
SRC_CORE=$TARGET/release/liblyyime_core.so
SRC_IBUS=$TARGET/release/ibus-engine-lyyime
SRC_FLOAT=$TARGET/release/lyyime-float
SRC_XIM=$ROOT/xim/build/bin/lyyime-xim
SRC_SETTINGS=$ROOT/xim/res/settings.ui
DST_CORE=/usr/local/lib/lyyime/liblyyime_core.so
DST_IBUS=/home/root/.local/share/lyyime/ibus/engine/ibus-engine-lyyime
DST_XIM=/usr/local/bin/lyyime-xim
DST_FLOAT=/usr/local/bin/lyyime-float
DST_SETTINGS=/usr/local/share/lyyime/res/settings.ui
CONFIG=/home/root/.config/lyyime/config.toml
UNIT=lyyime-xim-sample.service
LOCK=/tmp/lyyime-deploy-candidate-menu.lock

DRY_RUN=0
case "${1:-}" in
    "") ;;
    --dry-run) DRY_RUN=1 ;;
    *) echo "用法: $0 [--dry-run]" >&2; exit 2 ;;
esac

MUT_CORE=0; MUT_IBUS=0; MUT_XIM=0; MUT_SETTINGS=0; MUT_FLOAT=0
XIM_STOPPED=0; MANAGED_UNIT=0; CREATED_UNIT=0; ROLLING=0
XIM_PID=""; XIM_START=""; NEW_XIM_PID=""; NEW_XIM_START=""
OLD_IBUS_SHA=""; OLD_XIM_SHA=""
declare -A SESS=()

sha() { sha256sum "$1" | cut -d' ' -f1; }
proc_state() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $1}'; }
proc_start() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }

xim_pids() {
    local p exe
    pgrep -u "$(id -u)" -x lyyime-xim 2>/dev/null | while read -r p; do
        exe="$(readlink "/proc/$p/exe" 2>/dev/null || true)"
        [[ $exe == "$DST_XIM" || $exe == "$DST_XIM (deleted)" ]] && echo "$p"
    done
}

engine_pids() {
    local p exe
    pgrep -u "$(id -u)" -f '^/[^ ]*/ibus-engine-lyyime( |$)' 2>/dev/null \
        | while read -r p; do
            exe="$(readlink "/proc/$p/exe" 2>/dev/null || true)"
            [[ $exe == "$DST_IBUS" || $exe == "$DST_IBUS (deleted)" ]] || continue
            grep -zFxq -- "DISPLAY=${SESS[DISPLAY]:-}" "/proc/$p/environ" 2>/dev/null || continue
            grep -zFxq -- "DBUS_SESSION_BUS_ADDRESS=${SESS[DBUS_SESSION_BUS_ADDRESS]:-}" \
                "/proc/$p/environ" 2>/dev/null || continue
            echo "$p"
        done
}

term_pid_checked() {
    local p="$1" start="$2" state i
    [[ -n $start && $(proc_start "$p") == "$start" ]] || return 1
    kill -TERM "$p" || return 1
    for i in $(seq 1 50); do
        state="$(proc_state "$p")"
        [[ $(proc_start "$p") != "$start" || -z $state || $state == Z ]] && return 0
        sleep 0.1
    done
    return 1
}

unit_loadstate() { systemctl show -p LoadState --value "$UNIT" 2>/dev/null || true; }
unit_active() { systemctl show -p ActiveState --value "$UNIT" 2>/dev/null || true; }
unit_mainpid() { systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true; }

unit_exec_has_xim() {
    local e path=""
    e="$(systemctl show -p ExecStart --value "$UNIT" 2>/dev/null || true)"
    [[ $e =~ path=([^[:space:];]+) ]] && path="${BASH_REMATCH[1]}"
    [[ -n $path && $path == "$DST_XIM" ]]
}

unit_env_ok() {
    local k d=0 b=0
    local -a ev=()
    read -ra ev <<<"$(systemctl show -p Environment --value "$UNIT" 2>/dev/null)"
    for k in "${ev[@]}"; do
        [[ $k == "DISPLAY=${SESS[DISPLAY]:-}" ]] && d=1
        [[ $k == "DBUS_SESSION_BUS_ADDRESS=${SESS[DBUS_SESSION_BUS_ADDRESS]:-}" ]] && b=1
    done
    (( d == 1 && b == 1 ))
}

stop_xim() {
    local i
    if (( MANAGED_UNIT == 1 )); then
        systemctl stop "$UNIT" || return 1
        for i in $(seq 1 50); do
            [[ $(unit_active) != active ]] && return 0
            sleep 0.1
        done
        return 1
    fi
    term_pid_checked "$XIM_PID" "$XIM_START"
}

start_xim() {
    local uload k
    local -a args=()
    uload="$(unit_loadstate)"
    case "$uload" in
        loaded)
            { unit_exec_has_xim && unit_env_ok; } || return 2
            systemctl start "$UNIT" ;;
        not-found|"")
            for k in HOME DISPLAY XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS \
                     XAUTHORITY LANG PATH GTK_IM_MODULE XMODIFIERS; do
                [[ -n ${SESS[$k]:-} ]] && args+=(--setenv="$k=${SESS[$k]}")
            done
            systemd-run --system --collect --unit="${UNIT%.service}" \
                --service-type=simple -p User=root \
                -p StandardOutput=journal -p StandardError=journal \
                "${args[@]}" "$DST_XIM" && CREATED_UNIT=1 ;;
        *) return 2 ;;
    esac
}

wait_new_xim() {
    local i mp
    for i in $(seq 1 100); do
        mp="$(unit_mainpid)"
        if [[ -n $mp && $mp != 0 && -d /proc/$mp ]] \
            && [[ $(proc_state "$mp") != Z ]] \
            && [[ $(sha "/proc/$mp/exe" 2>/dev/null || true) == "$EXPECT_XIM_SHA" ]] \
            && grep -qF "$DST_CORE" "/proc/$mp/maps" 2>/dev/null; then
            NEW_XIM_PID="$mp"
            NEW_XIM_START="$(proc_start "$mp")"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

new_instance_stopped() {
    local mp i
    mp="$(unit_mainpid)"
    if [[ -n $mp && $mp != 0 && -d /proc/$mp ]]; then
        if [[ -n $NEW_XIM_PID && $mp == "$NEW_XIM_PID" ]] \
            || { [[ $(sha "/proc/$mp/exe" 2>/dev/null || true) == "$EXPECT_XIM_SHA" ]] \
                 && grep -zFxq -- "DISPLAY=${SESS[DISPLAY]:-}" "/proc/$mp/environ" 2>/dev/null; }; then
            systemctl stop "$UNIT" || return 1
            for i in $(seq 1 50); do
                [[ $(unit_active) != active ]] && return 0
                sleep 0.1
            done
            return 1
        fi
        echo "  !! 回滚:$UNIT MainPID=$mp 非已核新实例,拒绝停止" >&2
        return 1
    fi
    if [[ -n $NEW_XIM_PID && -d /proc/$NEW_XIM_PID ]]; then
        term_pid_checked "$NEW_XIM_PID" "$NEW_XIM_START" || return 1
    fi
    if [[ -n $XIM_PID && -d /proc/$XIM_PID ]] \
        && [[ $(proc_start "$XIM_PID") == "$XIM_START" ]]; then
        term_pid_checked "$XIM_PID" "$XIM_START" || return 1
    fi
    return 0
}

restore_file() {
    local dst="$1" stage
    [[ -f $dst.bak ]] || { echo "  !! 缺备份 $dst.bak" >&2; return 1; }
    stage="$(mktemp "${dst}.restore.XXXXXX")" || return 1
    if ! cp -a "$dst.bak" "$stage"; then rm -f "$stage"; return 1; fi
    if ! mv -fT "$stage" "$dst"; then rm -f "$stage"; return 1; fi
}

bounce_engine() {
    local want="$1" p start i esha
    local -a pids=()
    mapfile -t pids < <(engine_pids)
    [[ ${#pids[@]} -le 1 ]] \
        || { echo "  !! 回滚:引擎进程数=${#pids[@]} 非唯一,拒绝 TERM" >&2; return 1; }
    for p in "${pids[@]}"; do
        esha="$(sha "/proc/$p/exe" 2>/dev/null || true)"
        [[ $esha == "$OLD_IBUS_SHA" || $esha == "$EXPECT_IBUS_SHA" ]] \
            || { echo "  !! 回滚:引擎 $p sha=$esha 非已知版本,拒绝 TERM" >&2; return 1; }
        start="$(proc_start "$p")"
        term_pid_checked "$p" "$start" || return 1
    done
    for i in $(seq 1 20); do
        timeout 3 ibus engine lyyime >/dev/null 2>&1 || true
        mapfile -t pids < <(engine_pids)
        if [[ ${#pids[@]} -eq 1 ]] \
            && [[ $(sha "/proc/${pids[0]}/exe" 2>/dev/null || true) == "$want" ]] \
            && [[ $(proc_state "${pids[0]}") != Z ]]; then
            return 0
        fi
        sleep .5
    done
    return 1
}

rollback() {
    (( ROLLING == 1 )) && return 0
    ROLLING=1
    trap - ERR
    set +e
    local ok=1 p rc
    local -a xp=()
    echo "== 回滚开始 ==" >&2
    if (( MUT_CORE == 1 )); then
        restore_file "$DST_CORE" || { echo "  !! core 还原失败" >&2; ok=0; }
    fi
    if (( XIM_STOPPED == 1 )); then
        new_instance_stopped || { echo "  !! 回滚:新 XIM 实例停止受阻" >&2; ok=0; }
        mapfile -t xp < <(xim_pids)
        if [[ ${#xp[@]} -ne 0 ]]; then
            echo "  !! 回滚:残留 XIM 进程 ${xp[*]} 身份未核实,不再启动防重复" >&2
            ok=0
        else
            (( MUT_XIM == 1 )) && { restore_file "$DST_XIM" || ok=0; }
            (( MUT_SETTINGS == 1 )) && { restore_file "$DST_SETTINGS" || ok=0; }
            start_xim
            rc=$?
            if [[ $rc -eq 2 ]]; then
                echo "  !! 回滚:单元存在但 ExecStart 未核实,拒绝启动(需人工恢复)" >&2
                ok=0
            elif [[ $rc -ne 0 ]]; then
                echo "  !! 回滚:旧 XIM 拉起失败(需人工恢复会话输入法)" >&2
                ok=0
            else
                local i mp good=0
                for i in $(seq 1 100); do
                    mp="$(unit_mainpid)"
                    if [[ -n $mp && $mp != 0 && -d /proc/$mp ]] \
                        && [[ $(proc_state "$mp") != Z ]] \
                        && [[ $(sha "/proc/$mp/exe" 2>/dev/null || true) == "$OLD_XIM_SHA" ]]; then
                        good=1; break
                    fi
                    sleep 0.1
                done
                (( good == 1 )) || { echo "  !! 回滚:旧 XIM 未达旧 sha" >&2; ok=0; }
            fi
        fi
    fi
    if (( MUT_FLOAT == 1 )); then
        restore_file "$DST_FLOAT" || { echo "  !! float 还原失败" >&2; ok=0; }
    fi
    if (( MUT_IBUS == 1 )); then
        restore_file "$DST_IBUS" || { echo "  !! ibus 还原失败" >&2; ok=0; }
        bounce_engine "$OLD_IBUS_SHA" || { echo "  !! ibus 引擎未回旧 sha" >&2; ok=0; }
    fi
    if (( ok == 1 )); then
        echo "== 回滚完成:已变更工件均还原 ==" >&2
    else
        echo "== 回滚未完整成功,需人工核查(见上方 !!) ==" >&2
    fi
    return 0
}

die() {
    echo "!! $*" >&2
    if (( ROLLING == 0 )) && { (( MUT_CORE == 1 || MUT_IBUS == 1 || MUT_XIM == 1 \
        || MUT_SETTINGS == 1 || MUT_FLOAT == 1 || XIM_STOPPED == 1 )); }; then
        rollback
    fi
    exit 1
}
trap 'die "line $LINENO 命令失败(rc=$?)"' ERR

exec 9>"$LOCK"
flock -n 9 || die "已有部署锁占用:$LOCK"

for v in EXPECT_CORE_SHA EXPECT_IBUS_SHA EXPECT_XIM_SHA EXPECT_FLOAT_SHA \
         EXPECT_SETTINGS_SHA; do
    [[ -n ${!v:-} ]] || die "缺少必需环境变量 $v"
    [[ ${!v} =~ ^[0-9a-f]{64}$ ]] || die "$v 非 64 位小写 hex"
done

[[ -f $SRC_CORE ]] || die "源工件缺失:$SRC_CORE"
[[ -x $SRC_IBUS ]] || die "源工件缺失/不可执行:$SRC_IBUS"
[[ -x $SRC_FLOAT ]] || die "源工件缺失/不可执行:$SRC_FLOAT"
[[ -x $SRC_XIM ]] || die "源工件缺失/不可执行:$SRC_XIM"
[[ -f $SRC_SETTINGS ]] || die "源工件缺失:$SRC_SETTINGS"
[[ $(sha "$SRC_CORE") == "$EXPECT_CORE_SHA" ]] || die "src core sha 不符(非已验证构建)"
[[ $(sha "$SRC_IBUS") == "$EXPECT_IBUS_SHA" ]] || die "src ibus sha 不符"
[[ $(sha "$SRC_FLOAT") == "$EXPECT_FLOAT_SHA" ]] || die "src float sha 不符"
[[ $(sha "$SRC_XIM") == "$EXPECT_XIM_SHA" ]] || die "src xim sha 不符"
[[ $(sha "$SRC_SETTINGS") == "$EXPECT_SETTINGS_SHA" ]] || die "src settings.ui sha 不符"

for d in "$DST_CORE" "$DST_IBUS" "$DST_XIM" "$DST_FLOAT" "$DST_SETTINGS"; do
    [[ -e $d ]] || die "安装目标缺失:$d"
    [[ ! -e $d.new ]] || die "存在预置 $d.new,拒绝覆盖(请人工核查)"
done
[[ -x $DST_XIM && -x $DST_IBUS && -x $DST_FLOAT ]] || die "安装目标二进制不可执行"
OLD_CORE_SHA="$(sha "$DST_CORE")"
OLD_XIM_SHA="$(sha "$DST_XIM")"
OLD_IBUS_SHA="$(sha "$DST_IBUS")"
OLD_FLOAT_SHA="$(sha "$DST_FLOAT")"
OLD_SETTINGS_SHA="$(sha "$DST_SETTINGS")"
CONFIG_SHA="ABSENT"; [[ -f $CONFIG ]] && CONFIG_SHA="$(sha "$CONFIG")"

mapfile -t XPIDS < <(xim_pids)
[[ ${#XPIDS[@]} -eq 1 ]] || die "已安装 XIM 进程数=${#XPIDS[@]}(要求唯一,排除测试实例)"
XIM_PID="${XPIDS[0]}"
XIM_START="$(proc_start "$XIM_PID")"
[[ -n $XIM_START ]] || die "无法冻结 XIM 进程身份"
[[ $(sha "/proc/$XIM_PID/exe" 2>/dev/null || true) == "$OLD_XIM_SHA" ]] \
    || die "运行中 XIM sha 与安装文件不一致"

[[ -z $(pgrep -u "$(id -u)" -x lyyime-float 2>/dev/null || true) ]] \
    || die "lyyime-float 正在运行,拒绝部署(需人工确认)"

while IFS= read -r -d '' kv; do
    case "${kv%%=*}" in
        HOME|DISPLAY|XDG_RUNTIME_DIR|DBUS_SESSION_BUS_ADDRESS|XAUTHORITY|\
        LANG|PATH|GTK_IM_MODULE|XMODIFIERS)
            SESS[${kv%%=*}]="${kv#*=}" ;;
    esac
done < "/proc/$XIM_PID/environ"
[[ ${SESS[HOME]:-} == /home/root ]] || die "XIM 会话 HOME≠/home/root,拒绝猜测"
[[ -n ${SESS[DISPLAY]:-} && -n ${SESS[DBUS_SESSION_BUS_ADDRESS]:-} ]] \
    || die "XIM 会话缺 DISPLAY/DBUS_SESSION_BUS_ADDRESS"
export DISPLAY="${SESS[DISPLAY]}"
export DBUS_SESSION_BUS_ADDRESS="${SESS[DBUS_SESSION_BUS_ADDRESS]}"
export HOME="${SESS[HOME]}"
if [[ -n ${SESS[XDG_RUNTIME_DIR]:-} ]]; then
    export XDG_RUNTIME_DIR="${SESS[XDG_RUNTIME_DIR]}"
else
    unset XDG_RUNTIME_DIR
fi
if [[ -n ${SESS[XAUTHORITY]:-} ]]; then
    export XAUTHORITY="${SESS[XAUTHORITY]}"
else
    unset XAUTHORITY
fi

if command -v xdpyinfo >/dev/null 2>&1; then
    timeout 5 xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 \
        || die "X11 连接握手失败(authority/显示不可达),拒绝部署"
else
    timeout 5 python3 -c '
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk
import sys
sys.exit(0 if Gtk.init_check()[0] else 1)' \
        || die "X11 连接握手失败(Gtk init_check),拒绝部署"
fi

[[ $(timeout 3 ibus engine 2>/dev/null) == lyyime ]] \
    || die "当前 ibus engine 非 lyyime,拒绝部署"

mapfile -t EPIDS < <(engine_pids)
[[ ${#EPIDS[@]} -eq 1 ]] || die "会话内 ibus 引擎进程数=${#EPIDS[@]}(要求唯一)"
[[ $(sha "/proc/${EPIDS[0]}/exe" 2>/dev/null || true) == "$OLD_IBUS_SHA" ]] \
    || die "运行中引擎 sha 与安装文件不一致"

mapfile -t SESSION_PIDS < <(
    for p in $(pgrep -u "$(id -u)" -x ibus-daemon 2>/dev/null) \
             $(pgrep -u "$(id -u)" -x ibus-ui-gtk3 2>/dev/null); do
        grep -zFxq -- "DISPLAY=${SESS[DISPLAY]}" "/proc/$p/environ" 2>/dev/null \
            && echo "$p"
    done | sort -u)
[[ ${#SESSION_PIDS[@]} -ge 1 ]] || die "未发现本会话 ibus-daemon"
SESSION_MARK="$(for p in "${SESSION_PIDS[@]}"; do
    printf '%s(%s):%s\n' "$p" "$(readlink "/proc/$p/exe" 2>/dev/null | xargs -r basename)" \
        "$(proc_start "$p")"
done | sort)"

ULOAD="$(unit_loadstate)"
UACT="$(unit_active)"
UMPID="$(unit_mainpid)"
if [[ $ULOAD == loaded && $UACT == active ]]; then
    [[ $UMPID == "$XIM_PID" ]] \
        || die "$UNIT active 但 MainPID=$UMPID ≠ 已核 XIM $XIM_PID"
    { unit_exec_has_xim && unit_env_ok; } \
        || die "$UNIT ExecStart/Environment 未核实,拒绝托管操作"
    MANAGED_UNIT=1
else
    [[ $ULOAD == not-found || ( $ULOAD == loaded && $UACT =~ ^(inactive|failed)$ ) ]] \
        || die "$UNIT 状态异常(load=$ULOAD active=$UACT),拒绝部署"
    [[ $ULOAD == loaded ]] && { { unit_exec_has_xim && unit_env_ok; } \
        || die "$UNIT ExecStart/Environment 未核实,拒绝托管操作"; }
    MANAGED_UNIT=0
fi

[[ -z $(xdotool search --all --onlyvisible --pid "$XIM_PID" \
    --name '^lyyIme 输入法设置$' 2>/dev/null || true) ]] \
    || die "存在可见设置窗(XIM pid $XIM_PID),拒绝部署"

echo "预校验通过:XIM=$XIM_PID(managed=$MANAGED_UNIT) engine=${EPIDS[0]} session=[$SESSION_MARK]"
echo "  config sha=$CONFIG_SHA"
if (( DRY_RUN == 1 )); then
    echo "--dry-run:仅预校验,不执行任何安装/重启"
    exit 0
fi

install_artifact() {
    local src="$1" dst="$2" expect="$3" flag="$4" mode stage
    cp -a "$dst" "$dst.bak" || die "备份失败:$dst.bak"
    stage="$(mktemp "${dst}.new.XXXXXX")" || die "暂存文件创建失败:$dst"
    mode="$(stat -c %a "$dst")"
    if ! install -m "$mode" "$src" "$stage"; then
        rm -f "$stage"; die "暂存失败:$dst"
    fi
    if [[ $(sha "$stage") != "$expect" ]]; then
        rm -f "$stage"; die "暂存 sha 不符:$dst"
    fi
    if ! mv -fT "$stage" "$dst"; then
        rm -f "$stage"; die "原子替换失败:$dst"
    fi
    printf -v "$flag" '%s' 1
}

install_artifact "$SRC_CORE" "$DST_CORE" "$EXPECT_CORE_SHA" MUT_CORE
echo "已原子安装 $DST_CORE(旧 ${OLD_CORE_SHA:0:12} → 新 ${EXPECT_CORE_SHA:0:12})"

CARGO_TARGET_DIR="$TARGET" \
    EXPECT_IBUS_SHA="$EXPECT_IBUS_SHA" \
    EXPECT_OLD_IBUS_SHA="$OLD_IBUS_SHA" \
    bash "$ROOT/scripts/deploy-ibus-engine.sh" \
    || die "IBus 引擎部署失败(其内部已自回滚)"
MUT_IBUS=1
echo "IBus 引擎部署完成"

XIM_STOPPED=1
stop_xim || die "XIM 停止失败"
echo "XIM 已停止(pid $XIM_PID)"

install_artifact "$SRC_XIM" "$DST_XIM" "$EXPECT_XIM_SHA" MUT_XIM
install_artifact "$SRC_SETTINGS" "$DST_SETTINGS" "$EXPECT_SETTINGS_SHA" MUT_SETTINGS
install_artifact "$SRC_FLOAT" "$DST_FLOAT" "$EXPECT_FLOAT_SHA" MUT_FLOAT
echo "XIM/settings.ui/float 已原子安装"

rc=0
start_xim || rc=$?
if [[ $rc -eq 2 ]]; then
    die "$UNIT 单元存在但 ExecStart 未核实,拒绝启动"
elif [[ $rc -ne 0 ]]; then
    die "XIM 拉起失败(systemd-run/systemctl $UNIT)"
fi
wait_new_xim || die "新 XIM 未达预期(sha/映射超时)"

mapfile -t XPIDS < <(xim_pids)
[[ ${#XPIDS[@]} -eq 1 && ${XPIDS[0]} == "$NEW_XIM_PID" ]] \
    || die "终核验:XIM 会话进程数/身份异常(${XPIDS[*]:-无} vs $NEW_XIM_PID)"

mapfile -t EPIDS < <(engine_pids)
[[ ${#EPIDS[@]} -eq 1 ]] || die "终核验:引擎进程数=${#EPIDS[@]}"
[[ $(sha "/proc/${EPIDS[0]}/exe" 2>/dev/null || true) == "$EXPECT_IBUS_SHA" ]] \
    || die "终核验:运行引擎 sha 不符"

for pair in "$DST_CORE:$EXPECT_CORE_SHA" "$DST_XIM:$EXPECT_XIM_SHA" \
            "$DST_IBUS:$EXPECT_IBUS_SHA" "$DST_FLOAT:$EXPECT_FLOAT_SHA" \
            "$DST_SETTINGS:$EXPECT_SETTINGS_SHA"; do
    [[ $(sha "${pair%%:*}") == "${pair##*:}" ]] || die "终核验:落盘 sha 不符 ${pair%%:*}"
done

CFG_NOW="ABSENT"; [[ -f $CONFIG ]] && CFG_NOW="$(sha "$CONFIG")"
[[ $CFG_NOW == "$CONFIG_SHA" ]] || die "终核验:config.toml 被改动"

SESSION_NOW="$(for p in "${SESSION_PIDS[@]}"; do
    [[ -d /proc/$p ]] && printf '%s(%s):%s\n' "$p" \
        "$(readlink "/proc/$p/exe" 2>/dev/null | xargs -r basename)" \
        "$(proc_start "$p")"
done | sort)"
[[ $SESSION_NOW == "$SESSION_MARK" ]] \
    || die "终核验:ibus-daemon/面板进程集合/身份变化"

trap - ERR
echo "PASS: 候选菜单窄部署完成(created_unit=$CREATED_UNIT)"
echo "  XIM:${XIM_PID}(旧 ${OLD_XIM_SHA:0:12}) → ${NEW_XIM_PID}(新 ${EXPECT_XIM_SHA:0:12}) 单元 $UNIT"
echo "  引擎 sha=${EXPECT_IBUS_SHA:0:12}(由 deploy-ibus-engine.sh 完成)"
echo "  core/float/settings.ui 落盘新 sha;config.toml 未变;daemon/面板 $SESSION_MARK 未动"
