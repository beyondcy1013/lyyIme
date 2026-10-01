#!/usr/bin/env bash
# deploy-menu-trigger.sh — 菜单触发特性安装级部署(Install-Only)
#
# 只安装已验证产物并受控重启,不构建、不跑测试。工件哈希即权威:
#   EXPECT_XIM_SHA   = xim/build/bin/lyyime-xim                 → /usr/local/bin/lyyime-xim
#   EXPECT_CORE_SHA  = $CARGO_TARGET_DIR/release/liblyyime_core.so
#                                                            → /usr/local/lib/lyyime/liblyyime_core.so
#   EXPECT_IBUS_SHA  = $CARGO_TARGET_DIR/release/ibus-engine-lyyime
#                                                            → ~/.local/share/lyyime/ibus/engine/ibus-engine-lyyime
#   另装资源(无需哈希):xim/res/{settings.ui,candidate.css} → /usr/local/share/lyyime/res/
#
# 用法(lead 下发权威哈希):
#   EXPECT_XIM_SHA=… EXPECT_CORE_SHA=… EXPECT_IBUS_SHA=… \
#       bash scripts/deploy-menu-trigger.sh
#
# 保证:
#   - 校验全在运行态改动之前;用户 config.toml 字节级不动(哈希前后比对);
#   - 旧目标各留一份固定 .bak;原子 .new+mv 安装(core .so 不原地截断防 SIGBUS);
#   - XIM 经 systemctl stop + systemd-run 同名 unit 复用旧 /proc 会话环境重启;
#   - IBus 经官方 `ibus restart`,引擎选择以实际读回为准(不信退出码);
#   - 部署后强制核验候选面板(org.freedesktop.IBus.Panel 有主)——缺面板
#     即候选窗不可见(2026-09-30 用户实证);缺则独立 transient unit 补齐,
#     已有健康面板时零改动;
#     失败自动回滚 .bak + ibus restart 重载还原后的二进制 + 恢复原选择。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 回滚门控:预校验期失败直接退出;进入可回滚区后 die/ERR 都走 rollback。
MUTATION_STARTED=0
ROLLING_BACK=0

note() { echo "[deploy] $*"; }
sha() { sha256sum "$1" 2>/dev/null | awk '{print $1}'; }

# ---- 权威入参(必传) ----
EXPECT_XIM_SHA="${EXPECT_XIM_SHA:?缺 EXPECT_XIM_SHA(xim/build/bin/lyyime-xim 的 sha256)}"
EXPECT_CORE_SHA="${EXPECT_CORE_SHA:?缺 EXPECT_CORE_SHA(liblyyime_core.so 的 sha256)}"
EXPECT_IBUS_SHA="${EXPECT_IBUS_SHA:?缺 EXPECT_IBUS_SHA(ibus-engine-lyyime 的 sha256)}"

# ---- 路径表(与 install.sh / install-all.sh 既定布局一致) ----
CARGO_REL="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}/release"
SRC_XIM="$ROOT/xim/build/bin/lyyime-xim"
SRC_CORE="$CARGO_REL/liblyyime_core.so"
SRC_IBUS="$CARGO_REL/ibus-engine-lyyime"
SRC_UI="$ROOT/xim/res/settings.ui"
SRC_CSS="$ROOT/xim/res/candidate.css"

DST_XIM=/usr/local/bin/lyyime-xim
DST_CORE=/usr/local/lib/lyyime/liblyyime_core.so
DST_IBUS=/home/root/.local/share/lyyime/ibus/engine/ibus-engine-lyyime
DST_UI=/usr/local/share/lyyime/res/settings.ui
DST_CSS=/usr/local/share/lyyime/res/candidate.css

CONFIG=/home/root/.config/lyyime/config.toml
UNIT=lyyime-xim-sample.service
LOG_FILE=/tmp/lyyime-xim-deploy.log        # unit stdout(追加,保留历史)
PID_FILE=/tmp/lyyime-xim-deploy.pid
XIM_LOG=/home/root/.local/share/lyyime/logs/xim.log   # 进程内 lyy_log(启动判定用)

