#!/usr/bin/env bash
# deploy-skins.sh — 皮肤特性安装级部署(Install-Only)
#
# 只安装已验证工件并受控重启 XIM,不构建、不跑测试、不碰 IBus/core。
# 工件哈希即权威(lead 复核成功构建后下发):
#   EXPECT_XIM_SHA     = xim/build/bin/lyyime-xim   → /usr/local/bin/lyyime-xim
#   EXPECT_UI_SHA      = xim/res/settings.ui        → /usr/local/share/lyyime/res/settings.ui
#   EXPECT_CSS_SHA     = xim/res/candidate.css      → /usr/local/share/lyyime/res/candidate.css
#   EXPECT_OLD_XIM_SHA = 当前已安装 /usr/local/bin/lyyime-xim(运行中进程 exe 同值;
#                      不符 = 另一会话已部署过,拒绝覆盖)
#
# 用法:
#   EXPECT_XIM_SHA=… EXPECT_UI_SHA=… EXPECT_CSS_SHA=… EXPECT_OLD_XIM_SHA=… \
#       bash scripts/deploy-skins.sh
#
# 保证:
#   - 全部校验(源三件 + 现役运行体)先于任何运行态改动;
#   - 三目标各留固定 .bak;安装经同目录 .new + sha 复核 + mv 原子替换;
#   - XIM 仅经 systemctl stop + systemd-run 同名 transient unit 重启,
#     会话环境白名单从现役进程 /proc/environ 原样冻结(同 deploy-menu-trigger);
#   - 会话 env 里已有 LYYIME_CORE_LIB 则原样继承,否则指向已安装
#     /usr/local/lib/lyyime/liblyyime_core.so——core 文件只校验哈希不变;
#   - 用户 config.toml 只哈希比对不读内容;IBus/面板/引擎零接触;
#   - 失败自动回滚三 .bak + 同环境重启,回滚成败如实上报。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MUTATION_STARTED=0
ROLLING_BACK=0

note() { echo "[deploy-skins] $*"; }
sha() { sha256sum "$1" 2>/dev/null | awk '{print $1}'; }

# ---- 权威入参(四个全部必传) ----
EXPECT_XIM_SHA="${EXPECT_XIM_SHA:?缺 EXPECT_XIM_SHA(xim/build/bin/lyyime-xim 的 sha256)}"
EXPECT_UI_SHA="${EXPECT_UI_SHA:?缺 EXPECT_UI_SHA(xim/res/settings.ui 的 sha256)}"
EXPECT_CSS_SHA="${EXPECT_CSS_SHA:?缺 EXPECT_CSS_SHA(xim/res/candidate.css 的 sha256)}"
EXPECT_OLD_XIM_SHA="${EXPECT_OLD_XIM_SHA:?缺 EXPECT_OLD_XIM_SHA(已安装 lyyime-xim 的 sha256)}"

# ---- 路径表(与 install.sh 既定布局一致) ----
SRC_XIM="$ROOT/xim/build/bin/lyyime-xim"
SRC_UI="$ROOT/xim/res/settings.ui"
SRC_CSS="$ROOT/xim/res/candidate.css"

DST_XIM=/usr/local/bin/lyyime-xim
DST_UI=/usr/local/share/lyyime/res/settings.ui
DST_CSS=/usr/local/share/lyyime/res/candidate.css
DST_CORE=/usr/local/lib/lyyime/liblyyime_core.so   # 只校验不动

UNIT=lyyime-xim-sample.service
LOG_FILE=/tmp/lyyime-xim-deploy.log        # unit stdout(追加,保留历史)
PID_FILE=/tmp/lyyime-xim-deploy.pid

declare -A SES=()          # 收割自现役 XIM /proc/environ,全程冻结
declare -a BACKED_UP=()
OLD_XIM_PID=0
OLD_XIM_START=""           # /proc/PID/stat starttime 冻结身份,防 PID 复用误伤

