#!/usr/bin/env bash
# JShell Linux 冒烟测试：启动、双实例互斥、SIGTERM/SIGINT 优雅退出、
# 退出时配置落盘与残留进程检查（对应 AUDIT_REPORT 第 4 节的冒烟项）。
#
# 使用隔离 HOME（mktemp），不触碰真实用户配置；需要图形环境
# （DISPLAY 或 WAYLAND_DISPLAY），无图形环境时自动跳过。
# 用法: ./scripts/smoke-linux.sh

set -u

cd "$(dirname "$0")/.." || exit 1

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "SKIP: no DISPLAY/WAYLAND_DISPLAY; smoke test needs a graphical session"
    exit 0
fi

if ! cargo build --locked >/dev/null 2>&1; then
    echo "FAIL: debug build failed"
    exit 1
fi
BIN="$(pwd)/target/debug/jshell"

TMP_HOME="$(mktemp -d)"
APP_PID=""
cleanup() {
    if [[ -n "$APP_PID" ]]; then
        kill -KILL "$APP_PID" 2>/dev/null
    fi
    rm -rf "$TMP_HOME"
}
trap cleanup EXIT

fails=0
check() {
    local name="$1"
    shift
    if "$@"; then
        echo "PASS: $name"
    else
        echo "FAIL: $name"
        fails=$((fails + 1))
    fi
}

# 在 timeout 秒内等待进程退出；退出返回 0，超时返回 1。
wait_until_gone() {
    local timeout="$1" pid="$2" ticks=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.2
        ticks=$((ticks + 1))
        if [[ $ticks -ge $((timeout * 5)) ]]; then
            return 1
        fi
    done
    return 0
}

echo "==> smoke home: $TMP_HOME"

# 1. 启动冒烟：进程存活 5 秒、日志无 panic
HOME="$TMP_HOME" "$BIN" >/dev/null 2>&1 &
APP_PID=$!
sleep 5
check "app survives startup (5s, no crash)" kill -0 "$APP_PID"
LOG_DIR="$TMP_HOME/.config/jshell/log"
if [[ -d "$LOG_DIR" ]]; then
    check "no panic in startup logs" sh -c "! grep -r panic \"$LOG_DIR\" 2>/dev/null | grep -q ."
fi

# 2. 双实例：第二实例立即退出且不影响首实例
SECOND_EXIT="$(HOME="$TMP_HOME" "$BIN" >/dev/null 2>&1; echo $?)"
check "second instance exits with code 0" test "$SECOND_EXIT" = "0"
check "first instance still running" kill -0 "$APP_PID"

# 3. SIGTERM 优雅退出（3 秒内、退出码 0）
kill -TERM "$APP_PID"
if wait_until_gone 3 "$APP_PID"; then
    check "SIGTERM exits within 3s" true
else
    check "SIGTERM exits within 3s" false
    kill -KILL "$APP_PID" 2>/dev/null
fi
wait "$APP_PID" 2>/dev/null
TERM_CODE=$?
check "SIGTERM exit code is 0" test "$TERM_CODE" = "0"

# 4. SIGINT 优雅退出
HOME="$TMP_HOME" "$BIN" >/dev/null 2>&1 &
APP_PID=$!
sleep 5
kill -INT "$APP_PID"
if wait_until_gone 3 "$APP_PID"; then
    check "SIGINT exits within 3s" true
else
    check "SIGINT exits within 3s" false
    kill -KILL "$APP_PID" 2>/dev/null
fi
wait "$APP_PID" 2>/dev/null
APP_PID=""

# 5. 退出时配置落盘
check "config persisted on exit" test -f "$TMP_HOME/.config/jshell/sessions.json"

# 6. 无残留进程
sleep 1
if command -v pgrep >/dev/null 2>&1; then
    check "no leftover jshell processes" sh -c "! pgrep -x jshell >/dev/null 2>&1"
fi

echo
if [[ "$fails" -eq 0 ]]; then
    echo "Smoke tests passed."
else
    echo "$fails smoke check(s) failed."
    exit 1
fi
