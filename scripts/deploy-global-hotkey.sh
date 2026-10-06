#!/usr/bin/env bash
# 全局截屏快捷键(合同 §13)定向部署:只安装本次评审过的 C 部件
#   /usr/local/bin/lyyime-xim               ← xim/build/bin/lyyime-xim
#   /usr/local/share/lyyime/res/settings.ui ← xim/res/settings.ui
# 不动 Rust 侧 lyyime-shot/lyyime-ai/liblyyime_core.so(本次无变更),
# 不重新编译(复用已通过编译/单测/E2E 的构建产物)。
#
# 会话保持:运行环境从**现有 XIM 进程**的 /proc/<pid>/environ 读取
# (DISPLAY/DBUS/XDG_RUNTIME_DIR/XMODIFIERS/GTK_IM_MODULE/HOME/XAUTHORITY/
#  LANG/LC_*/XDG_*_HOME/XDG_CURRENT_DESKTOP/LYYIME_*_DIR|LIB),不猜 DISPLAY,
# 不把调度器/compile worker 的默认环境带进桌面会话;
# 目标先落 .bak 再以临时文件+mv 原子替换;新旧二进制安装前用会话环境做
# --sync-shot-hotkey 预检;重启沿用 deploy-xim.sh 的 lyyime-xim-sample.service
# 瞬态单元形态;健康判据=应用自身 xim.log 追加段出现登记与主循环行。
#
# 不做提交/推送;失败时自动回滚文件并尝试以旧二进制重启,
# .bak 保留,人工回滚命令打印在结尾。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_BIN="$ROOT/xim/build/bin/lyyime-xim"
SRC_UI="$ROOT/xim/res/settings.ui"
DST_BIN="/usr/local/bin/lyyime-xim"
DST_UI="/usr/local/share/lyyime/res/settings.ui"
UNIT="lyyime-xim-sample.service"
PID_FILE="${LYYIME_XIM_PID_FILE:-/tmp/lyyime-xim-deploy.pid}"
LOG_FILE="${LYYIME_XIM_LOG_FILE:-/tmp/lyyime-xim-deploy.log}"

die() { echo "deploy-global-hotkey: $*" >&2; exit 1; }

# ---- 1. 源工件/目标路径校验(任何运行态改动之前) ----
[[ -x "$SRC_BIN" ]] || die "构建产物缺失或不可执行:$SRC_BIN(先 webClx 编译)"
[[ -f "$SRC_UI" ]] || die "settings.ui 缺失:$SRC_UI"
[[ -x "$DST_BIN" ]] || die "目标二进制缺失:$DST_BIN"
[[ -f "$DST_UI" ]] || die "目标资源缺失:$DST_UI"

# ---- 2. 找现有 XIM 进程,读其真实会话环境 ----
unit_pid="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true)"
old_pid=""
old_via_unit=0
if [[ "$unit_pid" =~ ^[0-9]+$ && "$unit_pid" != 0 ]] && \
   [[ "$(cat /proc/"$unit_pid"/comm 2>/dev/null)" == lyyime-xim ]]; then
    old_pid="$unit_pid"
    old_via_unit=1
else
    old_pid="$(pgrep -o -x lyyime-xim 2>/dev/null || true)"
    [[ -n "$old_pid" && \
       "$(cat /proc/"$old_pid"/comm 2>/dev/null)" == lyyime-xim ]] ||
        old_pid=""
fi
[[ -n "$old_pid" ]] ||
    die "未发现运行中的 lyyime-xim(无会话环境来源;不猜 DISPLAY,中止)"

# 白名单变量:会话环境 + locale + XDG 目录 + lyyime 自定义路径;
# 不输出/不带入无关或敏感变量
WANT_VARS="DISPLAY DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR XMODIFIERS \
GTK_IM_MODULE HOME XAUTHORITY LANG LC_ALL LC_CTYPE XDG_CONFIG_HOME \
XDG_DATA_HOME XDG_CACHE_HOME XDG_CURRENT_DESKTOP LYYIME_DATA_DIR \
LYYIME_CORE_LIB LYYIME_RES_DIR"
env_pairs=()
env_display="" env_dbus="" env_home="" env_xdg_data=""
while IFS= read -r line; do
    name="${line%%=*}"
    case " $WANT_VARS " in *" $name "*) ;; *) continue ;; esac
    env_pairs+=("$line")
    case "$name" in
        DISPLAY) env_display="${line#*=}" ;;
        DBUS_SESSION_BUS_ADDRESS) env_dbus="${line#*=}" ;;
        HOME) env_home="${line#*=}" ;;
        XDG_DATA_HOME) env_xdg_data="${line#*=}" ;;
    esac
done < <(tr '\0' '\n' < "/proc/$old_pid/environ" 2>/dev/null || true)

