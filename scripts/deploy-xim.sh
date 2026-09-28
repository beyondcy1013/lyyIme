#!/usr/bin/env bash
# Build/install lyyime-xim and restart the desktop sample with its existing
# X11/D-Bus session environment. Used by the webClx deploy coordinator.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_FILE="${LYYIME_XIM_PID_FILE:-/tmp/lyyime-xim-deploy.pid}"
LOG_FILE="${LYYIME_XIM_LOG_FILE:-/tmp/lyyime-xim-deploy.log}"

old_pid="$(pgrep -o -x lyyime-xim 2>/dev/null || true)"
terminal_pid="${LYYIME_XIM_TERMINAL_PID:-}"
if [[ -z "$old_pid" && -n "$terminal_pid" && "$terminal_pid" =~ ^[0-9]+$ ]]; then
    if [[ "$(cat /proc/$terminal_pid/comm 2>/dev/null)" == lyyime-xim ]]; then
        old_pid="$terminal_pid"
    fi
fi
old_display="${DISPLAY:-}"
old_dbus="${DBUS_SESSION_BUS_ADDRESS:-}"
old_xdg_runtime="${XDG_RUNTIME_DIR:-}"
old_xmodifiers="${XMODIFIERS:-}"

if [[ -n "$old_pid" ]]; then
    while IFS= read -r line; do
        case "$line" in
            DISPLAY=*) old_display="${line#DISPLAY=}" ;;
            DBUS_SESSION_BUS_ADDRESS=*) old_dbus="${line#DBUS_SESSION_BUS_ADDRESS=}" ;;
            XDG_RUNTIME_DIR=*) old_xdg_runtime="${line#XDG_RUNTIME_DIR=}" ;;
            XMODIFIERS=*) old_xmodifiers="${line#XMODIFIERS=}" ;;
        esac
    done < <(tr '\0' '\n' < "/proc/$old_pid/environ" 2>/dev/null || true)
fi

# After a failed previous start there may be no old IME process. Reuse the
# active XFCE session environment so the replacement joins the same X11/D-Bus
# session instead of inventing a fallback display.
if [[ -z "$old_display" || -z "$old_dbus" || -z "$old_xdg_runtime" ]]; then
    session_pid="$(pgrep -o -x xfce4-session 2>/dev/null || true)"
    if [[ -n "$session_pid" ]]; then
        while IFS= read -r line; do
            case "$line" in
                DISPLAY=*) [[ -z "$old_display" ]] && old_display="${line#DISPLAY=}" ;;
                DBUS_SESSION_BUS_ADDRESS=*) [[ -z "$old_dbus" ]] && old_dbus="${line#DBUS_SESSION_BUS_ADDRESS=}" ;;
                XDG_RUNTIME_DIR=*) [[ -z "$old_xdg_runtime" ]] && old_xdg_runtime="${line#XDG_RUNTIME_DIR=}" ;;
                XMODIFIERS=*) [[ -z "$old_xmodifiers" ]] && old_xmodifiers="${line#XMODIFIERS=}" ;;
            esac
        done < <(tr '\0' '\n' < "/proc/$session_pid/environ" 2>/dev/null || true)
    fi
fi

# GLib determines XDG data even when a desktop session lacks HOME. Remove the
# known stale singleton marker only after confirming the recorded process is
# gone; the freshly started binary immediately rewrites this same path.
pidfile=/home/root/.local/share/lyyime/xim.pid
if [[ -f "$pidfile" ]]; then
    recorded_pid="$(head -n 1 "$pidfile" 2>/dev/null | tr -cd '0-9')"
    if [[ -z "$recorded_pid" ]] || ! kill -0 "$recorded_pid" 2>/dev/null; then
        : > "$pidfile"
    fi
fi

bash "$ROOT/xim/install.sh"

if [[ -n "$old_pid" ]]; then
    kill -TERM "$old_pid" 2>/dev/null || true
    for _ in {1..30}; do
        kill -0 "$old_pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$old_pid" 2>/dev/null; then
        kill -KILL "$old_pid" 2>/dev/null || true
    fi
fi

: "${old_display:=:10.0}"
: "${old_xdg_runtime:=/run/user/$(id -u)}"
[[ -n "$old_dbus" ]] || { echo "D-Bus session address unavailable" >&2; exit 1; }

mkdir -p "$(dirname "$LOG_FILE")"

# The webClx compile worker is a transient systemd unit and may reclaim setsid
# children when the deploy script exits. Run the desktop IME from a separate
# transient unit so it survives the coordinator and can be stopped/restarted
# consistently on later deployments.
unit=lyyime-xim-sample.service
systemctl stop "$unit" 2>/dev/null || true
systemctl reset-failed "$unit" 2>/dev/null || true
systemd-run --unit "$unit" --collect --property=Type=simple \
    --setenv=HOME=/home/root \
    --setenv=DISPLAY="$old_display" \
    --setenv=DBUS_SESSION_BUS_ADDRESS="$old_dbus" \
    --setenv=XDG_RUNTIME_DIR="$old_xdg_runtime" \
    --setenv=XMODIFIERS="${old_xmodifiers:-@im=lyyime}" \
    --setenv=GTK_IM_MODULE="${GTK_IM_MODULE:-xim}" \
    /bin/sh -c "exec /usr/local/bin/lyyime-xim >>'$LOG_FILE' 2>&1" \
    </dev/null >/dev/null

sleep 1
new_pid="$(systemctl show -p MainPID --value "$unit")"
if [[ ! "$new_pid" =~ ^[0-9]+$ ]] || [[ "$new_pid" == 0 ]] || ! kill -0 "$new_pid" 2>/dev/null; then
    echo "lyyime-xim transient unit failed; log follows" >&2
    systemctl status "$unit" --no-pager >&2 || true
    cat "$LOG_FILE" >&2
    exit 1
fi
printf '%s\n' "$new_pid" >"$PID_FILE"

printf 'lyyime-xim deployed: old_pid=%s new_pid=%s log=%s\n' \
    "${old_pid:-none}" "$new_pid" "$LOG_FILE"
