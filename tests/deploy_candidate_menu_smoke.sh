#!/usr/bin/env bash
set -u
DEPLOY="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/scripts/deploy-candidate-menu.sh"
FAIL=0
note() { echo "[smoke] $*"; }
bad() { echo "FAIL: $*" >&2; FAIL=1; }

bash -n "$DEPLOY" || bad "bash -n 语法失败"

snapshot() {
    sha256sum /usr/local/lib/lyyime/liblyyime_core.so /usr/local/bin/lyyime-xim \
        /home/root/.local/share/lyyime/ibus/engine/ibus-engine-lyyime \
        /usr/local/bin/lyyime-float /usr/local/share/lyyime/res/settings.ui \
        /home/root/.config/lyyime/config.toml 2>/dev/null
    ls /usr/local/lib/lyyime/*.new* /usr/local/bin/*.new* \
        /home/root/.local/share/lyyime/ibus/engine/*.new* \
        /usr/local/share/lyyime/res/*.new* \
        /usr/local/lib/lyyime/*.restore.* /usr/local/bin/*.restore.* 2>/dev/null || true
}
PRE="$(snapshot)"

out="$(bash "$DEPLOY" 2>&1)"; rc=$?
[[ $rc -ne 0 && $out == *EXPECT_CORE_SHA* ]] || bad "缺 EXPECT 变量未拒绝(rc=$rc):$out"

out="$(EXPECT_CORE_SHA=zz EXPECT_IBUS_SHA=zz EXPECT_XIM_SHA=zz \
    EXPECT_FLOAT_SHA=zz EXPECT_SETTINGS_SHA=zz bash "$DEPLOY" 2>&1)"; rc=$?
[[ $rc -ne 0 && $out == *hex* ]] || bad "非法 sha 格式未拒绝(rc=$rc):$out"

Z=0000000000000000000000000000000000000000000000000000000000000000
out="$(EXPECT_CORE_SHA=$Z EXPECT_IBUS_SHA=$Z EXPECT_XIM_SHA=$Z \
    EXPECT_FLOAT_SHA=$Z EXPECT_SETTINGS_SHA=$Z bash "$DEPLOY" 2>&1)"; rc=$?
[[ $rc -ne 0 && $out == *"sha 不符"* ]] || bad "错误工件 sha 未拒绝(rc=$rc):$out"

exec 8>/tmp/lyyime-deploy-candidate-menu.lock
flock -n 8 || bad "无法获取测试锁"
out="$(bash "$DEPLOY" 2>&1)"; rc=$?
[[ $rc -ne 0 && $out == *"部署锁"* ]] || bad "持锁下未拒绝并发(rc=$rc):$out"
flock -u 8

MANIFEST=/tmp/lyyime-task/artifact-sha256.txt
if [[ -f $MANIFEST ]]; then
    mapfile -t XPIDS < <(pgrep -u "$(id -u)" -x lyyime-xim 2>/dev/null | while read -r p; do
        [[ $(readlink "/proc/$p/exe" 2>/dev/null) == /usr/local/bin/lyyime-xim ]] && echo "$p"
    done)
    if [[ ${#XPIDS[@]} -eq 1 ]]; then
        out="$(env $(grep '^EXPECT_' "$MANIFEST") bash "$DEPLOY" --dry-run 2>&1)"; rc=$?
        if [[ $rc -eq 0 && $out == *"预校验通过"* && $out == *"--dry-run"* ]]; then
            note "live --dry-run 预校验通过(只读)"
        else
            bad "live --dry-run 预校验未通过(rc=$rc):$out"
        fi
    else
        note "跳过 live dry-run:会话内已安装 XIM 数=${#XPIDS[@]}"
    fi
else
    note "跳过 live dry-run:无 $MANIFEST"
fi

POST="$(snapshot)"
[[ $PRE == "$POST" ]] || bad "工件/配置 sha 或 .new/.restore 出现变化(脚本疑似有写入)"

if ! command -v gcc >/dev/null; then
    note "无 gcc:跳过 mocked 生命周期 fixture"
    [[ $FAIL -eq 0 ]] && echo "PASS(部分):拒绝路径+无副作用;fixture 未跑"
    exit $FAIL
fi

FIX="$(mktemp -d /tmp/lyyime-deploy-fx.XXXXXX)"
FPIDS=()
fx_killall() {
    local d exe
    for d in /proc/[0-9]*; do
        exe="$(readlink "$d/exe" 2>/dev/null || true)"
        [[ $exe == "$FIX/"* ]] && kill "${d#/proc/}" 2>/dev/null
    done
    sleep 0.3
    for d in /proc/[0-9]*; do
        exe="$(readlink "$d/exe" 2>/dev/null || true)"
        [[ $exe == "$FIX/"* ]] && kill -9 "${d#/proc/}" 2>/dev/null
    done
    sleep 0.2
}
fx_cleanup() {
    fx_killall
    rm -rf "$FIX"
}
trap fx_cleanup EXIT

mkdir -p "$FIX"/{bin,scripts,etc} \
    "$FIX"/cargotarget/release \
    "$FIX"/xim/build/bin "$FIX"/xim/res \
    "$FIX"/dst/{core,ibus,xim,float,res} "$FIX"/fakebin

printf '#include <unistd.h>\nint main(void){for(;;)pause();return 0;}\n' > "$FIX/sleeper.c"
gcc -o "$FIX/sleeper" "$FIX/sleeper.c" || bad "fixture sleeper 编译失败"
printf 'int lyyime_core_fixture;\n' | gcc -shared -fPIC -x c - -o "$FIX/coreold.so" \
    || bad "fixture core 编译失败"

cat > "$FIX/fake_xim.c" <<EOF
#include <dlfcn.h>
#include <unistd.h>

volatile int tag = TAG;
int main(void){dlopen("$FIX/dst/core/liblyyime_core.so",RTLD_NOW);for(;;)pause();return tag;}
EOF
gcc -DTAG=0 -o "$FIX/ximold" "$FIX/fake_xim.c" -ldl || bad "fixture xim 旧编译失败"
gcc -DTAG=1 -o "$FIX/ximnew" "$FIX/fake_xim.c" -ldl || bad "fixture xim 新编译失败"

cp "$FIX/coreold.so" "$FIX/dst/core/liblyyime_core.so"
printf 'int core_new_fixture;\n' | gcc -shared -fPIC -x c - \
    -o "$FIX/cargotarget/release/liblyyime_core.so" || bad "fixture 新 core 编译失败"

for d in "$FIX/dst/ibus/ibus-engine-lyyime" "$FIX/dst/float/lyyime-float" \
         "$FIX/fakebin/ibus-daemon" "$FIX/fakebin/ibus-ui-gtk3"; do
    cp "$FIX/sleeper" "$d"; chmod +x "$d"
done
cp "$FIX/ximold" "$FIX/dst/xim/lyyime-xim"; chmod +x "$FIX/dst/xim/lyyime-xim"
cp "$FIX/ximnew" "$FIX/cargotarget/release/ibus-engine-lyyime"
cp "$FIX/ximnew" "$FIX/cargotarget/release/lyyime-float"
cp "$FIX/ximnew" "$FIX/xim/build/bin/lyyime-xim"
for f in "$FIX/cargotarget/release/ibus-engine-lyyime" \
         "$FIX/cargotarget/release/lyyime-float" \
         "$FIX/xim/build/bin/lyyime-xim"; do chmod +x "$f"; done
printf 'old ui\n' > "$FIX/dst/res/settings.ui"
printf 'new ui\n' > "$FIX/xim/res/settings.ui"
printf 'mode = 0\n' > "$FIX/etc/config.toml"
chmod 644 "$FIX/dst/res/settings.ui" "$FIX/etc/config.toml" "$FIX/xim/res/settings.ui"

DSTX="$FIX/dst/xim/lyyime-xim"
DSTI="$FIX/dst/ibus/ibus-engine-lyyime"

cat > "$FIX/bin/systemctl" <<EOF
#!/usr/bin/env bash
set -u
cmd="\$1"; shift || true
field=""
while [[ \$# -gt 0 ]]; do
    case "\$1" in -p) field="\$2"; shift 2 ;; *) shift ;; esac
done
case "\$cmd" in
    show)
        case "\$field" in
            LoadState) cat "$FIX/u.load" 2>/dev/null || echo not-found ;;
            ActiveState) cat "$FIX/u.act" 2>/dev/null || echo inactive ;;
            MainPID) cat "$FIX/u.pid" 2>/dev/null || echo 0 ;;
            ExecStart) cat "$FIX/u.exec" 2>/dev/null || true ;;
            Environment) cat "$FIX/u.env" 2>/dev/null || true ;;
        esac ;;
    stop)
        p="\$(cat "$FIX/u.pid" 2>/dev/null || echo 0)"
        [[ \$p != 0 ]] && kill "\$p" 2>/dev/null
        echo inactive > "$FIX/u.act"; echo 0 > "$FIX/u.pid" ;;
    start)
        setsid env -i HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
            "$DSTX" >/dev/null 2>&1 9>&- &
        echo \$! > "$FIX/u.pid"; echo active > "$FIX/u.act"; echo loaded > "$FIX/u.load"
        echo "{ path=$DSTX ; argv[]=$DSTX ; ignore_errors=no }" > "$FIX/u.exec"
        echo "HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus" > "$FIX/u.env" ;;
    *) exit 0 ;;
esac
EOF
cat > "$FIX/bin/systemd-run" <<EOF
#!/usr/bin/env bash
set -u
if [[ \${STUB_FAIL_RUN:-} == 1 && ! -e "$FIX/run.failed" ]]; then
    touch "$FIX/run.failed"; exit 1
fi
bin="\${!#}"
setsid env -i HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
    "\$bin" >/dev/null 2>&1 9>&- &
echo \$! > "$FIX/u.pid"; echo active > "$FIX/u.act"; echo loaded > "$FIX/u.load"
echo "{ path=\$bin ; argv[]=\$bin ; ignore_errors=no }" > "$FIX/u.exec"
echo "HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus" > "$FIX/u.env"
exit 0
EOF
cat > "$FIX/bin/ibus" <<EOF
#!/usr/bin/env bash
set -u
if [[ "\$*" == "engine lyyime" ]]; then
    if ! pgrep -u "\$(id -u)" -x ibus-engine-lyyime | while read -r p; do
        [[ \$(readlink /proc/\$p/exe 2>/dev/null) == "$DSTI"* ]] && exit 0
    done; then :; fi
    if ! pgrep -u "\$(id -u)" -f "^$DSTI" >/dev/null 2>&1; then
        setsid env -i HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
            "$DSTI" >/dev/null 2>&1 9>&- &
    fi
    exit 0
fi
[[ "\$*" == engine ]] && { echo lyyime; exit 0; }
[[ "\$*" == address ]] && { echo "unix:fake"; exit 0; }
exit 0
EOF
cat > "$FIX/bin/xdotool" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$FIX/bin/xdpyinfo" <<EOF
#!/usr/bin/env bash
want="\$(cat "$FIX/xauth.mode" 2>/dev/null || echo unset)"
if [[ "\$want" == unset ]]; then
    [[ -z \${XAUTHORITY+x} ]] || exit 3
else
    [[ \${XAUTHORITY:-} == "\$want" ]] || exit 4
fi
[[ \${HOME:-} == /home/root && \${DISPLAY:-} == :99 ]] || exit 5
exit 0
EOF
cat > "$FIX/scripts/deploy-ibus-engine.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail
cp -a "$DSTI" "$DSTI.bak"
install -m755 "\$CARGO_TARGET_DIR/release/ibus-engine-lyyime" "$DSTI"
for p in \$(pgrep -u "\$(id -u)" -f "^$DSTI" 2>/dev/null || true); do kill "\$p" 2>/dev/null || true; done
sleep 0.3
setsid env -i HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
    "$DSTI" >/dev/null 2>&1 9>&- &
exit 0
EOF
chmod +x "$FIX"/bin/* "$FIX/scripts/deploy-ibus-engine.sh"

echo not-found > "$FIX/u.load"; echo inactive > "$FIX/u.act"
echo 0 > "$FIX/u.pid"; : > "$FIX/u.exec"
echo unset > "$FIX/xauth.mode"

sed -e "s|^TARGET=.*|TARGET=$FIX/cargotarget|" \
    -e "s|^DST_CORE=.*|DST_CORE=$FIX/dst/core/liblyyime_core.so|" \
    -e "s|^DST_IBUS=.*|DST_IBUS=$DSTI|" \
    -e "s|^DST_XIM=.*|DST_XIM=$DSTX|" \
    -e "s|^DST_FLOAT=.*|DST_FLOAT=$FIX/dst/float/lyyime-float|" \
    -e "s|^DST_SETTINGS=.*|DST_SETTINGS=$FIX/dst/res/settings.ui|" \
    -e "s|^CONFIG=.*|CONFIG=$FIX/etc/config.toml|" \
    -e "s|^UNIT=.*|UNIT=lyyime-xim-fx.service|" \
    -e "s|^LOCK=.*|LOCK=$FIX/lock|" \
    "$DEPLOY" > "$FIX/scripts/deploy-candidate-menu.sh"
bash -n "$FIX/scripts/deploy-candidate-menu.sh" || bad "fixture patched 脚本语法失败"

fx_sha() { sha256sum "$1" | cut -d' ' -f1; }
FEXP_CORE="$(fx_sha "$FIX/cargotarget/release/liblyyime_core.so")"
FEXP_IBUS="$(fx_sha "$FIX/cargotarget/release/ibus-engine-lyyime")"
FEXP_FLOAT="$(fx_sha "$FIX/cargotarget/release/lyyime-float")"
FEXP_XIM="$(fx_sha "$FIX/xim/build/bin/lyyime-xim")"
FEXP_SETTINGS="$(fx_sha "$FIX/xim/res/settings.ui")"
FOLD_XIM="$(fx_sha "$FIX/dst/xim/lyyime-xim")"
FOLD_IBUS="$(fx_sha "$FIX/dst/ibus/ibus-engine-lyyime")"
FOLD_CORE="$(fx_sha "$FIX/dst/core/liblyyime_core.so")"
FOLD_FLOAT="$(fx_sha "$FIX/dst/float/lyyime-float")"
FOLD_SETTINGS="$(fx_sha "$FIX/dst/res/settings.ui")"
FOLD_CFG="$(fx_sha "$FIX/etc/config.toml")"

spawn_fakes() {
    setsid env -i HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
        ${FX_EXTRA:-} "$DSTX" >/dev/null 2>&1 & FPIDS+=($!)
    setsid env -i HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
        "$DSTI" >/dev/null 2>&1 & FPIDS+=($!)
    setsid env -i HOME=/home/root DISPLAY=:99 \
        "$FIX/fakebin/ibus-daemon" >/dev/null 2>&1 & FPIDS+=($!)
    setsid env -i HOME=/home/root DISPLAY=:99 \
        "$FIX/fakebin/ibus-ui-gtk3" >/dev/null 2>&1 & FPIDS+=($!)
    sleep 0.5
}
fx_env() {
    echo "PATH=$FIX/bin:$PATH EXPECT_CORE_SHA=$FEXP_CORE EXPECT_IBUS_SHA=$FEXP_IBUS \
EXPECT_XIM_SHA=$FEXP_XIM EXPECT_FLOAT_SHA=$FEXP_FLOAT EXPECT_SETTINGS_SHA=$FEXP_SETTINGS"
}

sf=$FAIL
echo "== fixture S1:not-found unit → 全流程 =="
spawn_fakes
out="$(env PATH="$FIX/bin:$PATH" EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" 2>&1)"; rc=$?
echo "$out" | tail -4
[[ $rc -eq 0 && $out == *"PASS: 候选菜单窄部署完成"* ]] || bad "S1 部署未通过(rc=$rc)"
[[ $(fx_sha "$FIX/dst/core/liblyyime_core.so") == "$FEXP_CORE" ]] || bad "S1 core 未替换"
[[ $(fx_sha "$FIX/dst/xim/lyyime-xim") == "$FEXP_XIM" ]] || bad "S1 xim 未替换"
[[ $(fx_sha "$DSTI") == "$FEXP_IBUS" ]] || bad "S1 ibus 未替换"
[[ $(fx_sha "$FIX/dst/float/lyyime-float") == "$FEXP_FLOAT" ]] || bad "S1 float 未替换"
[[ $(fx_sha "$FIX/dst/res/settings.ui") == "$FEXP_SETTINGS" ]] || bad "S1 settings 未替换"
[[ $(fx_sha "$FIX/etc/config.toml") == "$FOLD_CFG" ]] || bad "S1 config 被改动"
[[ $(cat "$FIX/u.act") == active ]] || bad "S1 单元未激活"
mp="$(cat "$FIX/u.pid")"
[[ -d /proc/$mp && $(fx_sha "/proc/$mp/exe") == "$FEXP_XIM" ]] || bad "S1 新 XIM sha 不符"
grep -qF "$FIX/dst/core/liblyyime_core.so" "/proc/$mp/maps" || bad "S1 新 XIM 未映射 core"
fx_killall
[[ $FAIL -eq $sf ]] && note "S1 通过"

sf=$FAIL
echo "== fixture S2:启动失败 → 回滚还原 =="
printf 'old ui\n' > "$FIX/dst/res/settings.ui"
cp "$FIX/ximold" "$FIX/dst/xim/lyyime-xim"
cp "$FIX/sleeper" "$DSTI"
cp "$FIX/coreold.so" "$FIX/dst/core/liblyyime_core.so"
cp "$FIX/sleeper" "$FIX/dst/float/lyyime-float"
rm -f "$FIX"/dst/*/*.new* "$FIX"/dst/*/*.restore.* "$FIX/run.failed"
echo not-found > "$FIX/u.load"; echo inactive > "$FIX/u.act"; echo 0 > "$FIX/u.pid"
spawn_fakes
out="$(env PATH="$FIX/bin:$PATH" STUB_FAIL_RUN=1 EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" 2>&1)"; rc=$?
echo "$out" | tail -6
[[ $rc -ne 0 && $out == *"回滚完成"* ]] || bad "S2 回滚未完成(rc=$rc)"
[[ $(fx_sha "$FIX/dst/xim/lyyime-xim") == "$FOLD_XIM" ]] || bad "S2 xim 未还原"
[[ $(fx_sha "$DSTI") == "$FOLD_IBUS" ]] || bad "S2 ibus 未还原"
[[ $(fx_sha "$FIX/dst/core/liblyyime_core.so") == "$FOLD_CORE" ]] || bad "S2 core 未还原"
[[ $(fx_sha "$FIX/dst/float/lyyime-float") == "$FOLD_FLOAT" ]] || bad "S2 float 未还原"
[[ $(fx_sha "$FIX/dst/res/settings.ui") == "$FOLD_SETTINGS" ]] || bad "S2 settings 未还原"
mp="$(cat "$FIX/u.pid")"
[[ -d /proc/$mp && $(fx_sha "/proc/$mp/exe") == "$FOLD_XIM" ]] || bad "S2 旧 XIM 未恢复运行"
ci_file="$(stat -c %i "$FIX/dst/core/liblyyime_core.so")"
ci_map="$(awk '$6 ~ /liblyyime_core.so$/ {print $5; exit}' "/proc/$mp/maps" 2>/dev/null || true)"
[[ -n $ci_map && $ci_map == "$ci_file" ]] \
    || bad "S2 旧 XIM 映射 core inode 非还原文件(maps=$ci_map file=$ci_file)"
fx_killall
[[ $FAIL -eq $sf ]] && note "S2 通过"

sf=$FAIL
echo "== fixture S3:托管单元 MainPID 一致 → systemctl 路径 =="
cp "$FIX/ximold" "$FIX/dst/xim/lyyime-xim"
printf 'old ui\n' > "$FIX/dst/res/settings.ui"
cp "$FIX/coreold.so" "$FIX/dst/core/liblyyime_core.so"
cp "$FIX/sleeper" "$DSTI"
cp "$FIX/sleeper" "$FIX/dst/float/lyyime-float"
rm -f "$FIX"/dst/*/*.new* "$FIX"/dst/*/*.restore.* "$FIX/run.failed"
spawn_fakes
xp="$(pgrep -u "$(id -u)" -x lyyime-xim | while read -r p; do
    [[ $(readlink "/proc/$p/exe" 2>/dev/null) == "$DSTX" ]] && echo "$p"; done | head -1)"
echo loaded > "$FIX/u.load"; echo active > "$FIX/u.act"
echo "$xp" > "$FIX/u.pid"
echo "{ path=$DSTX ; argv[]=$DSTX ; ignore_errors=no }" > "$FIX/u.exec"
echo "HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus" > "$FIX/u.env"
out="$(env PATH="$FIX/bin:$PATH" EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" 2>&1)"; rc=$?
echo "$out" | tail -4
[[ $rc -eq 0 && $out == *"PASS: 候选菜单窄部署完成"* ]] || bad "S3 托管部署未通过(rc=$rc)"
mp="$(cat "$FIX/u.pid")"
[[ -d /proc/$mp && $(fx_sha "/proc/$mp/exe") == "$FEXP_XIM" ]] || bad "S3 新 XIM sha 不符"
fx_killall
[[ $FAIL -eq $sf ]] && note "S3 通过"

sf=$FAIL
echo "== fixture S4:loaded 但 ExecStart 未核实 → 拒绝 =="
spawn_fakes
echo loaded > "$FIX/u.load"; echo inactive > "$FIX/u.act"
echo 0 > "$FIX/u.pid"
echo "{ path=/usr/bin/other-daemon ; argv[]=/usr/bin/other-daemon }" > "$FIX/u.exec"
echo "HOME=/home/root DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus" > "$FIX/u.env"
out="$(env PATH="$FIX/bin:$PATH" EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" 2>&1)"; rc=$?
[[ $rc -ne 0 && $out == *"未核实"* ]] || bad "S4 未核实单元未拒绝(rc=$rc):$out"
fx_killall
[[ $FAIL -eq $sf ]] && note "S4 通过"

echo "== fixture S5:authority 契约(dry-run 握手) =="
sf=$FAIL
echo not-found > "$FIX/u.load"; echo inactive > "$FIX/u.act"; echo 0 > "$FIX/u.pid"
echo unset > "$FIX/xauth.mode"
FX_EXTRA="" spawn_fakes
out="$(env PATH="$FIX/bin:$PATH" EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" --dry-run 2>&1)"; rc=$?
[[ $rc -eq 0 && $out == *"预校验通过"* ]] \
    || bad "S5a:XAUTHORITY 缺省未 unset 契约致拒(rc=$rc):$out"
fx_killall

FX_EXTRA="XAUTHORITY=$FIX/xauthfile" spawn_fakes
echo "$FIX/xauthfile" > "$FIX/xauth.mode"
out="$(env PATH="$FIX/bin:$PATH" EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" --dry-run 2>&1)"; rc=$?
[[ $rc -eq 0 && $out == *"预校验通过"* ]] \
    || bad "S5b:XAUTHORITY 有值未透传致拒(rc=$rc):$out"

echo unset > "$FIX/xauth.mode"
out="$(env PATH="$FIX/bin:$PATH" EXPECT_CORE_SHA="$FEXP_CORE" \
    EXPECT_IBUS_SHA="$FEXP_IBUS" EXPECT_XIM_SHA="$FEXP_XIM" \
    EXPECT_FLOAT_SHA="$FEXP_FLOAT" EXPECT_SETTINGS_SHA="$FEXP_SETTINGS" \
    bash "$FIX/scripts/deploy-candidate-menu.sh" --dry-run 2>&1)"; rc=$?
[[ $rc -ne 0 && $out == *"握手失败"* ]] \
    || bad "S5c:authority 不符握手未拒绝(rc=$rc):$out"
fx_killall
[[ $FAIL -eq $sf ]] && note "S5 通过"

POST2="$(snapshot)"
[[ $PRE == "$POST2" ]] || bad "fixture 运行后生产工件状态变化(疑似越界写入)"

[[ $FAIL -eq 0 ]] && echo "PASS: deploy-candidate-menu smoke(拒绝路径+S1~S5 生命周期+无副作用)"
exit $FAIL
