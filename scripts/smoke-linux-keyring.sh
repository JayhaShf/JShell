#!/usr/bin/env bash
# Run the Linux application smoke test with an isolated Secret Service.

set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "${JSHELL_KEYRING_SMOKE_INNER:-0}" != "1" ]]; then
    for command in dbus-run-session gnome-keyring-daemon; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "FAIL: $command is required for the Linux keyring smoke test" >&2
            exit 1
        fi
    done

    KEYRING_HOME="$(mktemp -d)"
    mkdir -p "$KEYRING_HOME/runtime" "$KEYRING_HOME/control"
    chmod 700 "$KEYRING_HOME" "$KEYRING_HOME/runtime" "$KEYRING_HOME/control"
    cleanup() {
        local attempt
        for attempt in {1..20}; do
            if rm -rf -- "$KEYRING_HOME" 2>/dev/null; then
                return 0
            fi
            sleep 0.1
        done
        echo "FAIL: could not remove isolated keyring home: $KEYRING_HOME" >&2
        return 1
    }
    trap cleanup EXIT

    HOME="$KEYRING_HOME" \
        XDG_RUNTIME_DIR="$KEYRING_HOME/runtime" \
        JSHELL_KEYRING_SMOKE_INNER=1 \
        JSHELL_KEYRING_SMOKE_HOME="$KEYRING_HOME" \
        dbus-run-session -- "$0" "$@"
    exit $?
fi

KEYRING_HOME="${JSHELL_KEYRING_SMOKE_HOME:?missing isolated keyring home}"
CONTROL_DIR="$KEYRING_HOME/control"

# PAM normally initializes and unlocks the login keyring. CI has no PAM login,
# so reproduce that sequence with an isolated, throwaway password and home.
printf '%s' 'jshell-ci-keyring-password' \
    | gnome-keyring-daemon --login --control-directory="$CONTROL_DIR" \
        >"$KEYRING_HOME/login.env"
gnome-keyring-daemon --start --components=secrets --control-directory="$CONTROL_DIR" \
    >"$KEYRING_HOME/start.env"

if [[ -n "${DISPLAY:-}" ]]; then
    unset WAYLAND_DISPLAY
    exec bash scripts/smoke-linux.sh "$@"
fi

if ! command -v xvfb-run >/dev/null 2>&1; then
    echo "FAIL: xvfb-run is required when no graphical display is available" >&2
    exit 1
fi
unset WAYLAND_DISPLAY
exec xvfb-run -a bash scripts/smoke-linux.sh "$@"