declare -A SES=()          # 收割自现役 XIM /proc/environ,全程冻结
declare -a BACKED_UP=()
OLD_XIM_PID=0; OLD_ENGINE=""
OLD_XIM_UNMANAGED=0      # 0=systemd 托管;1=桌面自动启动(显式指定 PID)
OLD_XIM_START=""         # /proc/PID/stat starttime 冻结身份,防 PID 复用误伤

proc_start() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
old_xim_alive() {   # 同 PID 且同 starttime 才算"还是原来那个进程"
    [[ -n "$OLD_XIM_START" && "$(proc_start "$OLD_XIM_PID")" == "$OLD_XIM_START" ]]
}

# ============================================================
#  helpers(全部先于运行阶段定义;die/rollback 在预校验期不互调)
# ============================================================
die() {
    echo "DEPLOY-FAIL: $*" >&2
    (( MUTATION_STARTED == 1 )) && rollback
    exit 1
}

# IBus 会话命令一律用冻结的会话环境;剥掉 worker 可能继承的脏 IBUS_ADDRESS
ibus_env() {
    local -a e=(-u IBUS_ADDRESS
        "HOME=${SES[HOME]}" "DISPLAY=${SES[DISPLAY]}"
        "XDG_RUNTIME_DIR=${SES[XDG_RUNTIME_DIR]}"
        "DBUS_SESSION_BUS_ADDRESS=${SES[DBUS_SESSION_BUS_ADDRESS]}")
    [[ -n "${SES[XAUTHORITY]:-}" ]] && e+=("XAUTHORITY=${SES[XAUTHORITY]}")
    env "${e[@]}" "$@"
}

# 引擎选择恢复:实际总线读回为准,不信单次退出码。
# 有界 20 次 ×0.5s;每次 set 与读回都留痕;任一 CLI 卡死由 timeout 3 切断。
restore_ibus_selection() {
    local want="$1" i rc cur
    for i in $(seq 1 20); do
        rc=0
        ibus_env timeout 3 ibus engine "$want" >>"$LOG_FILE" 2>&1 || rc=$?
        echo "[deploy] ibus engine $want → rc=$rc(第 $i 次)" >>"$LOG_FILE"
        cur="$(ibus_env timeout 3 ibus engine 2>/dev/null || true)"
        echo "[deploy]   读回:'${cur:-<空>}'" >>"$LOG_FILE"
        [[ $cur == "$want" ]] && return 0
        sleep 0.5
    done
    return 1
}

ibus_wait_ready() { # 有界等 ibus-daemon 就绪且 lyyime 引擎在册(无 SIGPIPE)
    local list
    for _ in $(seq 1 20); do
        list="$(ibus_env timeout 3 ibus list-engine 2>>"$LOG_FILE" || true)"
        grep -q 'lyyime' <<<"$list" && return 0
        sleep 1
    done
    return 1
}

PANEL_UNIT=lyyime-ibus-panel.service
PANEL_BIN=/usr/libexec/ibus-ui-gtk3

panel_owner() { # 当前私有总线上 org.freedesktop.IBus.Panel 是否有主
    local addr="$1"
    timeout 3 gdbus call --address "$addr" --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner org.freedesktop.IBus.Panel \
        2>/dev/null
}

# 5s 观察窗内持续持有总线名才视为稳定(覆盖"注册数秒后崩溃"的迟发故障)
panel_stable() {
    local addr="$1"
    for _ in $(seq 1 20); do
        [[ "$(panel_owner "$addr" || true)" == "(true,)" ]] || return 1
        sleep 0.25
    done
}

