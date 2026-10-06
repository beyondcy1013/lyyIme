#!/usr/bin/env bash
set -euo pipefail
FIX="$(mktemp -d /tmp/lyyime-pid-smoke.XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$ROOT/scripts/lib-live-pids.sh"
mkdir -p "$FIX/bin" "$FIX/proc"
FAIL=0
bad() { echo "BAD:$*" >&2; FAIL=$((FAIL+1)); }

mkproc() {
    local d="$FIX/proc/$1"
    mkdir -p "$d"
    printf '%s (stubproc) %s 1 1 1 0 -1 4194304 100 0 0 0 0 0 0 0 20 0 1 0 0 0 0 0\n' \
        "$1" "$2" > "$d/stat"
    ln -sfn "$3" "$d/exe"
}

mkenv() {
    local d="$FIX/proc/$1"
    shift
    printf '%s\0' "$@" > "$d/environ"
}

cat > "$FIX/bin/pgrep" <<EOF
#!/usr/bin/env bash
cat "$FIX/pgrep.out" 2>/dev/null || true
EOF
chmod +x "$FIX/bin/pgrep"
export PATH="$FIX/bin:$PATH"

mkproc 101 S /usr/local/bin/lyyime-xim
mkproc 202 Z /usr/local/bin/lyyime-xim
mkproc 303 S "$FIX/other-xim"
mkdir -p "$FIX/proc/909"   # stat/exe 均缺失,直接跳过
printf '101\n202\n303\n909\n' > "$FIX/pgrep.out"

out="$(session_xim_pid "$FIX/proc" 2>"$FIX/err1.log")"; rc=$?
[[ $rc -eq 0 && $out == 101 ]] \
    || bad "T1:存活+exe 过滤后未唯一选中 101(rc=$rc out=$out)"
grep -q 'ignored ended launcher pid=202 state=Z' "$FIX/err1.log" \
    || bad "T1:缺少 Z 忽略探针证据:$(cat "$FIX/err1.log" 2>/dev/null)"

mkproc 404 S /usr/local/bin/lyyime-xim
printf '101\n404\n202\n' > "$FIX/pgrep.out"
if session_xim_pid "$FIX/proc" >/dev/null 2>&1; then
    bad "T2:双存活同 exe 未拒绝"
fi
rm -f "$FIX/proc/404/stat"
printf '202\n' > "$FIX/pgrep.out"
if session_xim_pid "$FIX/proc" >/dev/null 2>&1; then
    bad "T3:仅僵尸未拒绝"
fi

ENG="$FIX/engine-bin/ibus-engine-lyyime"
mkproc 501 S "$ENG"; mkenv 501 'DISPLAY=:99' 'DBUS_SESSION_BUS_ADDRESS=fakebus'
mkproc 502 S "$ENG"; mkenv 502 'DISPLAY=:OTHER' 'DBUS_SESSION_BUS_ADDRESS=fakebus'
mkproc 503 Z "$ENG"; mkenv 503 'DISPLAY=:99' 'DBUS_SESSION_BUS_ADDRESS=fakebus'
mkproc 504 S "$FIX/other-eng"; mkenv 504 'DISPLAY=:99' 'DBUS_SESSION_BUS_ADDRESS=fakebus'
printf '501\n502\n503\n504\n' > "$FIX/pgrep.out"
out="$(DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
    engine_pids "$FIX/proc" "$ENG" 2>"$FIX/err2.log")"
[[ $out == 501 ]] \
    || bad "T4:引擎存活+exe+会话过滤后未唯一选中 501(out=$out)"
grep -q 'ignored ended launcher pid=503 state=Z' "$FIX/err2.log" \
    || bad "T4:缺少引擎 Z 忽略证据:$(cat "$FIX/err2.log" 2>/dev/null)"

mkproc 505 S "$ENG"; mkenv 505 'DISPLAY=:99' 'DBUS_SESSION_BUS_ADDRESS=fakebus'
printf '501\n505\n' > "$FIX/pgrep.out"
out="$(DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=fakebus \
    engine_pids "$FIX/proc" "$ENG" 2>/dev/null)"
[[ $out == $'501\n505' ]] \
    || bad "T5:双存活引擎应全部列出供上层拒绝(out=$(printf %q "$out"))"

[[ $FAIL -eq 0 ]] && echo "PASS: live-pid 过滤(Z 忽略证据/唯一性/会话匹配)"
exit $FAIL