[[ -n "$env_display" && -n "$env_dbus" ]] ||
    die "从 PID $old_pid 未读到 DISPLAY/DBUS_SESSION_BUS_ADDRESS,中止"
has_var() { # $1=变量名;env_pairs 中是否已有
    local n="$1" kv
    for kv in ${env_pairs[@]+"${env_pairs[@]}"}; do
        [[ "$kv" == "$n="* ]] && return 0
    done
    return 1
}
# 兜底默认(仅对安全变量;DISPLAY/DBUS 绝不猜)
has_var HOME || env_pairs+=("HOME=/home/root")
has_var XMODIFIERS || env_pairs+=("XMODIFIERS=@im=lyyime")
has_var GTK_IM_MODULE || env_pairs+=("GTK_IM_MODULE=xim")
has_var XDG_RUNTIME_DIR ||
    env_pairs+=("XDG_RUNTIME_DIR=/run/user/$(id -u)")
: "${env_home:=/home/root}"
echo "== 会话环境取自 PID $old_pid(via_unit=$old_via_unit,DISPLAY=$env_display) =="

XIM_APP_LOG="${env_xdg_data:-$env_home/.local/share}/lyyime/logs/xim.log"
CFG_FILE="${XDG_CONFIG_HOME:-$env_home/.config}/lyyime/config.toml"
# XDG_CONFIG_HOME 若来自环境数组则用之
for kv in "${env_pairs[@]}"; do
    [[ "$kv" == XDG_CONFIG_HOME=* ]] && CFG_FILE="${kv#XDG_CONFIG_HOME=}/lyyime/config.toml"
done

# ---- 3. 部署前配置指纹(部署不应当改动用户配置;只比对哈希) ----
cfg_hash_before=""
[[ -f "$CFG_FILE" ]] && cfg_hash_before="$(sha256sum <"$CFG_FILE")"

# ---- 4. 预检:用**源码产物**以旧会话环境登记现有快捷键 ----
#        (同一安装命令/同一已配置组合,幂等安全;旧进程继续跑;
#        环境总线/权限问题在此先行暴露,文件尚未动)
if ! env -i "${env_pairs[@]}" PATH=/usr/local/bin:/usr/bin:/bin \
        "$SRC_BIN" --sync-shot-hotkey; then
    die "预检 --sync-shot-hotkey 失败(未改任何文件,旧进程仍在运行)"
fi

# ---- 5. .bak 备份 + 原子安装 + sha256 校验 ----
cp -a "$DST_BIN" "$DST_BIN.bak"
cp -a "$DST_UI" "$DST_UI.bak"
install -m 0755 "$SRC_BIN" "$DST_BIN.tmp.$$"
mv -f "$DST_BIN.tmp.$$" "$DST_BIN"
install -m 0644 "$SRC_UI" "$DST_UI.tmp.$$"
mv -f "$DST_UI.tmp.$$" "$DST_UI"
[[ "$(sha256sum <"$SRC_BIN")" == "$(sha256sum <"$DST_BIN")" ]] ||
    die "lyyime-xim 安装后 sha256 不一致"
[[ "$(sha256sum <"$SRC_UI")" == "$(sha256sum <"$DST_UI")" ]] ||
    die "settings.ui 安装后 sha256 不一致"
echo "== 安装校验通过:$DST_BIN / $DST_UI(sha256 与源一致) =="

# ---- 6. 启动函数:瞬态单元,形态同 deploy-xim.sh;同名单元残留则先停
#        (stop 失败向上传 1,由调用处决定中止) ----
start_runtime() {
    local run_args=()
    for kv in "${env_pairs[@]}"; do run_args+=("--setenv=$kv"); done
    if systemctl is-active --quiet "$UNIT" 2>/dev/null; then
        systemctl stop "$UNIT" || return 1
    fi
    systemd-run --unit "$UNIT" --collect --property=Type=simple \
        "${run_args[@]}" \
        /bin/sh -c "exec /usr/local/bin/lyyime-xim >>'$LOG_FILE' 2>&1" \
        </dev/null >/dev/null
}

restore_bak() { # 原子还原 .bak(tmp+mv;.bak 本身保留)
    cp -a "$DST_BIN.bak" "$DST_BIN.rst.$$" && mv -f "$DST_BIN.rst.$$" "$DST_BIN"
    cp -a "$DST_UI.bak" "$DST_UI.rst.$$" && mv -f "$DST_UI.rst.$$" "$DST_UI"
}