proc_start() { sed -E 's/^.*\) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
old_xim_alive() {   # 同 PID 且同 starttime 才算"还是原来那个进程"
    [[ -n "$OLD_XIM_START" && "$(proc_start "$OLD_XIM_PID")" == "$OLD_XIM_START" ]]
}

die() {
    echo "DEPLOY-FAIL: $*" >&2
    (( MUTATION_STARTED == 1 )) && rollback
    exit 1
}

backup_dst() {   # 固定 .bak 约定:只替换当前一份,不留时间戳堆
    local dst="$1"
    [[ -f $dst ]] || return 0
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

start_xim() {    # 同名 transient unit + 冻结的原会话环境;core 路径已解析
    wait_unit_idle || return 1
    systemctl reset-failed "$UNIT" 2>/dev/null || true
    local -a envargs=(
        "--setenv=HOME=${SES[HOME]}"
        "--setenv=DISPLAY=${SES[DISPLAY]}"
        "--setenv=DBUS_SESSION_BUS_ADDRESS=${SES[DBUS_SESSION_BUS_ADDRESS]}"
        "--setenv=XDG_RUNTIME_DIR=${SES[XDG_RUNTIME_DIR]}"
        "--setenv=GTK_IM_MODULE=${SES[GTK_IM_MODULE]}"
        "--setenv=XMODIFIERS=${SES[XMODIFIERS]}"
        "--setenv=LYYIME_CORE_LIB=${SES[LYYIME_CORE_LIB]}"
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

stop_managed_xim() { # 本特性只接受托管路径:systemctl stop + 有界空闲
    systemctl stop "$UNIT" || return 1
    wait_unit_idle || return 1
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
    echo "DEPLOY-FAIL: 开始回滚(三 .bak 原子还原 + 同会话环境重启 XIM)" >&2

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

    if (( ok == 1 )); then
        echo "DEPLOY-FAIL: 回滚完成" >&2
    else
        echo "DEPLOY-FAIL: 回滚未完整成功(见上方 !! 步骤,.bak 均保留)" >&2
    fi
}
trap 'rollback; exit 1' ERR

[[ $EXPECT_XIM_SHA =~ ^[0-9a-f]{64}$ ]]     || die "EXPECT_XIM_SHA 非 sha256"
[[ $EXPECT_UI_SHA =~ ^[0-9a-f]{64}$ ]]      || die "EXPECT_UI_SHA 非 sha256"
[[ $EXPECT_CSS_SHA =~ ^[0-9a-f]{64}$ ]]     || die "EXPECT_CSS_SHA 非 sha256"
[[ $EXPECT_OLD_XIM_SHA =~ ^[0-9a-f]{64}$ ]] || die "EXPECT_OLD_XIM_SHA 非 sha256"

# ============================================================
# 阶段 0:运行态改动之前的全量校验与现场留证
# ============================================================
note "阶段0:校验工件、现役运行体与会话环境"

for f in "$SRC_XIM" "$SRC_UI" "$SRC_CSS"; do
    [[ -f $f ]] || die "源工件缺失:$f"
done
[[ "$(sha "$SRC_XIM")" == "$EXPECT_XIM_SHA" ]] \
    || die "xim 源 sha 不符:$(sha "$SRC_XIM") != $EXPECT_XIM_SHA"
[[ "$(sha "$SRC_UI")" == "$EXPECT_UI_SHA" ]] \
    || die "settings.ui 源 sha 不符:$(sha "$SRC_UI") != $EXPECT_UI_SHA"
[[ "$(sha "$SRC_CSS")" == "$EXPECT_CSS_SHA" ]] \
    || die "candidate.css 源 sha 不符:$(sha "$SRC_CSS") != $EXPECT_CSS_SHA"
note "工件哈希三件核对通过"

# 现役 XIM 必须由本 unit 托管且 MainPID>1;exe 路径=安装位且哈希=旧版权威
# 值(不符说明另一会话已部署,拒绝连锁覆盖)
OLD_XIM_PID="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true)"
[[ $OLD_XIM_PID =~ ^[0-9]+$ && $OLD_XIM_PID -gt 1 ]] \
    || die "$UNIT MainPID 无效($OLD_XIM_PID):须为托管运行中的 XIM"
[[ "$(readlink "/proc/$OLD_XIM_PID/exe" 2>/dev/null)" == "$DST_XIM" ]] \
    || die "现役 XIM exe 非安装路径 $DST_XIM"
[[ "$(sha "/proc/$OLD_XIM_PID/exe")" == "$EXPECT_OLD_XIM_SHA" ]] \
    || die "现役 XIM 运行体 sha 非 EXPECT_OLD_XIM_SHA(可能已被其它会话部署)"
[[ "$(sha "$DST_XIM")" == "$EXPECT_OLD_XIM_SHA" ]] \
    || die "已安装 $DST_XIM sha 非 EXPECT_OLD_XIM_SHA(磁盘与基线不符)"
OLD_XIM_START="$(proc_start "$OLD_XIM_PID")"
[[ -n $OLD_XIM_START ]] || die "无法冻结 XIM 进程身份"
note "现役 XIM:pid=$OLD_XIM_PID(托管,exe 哈希=旧版权威值)"

# 从现役进程 /proc/environ 原样收割会话变量(白名单同 deploy-menu-trigger;
# 增 LYYIME_CORE_LIB:现役显式指定则冻结继承,否则落到已安装 core 路径)
while IFS= read -r line; do
    case "$line" in
        HOME=*|DISPLAY=*|DBUS_SESSION_BUS_ADDRESS=*|XDG_RUNTIME_DIR=*|\
        GTK_IM_MODULE=*|XMODIFIERS=*|XDG_CONFIG_HOME=*|XDG_DATA_HOME=*|\
        LYYIME_DATA_DIR=*|XAUTHORITY=*|LYYIME_CORE_LIB=*) \
            SES["${line%%=*}"]="${line#*=}" ;;
    esac
done < <(tr '\0' '\n' < "/proc/$OLD_XIM_PID/environ" 2>/dev/null || true)
for v in HOME DISPLAY DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR \
         GTK_IM_MODULE XMODIFIERS; do
    [[ -n "${SES[$v]:-}" ]] || die "现役 XIM 环境缺 $v,拒绝臆造会话"
done
if [[ -z "${SES[LYYIME_CORE_LIB]:-}" ]]; then
    [[ -f $DST_CORE ]] || die "现役环境无 LYYIME_CORE_LIB 且 $DST_CORE 不存在"
    SES[LYYIME_CORE_LIB]="$DST_CORE"
    note "LYYIME_CORE_LIB 未在会话中 → 使用已安装 $DST_CORE"
else
    note "LYYIME_CORE_LIB 继承现役值:${SES[LYYIME_CORE_LIB]}"
fi
note "会话环境:DISPLAY=${SES[DISPLAY]} GTK_IM_MODULE=${SES[GTK_IM_MODULE]} XMODIFIERS=${SES[XMODIFIERS]}"

# config.toml 按会话 XDG_CONFIG_HOME/HOME 定位;只哈希不读内容
SES_CFG_DIR="${SES[XDG_CONFIG_HOME]:-${SES[HOME]}/.config}"
CONFIG="$SES_CFG_DIR/lyyime/config.toml"
CONFIG_SHA_BEFORE="ABSENT"
[[ -f $CONFIG ]] && CONFIG_SHA_BEFORE="$(sha "$CONFIG")"
note "config.toml sha=${CONFIG_SHA_BEFORE:0:16}…(部署后需完全一致)"

# core 只校验不动:部署前哈希,部署后复核不变
CORE_SHA_BEFORE="ABSENT"
[[ -f $DST_CORE ]] && CORE_SHA_BEFORE="$(sha "$DST_CORE")"
note "core sha=${CORE_SHA_BEFORE:0:16}…(本部署不触碰,前后须一致)"

XIM_LOG="${SES[XDG_DATA_HOME]:-${SES[HOME]}/.local/share}/lyyime/logs/xim.log"

# ============================================================
# 阶段 1:备份 + 原子安装(此步起进入可回滚区)
# ============================================================
MUTATION_STARTED=1
note "阶段1:.bak 备份与原子安装"
backup_dst "$DST_XIM"; backup_dst "$DST_UI"; backup_dst "$DST_CSS"

mkdir -p "$(dirname "$DST_UI")"
atomic_install "$SRC_XIM" "$DST_XIM" 0755 "$EXPECT_XIM_SHA"
atomic_install "$SRC_UI"  "$DST_UI"  0644 "$EXPECT_UI_SHA"
atomic_install "$SRC_CSS" "$DST_CSS" 0644 "$EXPECT_CSS_SHA"
note "安装落位:xim=$(sha "$DST_XIM" | cut -c1-12) ui=$(sha "$DST_UI" | cut -c1-12) css=$(sha "$DST_CSS" | cut -c1-12)"

# ============================================================
# 阶段 2:XIM 受控重启(仅本 unit;IBus/core 零接触)
# ============================================================
note "阶段2:重启 XIM(systemd 托管,复用冻结会话环境)"
MARK="$(wc -l <"$XIM_LOG" 2>/dev/null || echo 0)"   # 启动日志判定基线
stop_managed_xim || die "旧 XIM($OLD_XIM_PID)未在时限内退出"
start_xim || die "新 XIM unit 未拉起,日志见 $LOG_FILE"
NEW_XIM_PID="$(systemctl show -p MainPID --value "$UNIT")"
printf '%s\n' "$NEW_XIM_PID" >"$PID_FILE"
note "新 XIM:pid=$NEW_XIM_PID($UNIT active)"

# ============================================================
# 阶段 3:运行态核验
# ============================================================
note "阶段3:运行态核验"
[[ "$(systemctl is-active "$UNIT")" == active ]] || die "$UNIT 非 active"
[[ "$(sha "/proc/$NEW_XIM_PID/exe")" == "$EXPECT_XIM_SHA" ]] \
    || die "新 XIM 运行体 sha 不符"

# 启动日志:基线之后须见 XIM server ready,且不得出现启动 ERROR
ok=0
for _ in $(seq 1 30); do
    tail -n +"$((MARK + 1))" "$XIM_LOG" 2>/dev/null \
        | grep -q 'XIM server ready' && { ok=1; break; }
    sleep 1
done
[[ $ok -eq 1 ]] || die "新 XIM 30s 内未见 'XIM server ready'($XIM_LOG)"
tail -n +"$((MARK + 1))" "$XIM_LOG" 2>/dev/null | grep -q 'ERROR' \
    && die "新 XIM 启动日志含 ERROR,拒绝判定成功"

# 服务稳定 2s(覆盖"起来即崩"的迟发故障)
sleep 2
[[ "$(systemctl is-active "$UNIT")" == active ]] \
    || die "$UNIT 启动后 2s 内失稳(非 active)"
kill -0 "$NEW_XIM_PID" 2>/dev/null \
    && [[ "$(systemctl show -p MainPID --value "$UNIT")" == "$NEW_XIM_PID" ]] \
    || die "$UNIT MainPID 漂移/进程消失"

# 用户配置与 core 文件字节级不动
CONFIG_SHA_AFTER="ABSENT"
[[ -f $CONFIG ]] && CONFIG_SHA_AFTER="$(sha "$CONFIG")"
[[ "$CONFIG_SHA_AFTER" == "$CONFIG_SHA_BEFORE" ]] \
    || die "config.toml 哈希变动($CONFIG_SHA_BEFORE → $CONFIG_SHA_AFTER)"
CORE_SHA_AFTER="ABSENT"
[[ -f $DST_CORE ]] && CORE_SHA_AFTER="$(sha "$DST_CORE")"
[[ "$CORE_SHA_AFTER" == "$CORE_SHA_BEFORE" ]] \
    || die "core 哈希变动($CORE_SHA_BEFORE → $CORE_SHA_AFTER,本部署不应触碰)"

trap - ERR
echo "================ DEPLOY OK(skins)================"
echo "XIM    pid=$NEW_XIM_PID  exe_sha=$EXPECT_XIM_SHA"
echo "RES    settings.ui sha=$EXPECT_UI_SHA"
echo "RES    candidate.css sha=$EXPECT_CSS_SHA"
echo "CORE   sha=$CORE_SHA_AFTER(未触碰,前后一致)"
echo "CONFIG sha=$CONFIG_SHA_AFTER(前后一致)"
echo "LOG    $LOG_FILE(unit stdout) / $XIM_LOG(进程日志)"
echo ".bak   $DST_XIM.bak $DST_UI.bak $DST_CSS.bak"
