#!/usr/bin/env bash
# JShell Linux 冒烟测试：启动、双实例互斥、SIGTERM/SIGINT 优雅退出、
# 退出时配置落盘与残留进程检查（对应 AUDIT_REPORT 第 4 节的冒烟项）。
#
# 使用隔离 HOME（mktemp），不触碰真实用户配置；需要图形环境
# （DISPLAY 或 WAYLAND_DISPLAY），无图形环境时默认跳过。
# 用法: ./scripts/smoke-linux.sh [--binary <path>] [--require-display]

set -u

cd "$(dirname "$0")/.." || exit 1

BIN=""
REQUIRE_DISPLAY=0
EXPECT_FALLBACK=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary)
            if [[ $# -lt 2 ]]; then
                echo "FAIL: --binary requires a path" >&2
                exit 2
            fi
            BIN="$2"
            shift 2
            ;;
        --require-display)
            REQUIRE_DISPLAY=1
            shift
            ;;
        --expect-fallback)
            EXPECT_FALLBACK=1
            shift
            ;;
        *)
            echo "FAIL: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    if [[ "$REQUIRE_DISPLAY" -eq 1 ]]; then
        echo "FAIL: no DISPLAY/WAYLAND_DISPLAY; smoke test requires a graphical session"
        exit 1
    else
        echo "SKIP: no DISPLAY/WAYLAND_DISPLAY; smoke test needs a graphical session"
        exit 0
    fi
fi

if [[ -z "$BIN" ]]; then
    if ! cargo build --locked >/dev/null 2>&1; then
        echo "FAIL: debug build failed"
        exit 1
    fi
    BIN="$(pwd)/target/debug/jshell"
elif [[ ! -x "$BIN" ]]; then
    echo "FAIL: smoke-test binary is not executable: $BIN"
    exit 1
fi

TMP_HOME="$(mktemp -d)"
APP_PID=""
STARTED_PIDS=()
cleanup() {
    if [[ -n "$APP_PID" ]]; then
        kill -KILL "$APP_PID" 2>/dev/null
        # Reap a child that has already exited so it cannot remain a zombie.
        wait "$APP_PID" 2>/dev/null || true
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

# 在 timeout 秒内等待进程退出；退出或进入 zombie 状态都返回 0，超时返回 1。
wait_until_gone() {
    local timeout="$1" pid="$2" ticks=0
    while [[ -r "/proc/$pid/stat" ]] && [[ "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null)" != "Z" ]]; do
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
STARTED_PIDS+=("$APP_PID")
sleep 5
check "app survives startup (5s, no crash)" kill -0 "$APP_PID"
LOG_DIR="$TMP_HOME/.config/jshell/log"
if [[ -d "$LOG_DIR" ]]; then
    check "no panic in startup logs" sh -c "! grep -r panic \"$LOG_DIR\" 2>/dev/null | grep -q ."
else
    check "startup log directory exists" false
fi

# 2. 双实例：第二实例立即退出且不影响首实例
HOME="$TMP_HOME" timeout 5s "$BIN" >/dev/null 2>&1
SECOND_EXIT=$?
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
STARTED_PIDS+=("$APP_PID")
sleep 5
kill -INT "$APP_PID"
if wait_until_gone 3 "$APP_PID"; then
    check "SIGINT exits within 3s" true
else
    check "SIGINT exits within 3s" false
    kill -KILL "$APP_PID" 2>/dev/null
fi
wait "$APP_PID" 2>/dev/null
INT_CODE=$?
APP_PID=""
check "SIGINT exit code is 0" test "$INT_CODE" = "0"

# 5. 退出时配置落盘，或在安全存储不可用时保持非持久化
if [[ "$EXPECT_FALLBACK" -eq 1 ]]; then
    check "fallback mode does not persist configuration" \
        test ! -e "$TMP_HOME/.config/jshell/sessions.json"
    if [[ -d "$LOG_DIR" ]]; then
        check "fallback mode reports secure-storage failure" \
            sh -c "grep -r 'failed to load persistent configuration' \"$LOG_DIR\" 2>/dev/null | grep -q ."
    else
        check "fallback mode log directory exists" false
    fi
else
    check "config persisted on exit" test -f "$TMP_HOME/.config/jshell/sessions.json"
    if [[ -d "$LOG_DIR" ]]; then
        check "secure storage remained available" \
            sh -c "! grep -r 'failed to load persistent configuration' \"$LOG_DIR\" 2>/dev/null | grep -q ."
    fi
fi

# 6. 本次测试启动的进程均已退出；不扫描或干扰系统中的其他 JShell 实例。
sleep 1
LEFTOVER_STARTED_PROCESS=0
for pid in "${STARTED_PIDS[@]}"; do
    if [[ -r "/proc/$pid/stat" ]] && [[ "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null)" != "Z" ]]; then
        LEFTOVER_STARTED_PROCESS=1
        break
    fi
done
check "no leftover smoke-test processes" test "$LEFTOVER_STARTED_PROCESS" = "0"

echo
if [[ "$fails" -eq 0 ]]; then
    echo "Smoke tests passed."
else
    echo "$fails smoke check(s) failed."
    exit 1
fi