ensure_ibus_panel() { # 候选窗靠 panel:无主时以独立 unit 补齐,有主零改动
    local addr owner
    addr="$(ibus_env timeout 3 ibus address 2>/dev/null || true)"
    [[ -n $addr ]] || { echo "  !! 无法取当前 ibus address" >&2; return 1; }
    owner="$(panel_owner "$addr" || true)"
    if [[ $owner == "(true,)" ]] && panel_stable "$addr"; then
        note "IBus panel 已有主且稳定($PANEL_UNIT 现存即留用,不动)"
        return 0
    fi
    note "IBus panel 无主/不稳(owner=$owner)——独立 unit 拉起 $PANEL_BIN"
    [[ -x $PANEL_BIN ]] || { echo "  !! $PANEL_BIN 不存在" >&2; return 1; }
    # 清掉可能残留旧总线的同名服务(只动我们自己的 unit,不碰其他面板)
    if systemctl cat "$PANEL_UNIT" >/dev/null 2>&1; then
        systemctl stop "$PANEL_UNIT" 2>/dev/null || true
        for _ in $(seq 1 40); do   # 有界 4s 等同名 unit 回收,否则撞名
            [[ "$(systemctl show "$PANEL_UNIT" -p MainPID --value 2>/dev/null)" == "0" ]] && break
            sleep 0.1
        done
        [[ "$(systemctl show "$PANEL_UNIT" -p MainPID --value 2>/dev/null)" == "0" ]]             || { echo "  !! 旧 $PANEL_UNIT 未在时限内回收" >&2; return 1; }
    fi
    local -a penv=(
        "--setenv=HOME=${SES[HOME]}" "--setenv=DISPLAY=${SES[DISPLAY]}"
        "--setenv=XDG_RUNTIME_DIR=${SES[XDG_RUNTIME_DIR]}"
        "--setenv=DBUS_SESSION_BUS_ADDRESS=${SES[DBUS_SESSION_BUS_ADDRESS]}"
        "--setenv=IBUS_ADDRESS=$addr" "--setenv=GTK_IM_MODULE=ibus")
    [[ -n "${SES[XAUTHORITY]:-}" ]] \
        && penv+=("--setenv=XAUTHORITY=${SES[XAUTHORITY]}")
    systemd-run --unit "$PANEL_UNIT" --collect --property=Type=simple \
        --property=Restart=on-failure --property=RestartSec=1 \
        "${penv[@]}" "$PANEL_BIN" </dev/null >/dev/null || return 1
    for _ in $(seq 1 40); do   # 有界 10s 等接管;接管后还须 5s 持续稳定
        owner="$(panel_owner "$addr" || true)"
        if [[ $owner == "(true,)" ]] && panel_stable "$addr"; then
            note "IBus panel 已稳定接管:$addr"
            return 0
        fi
        sleep 0.25
    done
    journalctl -u "$PANEL_UNIT" -n 30 --no-pager >&2 || true
    echo "  !! IBus panel 10s 内未接管" >&2
    return 1
}

backup_dst() {   # 固定 .bak 约定:只替换当前一份,不留时间戳堆
    local dst="$1"
    [[ -f $dst ]] || return 0          # 目标不存在→无需备份,安全返回
    cp -a "$dst" "$dst.bak" || die "备份失败:$dst"
    BACKED_UP+=("$dst")
}

atomic_install() { # 同目录 .new 暂存 + sha 复核 + mv(不原地截断旧映射文件)
    local src="$1" dst="$2" mode="$3" want="${4:-}"
    cp -f "$src" "$dst.new" && chmod "$mode" "$dst.new" \
        || die "暂存失败:$dst.new"
    if [[ -n $want && "$(sha "$dst.new")" != "$want" ]]; then
        rm -f "$dst.new"; die "暂存文件 sha 不符:$dst"
    fi
    mv -fT "$dst.new" "$dst" || die "原子替换失败:$dst"
}

