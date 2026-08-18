#!/usr/bin/env bash
# Run the Linux smoke test with Secret Service deliberately unavailable.

set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "${JSHELL_FALLBACK_SMOKE_INNER:-0}" != "1" ]]; then
    if [[ -z "${DISPLAY:-}" ]] && ! command -v xvfb-run >/dev/null 2>&1; then
        echo "FAIL: xvfb-run is required when no X11 display is available" >&2
        exit 1
    fi

    FALLBACK_HOME="$(mktemp -d)"
    mkdir -p "$FALLBACK_HOME/runtime"
    chmod 700 "$FALLBACK_HOME" "$FALLBACK_HOME/runtime"
    cleanup() {
        rm -rf -- "$FALLBACK_HOME"
    }
    trap cleanup EXIT

    export HOME="$FALLBACK_HOME"
    export XDG_RUNTIME_DIR="$FALLBACK_HOME/runtime"
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$FALLBACK_HOME/unavailable-session-bus"
    export GNOME_KEYRING_CONTROL="$FALLBACK_HOME/unavailable-keyring"
    export JSHELL_FALLBACK_SMOKE_INNER=1

    if [[ -n "${DISPLAY:-}" ]]; then
        unset WAYLAND_DISPLAY
        bash scripts/smoke-linux.sh --expect-fallback "$@"
    else
        unset WAYLAND_DISPLAY
        xvfb-run -a bash scripts/smoke-linux.sh --expect-fallback "$@"
    fi
    exit 0
fi

exec bash scripts/smoke-linux.sh --expect-fallback "$@"
