#!/usr/bin/env bash
live_exe_pids() {
    local proot="$1" want="$2" pid exe st
    shift 2
    for pid in "$@"; do
        st="$(sed -E 's/^.*\) //' "$proot/$pid/stat" 2>/dev/null | awk '{print $1}')"
        [[ -n $st ]] || continue
        if [[ $st == Z ]]; then
            echo "[probe] ignored ended launcher pid=$pid state=Z" >&2
            continue
        fi
        exe="$(readlink "$proot/$pid/exe" 2>/dev/null || true)"
        [[ $exe == "$want" || $exe == "$want (deleted)" ]] || continue
        printf '%s\n' "$pid"
    done
}

env_kv_has() { grep -zFxq -- "$2=$3" "$1" 2>/dev/null; }

session_xim_pid() {
    local proot="${1:-/proc}"
    local -a pids live
    mapfile -t pids < <(pgrep -u "$(id -u)" -x lyyime-xim || true)
    mapfile -t live < <(live_exe_pids "$proot" /usr/local/bin/lyyime-xim \
        ${pids[@]+"${pids[@]}"})
    [[ ${#live[@]} -eq 1 ]] || return 1
    printf '%s\n' "${live[0]}"
}

engine_pids() {
    local proot="${1:-/proc}" want="${2:?engine exe 必传}" p
    { pgrep -u "$(id -u)" -f '^/[^ ]*/ibus-engine-lyyime( |$)' 2>/dev/null || true; } \
        | while read -r p; do
            [[ $(live_exe_pids "$proot" "$want" "$p") == "$p" ]] || continue
            env_kv_has "$proot/$p/environ" DISPLAY "${DISPLAY:-}" || continue
            env_kv_has "$proot/$p/environ" DBUS_SESSION_BUS_ADDRESS \
                "${DBUS_SESSION_BUS_ADDRESS:-}" || continue
            printf '%s\n' "$p"
        done
}