wait_unit_idle() { # --collect unit 的 stop/回收异步:等有界 inactive+无 MainPID
    local mp st
    for _ in $(seq 1 100); do
        mp="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null)"
        st="$(systemctl show -p ActiveState --value "$UNIT" 2>/dev/null)"
        if [[ -z $mp || $mp == 0 ]] && [[ $st != active && $st != activating ]]; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

start_xim() {    # 同名 transient unit + 冻结的原会话环境 + 显式 core 路径
    wait_unit_idle || return 1
    systemctl reset-failed "$UNIT" 2>/dev/null || true   # 清残留 failed 态
    local -a envargs=(
        "--setenv=HOME=${SES[HOME]}"
        "--setenv=DISPLAY=${SES[DISPLAY]}"
        "--setenv=DBUS_SESSION_BUS_ADDRESS=${SES[DBUS_SESSION_BUS_ADDRESS]}"
        "--setenv=XDG_RUNTIME_DIR=${SES[XDG_RUNTIME_DIR]}"
        "--setenv=GTK_IM_MODULE=${SES[GTK_IM_MODULE]}"
        "--setenv=XMODIFIERS=${SES[XMODIFIERS]}"
        "--setenv=LYYIME_CORE_LIB=$DST_CORE"   # 加载工件无歧义
    )
    for v in XDG_CONFIG_HOME XDG_DATA_HOME LYYIME_DATA_DIR XAUTHORITY; do
        [[ -n "${SES[$v]:-}" ]] && envargs+=("--setenv=$v=${SES[$v]}")
    done
    mkdir -p "$(dirname "$LOG_FILE")"
    systemd-run --unit "$UNIT" --collect --property=Type=simple \
        "${envargs[@]}" \
        /bin/sh -c "exec $DST_XIM >>'$LOG_FILE' 2>&1" \
        </dev/null >/dev/null || return 1
    local mp
    for _ in $(seq 1 100); do   # 有界等 MainPID,不在 unit 活动期盲睡
        mp="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null)"
        if [[ $mp =~ ^[0-9]+$ && $mp -gt 0 ]] && kill -0 "$mp" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

stop_original_xim() { # 托管走 systemctl stop;显式指定的自动启动进程走 TERM
    if (( OLD_XIM_UNMANAGED == 1 )); then
        if old_xim_alive; then
            local exe
            exe="$(readlink "/proc/$OLD_XIM_PID/exe" 2>/dev/null)"
            [[ $exe == "$DST_XIM" || $exe == "$DST_XIM (deleted)" ]] || return 1
            kill -TERM "$OLD_XIM_PID" || return 1
        fi
    else
        systemctl stop "$UNIT" || return 1
        wait_unit_idle || return 1
    fi
    for _ in $(seq 1 50); do
        old_xim_alive || return 0
        sleep 0.1
    done
    return 1
}

rollback() {     # 每步计成败;任何一步失手如实报"回滚未完整成功"
    (( MUTATION_STARTED == 1 && ROLLING_BACK == 0 )) || return 0
    ROLLING_BACK=1
    trap - ERR
    set +e
    local ok=1 dst
    echo "DEPLOY-FAIL: 开始回滚(.bak 原子还原 + 同会话重启)" >&2

    # 先停现役 unit:新二进制可能正被运行进程映射,停服后才能干净还原
    systemctl stop "$UNIT" 2>/dev/null
    wait_unit_idle || { echo "  !! unit 未在时限内空闲" >&2; ok=0; }

    for dst in "${BACKED_UP[@]}"; do
        if [[ -f $dst.bak ]]; then
            cp -f "$dst.bak" "$dst.new" 2>/dev/null \
                && mv -fT "$dst.new" "$dst" \
                && echo "  restored: $dst" >&2 \
                || { echo "  !! 还原失败:$dst" >&2; ok=0; }
        fi
    done

    if old_xim_alive; then
        echo "  原 XIM 仍在运行;保持原进程" >&2
    else
        start_xim || { echo "  !! 回滚重启 XIM 失败,日志见 $LOG_FILE" >&2; ok=0; }
    fi

    # IBus 必须 restart 让守护进程重载已还原的引擎二进制(不是只切选择)
    ibus_env timeout 15 ibus restart >>"$LOG_FILE" 2>&1 \
        || { echo "  !! 回滚 ibus restart 失败" >&2; ok=0; }
    ibus_wait_ready || { echo "  !! 回滚后 ibus 未就绪" >&2; ok=0; }
    ensure_ibus_panel || { echo "  !! 回滚后 IBus panel 未恢复" >&2; ok=0; }
    restore_ibus_selection "$OLD_ENGINE" \
        || { echo "  !! 回滚恢复引擎 '$OLD_ENGINE' 失败" >&2; ok=0; }

    if (( ok == 1 )); then
        echo "DEPLOY-FAIL: 回滚完成" >&2
    else
        echo "DEPLOY-FAIL: 回滚未完整成功(见上方 !! 步骤,.bak 均保留)" >&2
    fi
}
trap 'rollback; exit 1' ERR

[[ $EXPECT_XIM_SHA =~ ^[0-9a-f]{64}$ ]]  || die "EXPECT_XIM_SHA 非 sha256"
[[ $EXPECT_CORE_SHA =~ ^[0-9a-f]{64}$ ]] || die "EXPECT_CORE_SHA 非 sha256"
[[ $EXPECT_IBUS_SHA =~ ^[0-9a-f]{64}$ ]] || die "EXPECT_IBUS_SHA 非 sha256"

# ============================================================
# 阶段 0:运行态改动之前的全量校验与现场留证
# ============================================================
note "阶段0:校验工件、会话环境与现行引擎"

for f in "$SRC_XIM" "$SRC_CORE" "$SRC_IBUS" "$SRC_UI" "$SRC_CSS"; do
    [[ -f $f ]] || die "源工件缺失:$f"
done
[[ "$(sha "$SRC_XIM")" == "$EXPECT_XIM_SHA" ]] \
    || die "xim 源 sha 不符:$(sha "$SRC_XIM") != $EXPECT_XIM_SHA"
[[ "$(sha "$SRC_CORE")" == "$EXPECT_CORE_SHA" ]] \
    || die "core 源 sha 不符:$(sha "$SRC_CORE") != $EXPECT_CORE_SHA"
[[ "$(sha "$SRC_IBUS")" == "$EXPECT_IBUS_SHA" ]] \
    || die "ibus 源 sha 不符:$(sha "$SRC_IBUS") != $EXPECT_IBUS_SHA"
note "工件哈希三件核对通过"

# 现役 XIM 定位:优先托管 unit MainPID;无托管则须显式
# LYYIME_EXISTING_XIM_PID 指定桌面自动启动进程(且做四重防伪)
OLD_XIM_PID="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true)"
if [[ ! $OLD_XIM_PID =~ ^[0-9]+$ || $OLD_XIM_PID -eq 0 ]]; then
    OLD_XIM_PID="${LYYIME_EXISTING_XIM_PID:-}"
    [[ $OLD_XIM_PID =~ ^[0-9]+$ && $OLD_XIM_PID -gt 1 ]] \
        || die "无托管 XIM;须显式指定 LYYIME_EXISTING_XIM_PID"
    # Never bypass a service manager for a service-owned process.
    [[ -r /proc/$OLD_XIM_PID/cgroup ]] || die "XIM PID 已失效"
    if grep -Eq '\.service(/|$)' "/proc/$OLD_XIM_PID/cgroup"; then
        die "指定 PID 由其它 service 托管,拒绝直接停止"
    fi
    [[ "$(stat -c %u "/proc/$OLD_XIM_PID")" == "$(id -u)" ]] \
        || die "XIM PID 用户不匹配"
    parent="$(awk '/^PPid:/ {print $2}' "/proc/$OLD_XIM_PID/status")"
    [[ "$(cat "/proc/$parent/comm" 2>/dev/null)" == xfce4-session ]] \
        || die "指定 XIM 非桌面自动启动进程"
    OLD_XIM_UNMANAGED=1
fi
[[ "$(readlink "/proc/$OLD_XIM_PID/exe" 2>/dev/null)" == "$DST_XIM" ]] \
    || die "指定 PID 非安装路径 XIM($DST_XIM)"
OLD_XIM_START="$(proc_start "$OLD_XIM_PID")"
[[ -n $OLD_XIM_START ]] || die "无法冻结 XIM 进程身份"
note "现役 XIM:pid=$OLD_XIM_PID unmanaged=$OLD_XIM_UNMANAGED"

# 从现役进程 /proc/environ 原样收割会话变量(不含则不带;部署全程冻结)
while IFS= read -r line; do
    case "$line" in
        HOME=*|DISPLAY=*|DBUS_SESSION_BUS_ADDRESS=*|XDG_RUNTIME_DIR=*|\
        GTK_IM_MODULE=*|XMODIFIERS=*|XDG_CONFIG_HOME=*|XDG_DATA_HOME=*|\
        LYYIME_DATA_DIR=*|XAUTHORITY=*) SES["${line%%=*}"]="${line#*=}" ;;
    esac
done < <(tr '\0' '\n' < "/proc/$OLD_XIM_PID/environ" 2>/dev/null || true)
for v in HOME DISPLAY DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR GTK_IM_MODULE XMODIFIERS; do
    [[ -n "${SES[$v]:-}" ]] || die "现役 XIM 环境缺 $v,拒绝臆造会话"
done
note "会话环境:DISPLAY=${SES[DISPLAY]} GTK_IM_MODULE=${SES[GTK_IM_MODULE]} XMODIFIERS=${SES[XMODIFIERS]}"

OLD_ENGINE="$(ibus_env timeout 3 ibus engine 2>/dev/null || true)"
[[ -n $OLD_ENGINE ]] \
    || die "无法捕获当前 IBus 选定引擎(拒绝盲重启破坏用户选择)"
note "当前 IBus 选定引擎:'$OLD_ENGINE'(部署后原样恢复)"

CONFIG_SHA_BEFORE="ABSENT"
[[ -f $CONFIG ]] && CONFIG_SHA_BEFORE="$(sha "$CONFIG")"
note "config.toml sha=${CONFIG_SHA_BEFORE:0:16}…(部署后需完全一致)"

# ============================================================
# 阶段 1:备份 + 原子安装(此步起进入可回滚区)
# ============================================================
MUTATION_STARTED=1
note "阶段1:.bak 备份与原子安装"
backup_dst "$DST_XIM";  backup_dst "$DST_CORE"; backup_dst "$DST_IBUS"
backup_dst "$DST_UI";   backup_dst "$DST_CSS"

mkdir -p "$(dirname "$DST_CORE")" "$(dirname "$DST_IBUS")" "$(dirname "$DST_UI")"
atomic_install "$SRC_XIM"  "$DST_XIM"  0755 "$EXPECT_XIM_SHA"
atomic_install "$SRC_CORE" "$DST_CORE" 0644 "$EXPECT_CORE_SHA"
atomic_install "$SRC_IBUS" "$DST_IBUS" 0755 "$EXPECT_IBUS_SHA"
atomic_install "$SRC_UI"   "$DST_UI"   0644
atomic_install "$SRC_CSS"  "$DST_CSS"  0644
note "安装落位:xim=$(sha "$DST_XIM" | cut -c1-12) core=$(sha "$DST_CORE" | cut -c1-12) ibus=$(sha "$DST_IBUS" | cut -c1-12)"

# ============================================================
# 阶段 2:XIM 受控重启(systemctl stop 优雅停;同名 unit 起)
# ============================================================
note "阶段2:重启 XIM(systemd 托管,复用原会话环境)"
MARK="$(wc -l <"$XIM_LOG" 2>/dev/null || echo 0)"   # 启动日志判定基线
stop_original_xim || die "旧 XIM($OLD_XIM_PID)未在时限内退出"
start_xim || die "新 XIM unit 未拉起,日志见 $LOG_FILE"
NEW_XIM_PID="$(systemctl show -p MainPID --value "$UNIT")"
printf '%s\n' "$NEW_XIM_PID" >"$PID_FILE"
note "新 XIM:pid=$NEW_XIM_PID($UNIT active)"

# ============================================================
# 阶段 3:IBus 引擎原子替换已完成,ibus restart + 恢复选定引擎
# ============================================================
note "阶段3:ibus restart 并恢复原选定引擎"
ibus_env timeout 15 ibus restart >>"$LOG_FILE" 2>&1 \
    || die "ibus restart 失败"
ibus_wait_ready || die "ibus restart 后 20s 内 list-engine 未见 lyyime"
ensure_ibus_panel || die "IBus panel 未接管(候选窗将不可见)"
restore_ibus_selection "$OLD_ENGINE" \
    || die "恢复原选定引擎 '$OLD_ENGINE' 失败(读回不符,日志见 $LOG_FILE)"
[[ $OLD_ENGINE == lyyime ]] \
    && note "IBus 引擎已恢复:lyyime" \
    || note "IBus 引擎恢复原选择 '$OLD_ENGINE'(本部署不切换)"

# ============================================================
# 阶段 4:运行态核验
# ============================================================
note "阶段4:运行态核验"
[[ "$(systemctl is-active "$UNIT")" == active ]] || die "$UNIT 非 active"
[[ "$(sha "/proc/$NEW_XIM_PID/exe")" == "$EXPECT_XIM_SHA" ]] \
    || die "新 XIM 运行体 sha 不符"
grep -Fq "$DST_CORE" "/proc/$NEW_XIM_PID/maps" \
    || die "新 XIM 未映射 $DST_CORE"
grep -F "$DST_CORE" "/proc/$NEW_XIM_PID/maps" | grep '(deleted)' >/dev/null \
    && die "新 XIM 映射到已删除的 core 副本"
[[ "$(sha "$DST_CORE")" == "$EXPECT_CORE_SHA" ]] || die "core 落位 sha 不符"

# 启动日志:基线之后须见 已加载core / 菜单触发默认档 / 非降级主循环
ok=0
for _ in $(seq 1 30); do
    tail -n +"$((MARK + 1))" "$XIM_LOG" 2>/dev/null | grep '进入主循环' >/dev/null \
        && { ok=1; break; }
    sleep 1
done
[[ $ok -eq 1 ]] || die "新 XIM 30s 内未进入主循环($XIM_LOG)"
tail -n +"$((MARK + 1))" "$XIM_LOG" | grep 'core 库已加载' >/dev/null \
    || die "新 XIM 日志缺 'core 库已加载'"
tail -n +"$((MARK + 1))" "$XIM_LOG" | grep '菜单触发:enabled=1 key=F7' >/dev/null \
    || die "新 XIM 菜单触发非预期默认档(应为 enabled=1 key=F7)"
tail -n +"$((MARK + 1))" "$XIM_LOG" | grep '进入主循环(降级=0)' >/dev/null \
    || die "新 XIM 启动降级(降级!=0)"
note "XIM 核验通过:pid=$NEW_XIM_PID,菜单触发 enabled=1 key=F7,降级=0"

# IBus 运行体:10s 内轮询,sha 匹配且 DISPLAY/HOME 同捕获会话(防串台)
ibus_pid=""
for _ in $(seq 1 20); do
    for p in $(pgrep -u 0 -f 'ibus-engine-lyyime' 2>/dev/null || true); do
        [[ "$(readlink "/proc/$p/exe" 2>/dev/null)" == "$DST_IBUS" ]] || continue
        grep -qz "^DISPLAY=${SES[DISPLAY]}$" "/proc/$p/environ" 2>/dev/null || continue
        grep -qz "^HOME=${SES[HOME]}$" "/proc/$p/environ" 2>/dev/null || continue
        [[ "$(sha "/proc/$p/exe")" == "$EXPECT_IBUS_SHA" ]] \
            && { ibus_pid="$p"; break 2; }
    done
    sleep 0.5
done
[[ -n $ibus_pid ]] \
    || die "10s 内未见 sha/会话匹配的 ibus-engine-lyyime 进程"
note "IBus 核验通过:pid=$ibus_pid 运行新引擎(会话吻合)"

# 面板健康复核:当前私有总线上 IBus.Panel 必须持续有主(缺=候选窗不可见)
ensure_ibus_panel || die "部署后 IBus.Panel 未稳定连接"
PANEL_ADDR="$(ibus_env timeout 3 ibus address 2>/dev/null || true)"
[[ -n $PANEL_ADDR && "$(panel_owner "$PANEL_ADDR" || true)" == "(true,)" ]] \
    || die "部署后 IBus.Panel 无主,候选窗不可用"

CONFIG_SHA_AFTER="ABSENT"
[[ -f $CONFIG ]] && CONFIG_SHA_AFTER="$(sha "$CONFIG")"
[[ "$CONFIG_SHA_AFTER" == "$CONFIG_SHA_BEFORE" ]] \
    || die "config.toml 哈希变动($CONFIG_SHA_BEFORE → $CONFIG_SHA_AFTER)"

trap - ERR
echo "================ DEPLOY OK ================"
echo "XIM   pid=$NEW_XIM_PID  exe_sha=$EXPECT_XIM_SHA"
echo "CORE  path=$DST_CORE  sha=$EXPECT_CORE_SHA"
echo "IBUS  pid=$ibus_pid  engine='$OLD_ENGINE'  exe_sha=$EXPECT_IBUS_SHA"
echo "RES   settings.ui sha=$(sha "$DST_UI")"
echo "RES   candidate.css sha=$(sha "$DST_CSS")"
echo "CONFIG sha=$CONFIG_SHA_AFTER (前后一致)"
echo "LOG   $LOG_FILE (unit stdout) / $XIM_LOG (进程日志)"
echo ".bak  $(ls "$DST_XIM".bak "$DST_CORE".bak "$DST_IBUS".bak 2>/dev/null | tr '\n' ' ')"