rollback_report() {
    cat >&2 <<EOF
== 部署失败,已自动回滚文件并尝试旧二进制重启 ==
人工核对/兜底命令:
  systemctl status $UNIT --no-pager
  tail -30 '$XIM_APP_LOG'
  mv -f '$DST_BIN.bak' '$DST_BIN'   # .bak 仍在,可重复手工还原
  mv -f '$DST_UI.bak' '$DST_UI'
  systemctl restart $UNIT
可选:还原旧二进制**之前**如需摘除全局快捷键绑定
  (旧二进制无 --remove-shot-hotkey 参数):
  env HOME=$env_home DISPLAY=$env_display \\
      DBUS_SESSION_BUS_ADDRESS='$env_dbus' \\
      /usr/local/bin/lyyime-xim --remove-shot-hotkey
仅查看当前绑定(只读):
  xfconf-query -c xfce4-keyboard-shortcuts -lv | grep lyyime-shot
EOF
}

# ---- 7. 停旧实例:单元管理走 systemctl(失败即中止);独立进程只对
#        已校验 PID 优雅结束 ----
if [[ "$old_via_unit" == 1 ]]; then
    systemctl stop "$UNIT" ||
        die "systemctl stop $UNIT 失败,中止(文件已装,旧进程仍在运行)"
    systemctl reset-failed "$UNIT" 2>/dev/null || true
else
    kill -TERM "$old_pid" 2>/dev/null || true
    for _ in {1..30}; do
        kill -0 "$old_pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$old_pid" 2>/dev/null; then
        kill -KILL "$old_pid" 2>/dev/null || true
    fi
fi

# ---- 8. 重启 + 应用日志健康检查(偏移量定位新增段,≤10s 双标记) ----
mkdir -p "$(dirname "$LOG_FILE")"
log_off=0
[[ -f "$XIM_APP_LOG" ]] && log_off="$(wc -l <"$XIM_APP_LOG")"
if ! start_runtime; then
    echo "== systemd-run 启动失败,走回滚路径 ==" >&2
fi
sleep 1
new_pid="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true)"
healthy=0
if [[ "$new_pid" =~ ^[0-9]+$ && "$new_pid" != 0 ]] && \
   kill -0 "$new_pid" 2>/dev/null; then
    for _ in {1..20}; do
        seg="$(tail -n "+$((log_off + 1))" "$XIM_APP_LOG" 2>/dev/null || true)"
        if grep -q '全局截屏快捷键已登记:' <<<"$seg" && \
           grep -q '初始化完成,进入主循环' <<<"$seg"; then
            healthy=1
            break
        fi
        sleep 0.5
    done
fi

if [[ "$healthy" != 1 ]]; then
    echo "== 新实例未通过健康检查(MainPID=$new_pid),开始回滚 ==" >&2
    systemctl stop "$UNIT" 2>/dev/null || true
    systemctl reset-failed "$UNIT" 2>/dev/null || true
    restore_bak
    if ! start_runtime; then
        echo "== 警告:回滚用旧二进制重启 $UNIT 也失败 ==" >&2
    fi
    sleep 1
    rb_pid="$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true)"
    if [[ "$rb_pid" =~ ^[0-9]+$ && "$rb_pid" != 0 ]] && \
       kill -0 "$rb_pid" 2>/dev/null; then
        echo "== 回滚后旧实例已恢复运行:pid=$rb_pid ==" >&2
    else
        echo "== 警告:回滚后旧实例未拉起,需人工介入 ==" >&2
    fi
    rollback_report
    exit 1
fi
printf '%s\n' "$new_pid" >"$PID_FILE"

# ---- 9. 部署后配置指纹核对:部署不应改动用户配置;
#        若并发被用户修改,只报告不回滚 ----
cfg_hash_after=""
[[ -f "$CFG_FILE" ]] && cfg_hash_after="$(sha256sum <"$CFG_FILE")"
if [[ "$cfg_hash_before" != "$cfg_hash_after" ]]; then
    echo "== 注意:$CFG_FILE 哈希在部署期间变化(疑用户并发改动,不还原) ==" >&2
fi

tail -5 "$XIM_APP_LOG" 2>/dev/null || true
cat <<EOF
== 部署完成:lyyime-xim old_pid=$old_pid → new_pid=$new_pid(unit=$UNIT) ==
人工回滚(如需):
  mv -f '$DST_BIN.bak' '$DST_BIN'
  mv -f '$DST_UI.bak' '$DST_UI'
  systemctl restart $UNIT
可选:还原旧二进制**之前**如需摘除全局快捷键绑定
  (旧二进制无 --remove-shot-hotkey 参数):
  env HOME=$env_home DISPLAY=$env_display \\
      DBUS_SESSION_BUS_ADDRESS='$env_dbus' \\
      /usr/local/bin/lyyime-xim --remove-shot-hotkey
仅查看当前绑定(只读):
  xfconf-query -c xfce4-keyboard-shortcuts -lv | grep lyyime-shot
EOF
