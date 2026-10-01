#!/usr/bin/env bash
# Restore the current desktop panel without restarting IBus or its engine.
set -euo pipefail
session_pid="$(systemctl show lyyime-xim-sample.service -p MainPID --value)"
[[ "$session_pid" =~ ^[0-9]+$ && "$session_pid" -gt 1 ]]
[[ "$(readlink "/proc/$session_pid/exe")" == /usr/local/bin/lyyime-xim ]]
declare -A session_env=()
while IFS= read -r -d '' entry; do
    case "$entry" in
        HOME=*|DISPLAY=*|XDG_RUNTIME_DIR=*|DBUS_SESSION_BUS_ADDRESS=*|XAUTHORITY=*)
            session_env["${entry%%=*}"]="${entry#*=}"
            ;;
    esac
done < "/proc/$session_pid/environ"
for name in HOME DISPLAY XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS; do
    [[ -n "${session_env[$name]:-}" ]]
    export "$name=${session_env[$name]}"
done
if [[ -n "${session_env[XAUTHORITY]:-}" ]]; then
    export XAUTHORITY="${session_env[XAUTHORITY]}"
fi
unset IBUS_ADDRESS
address="$(timeout 3 ibus address)"
[[ -n "$address" ]]
panel_owner() {
    timeout 3 gdbus call --address "$address" --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner org.freedesktop.IBus.Panel
}
if [[ "$(panel_owner)" == "(true,)" ]]; then
    echo "IBus panel already connected."
    exit 0
fi
panel_env=()
[[ -z "${session_env[XAUTHORITY]:-}" ]] || panel_env+=("--setenv=XAUTHORITY=${session_env[XAUTHORITY]}")
systemd-run --unit=lyyime-ibus-panel.service --collect --property=Type=simple \
    --property=Restart=on-failure --property=RestartSec=1 \
    --setenv="HOME=$HOME" --setenv="DISPLAY=$DISPLAY" \
    --setenv="XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" \
    --setenv="DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS" \
    --setenv="IBUS_ADDRESS=$address" --setenv=GTK_IM_MODULE=ibus \
    "${panel_env[@]}" /usr/libexec/ibus-ui-gtk3
for _ in {1..40}; do
    if [[ "$(panel_owner)" == "(true,)" ]]; then
        sleep 3
        [[ "$(panel_owner)" == "(true,)" ]]
        echo "IBus panel restored and connected: $address"
        exit 0
    fi
    sleep 0.25
done
journalctl -u lyyime-ibus-panel.service -n 30 --no-pager
exit 1
