#!/usr/bin/env bash
# JShell 本地一键验证套件（对应 README「验证」章节与 CI 质量门禁）。
# 用法: ./scripts/verify.sh [--skip-build]
# 退出码非 0 表示某一项检查未通过；每个步骤独立报告结果，最后汇总。

set -u

cd "$(dirname "$0")/.." || exit 1

SKIP_BUILD=0
if [[ "${1:-}" == "--skip-build" ]]; then
    SKIP_BUILD=1
fi

failures=0
run_step() {
    local name="$1"
    shift
    echo "==> $name"
    if "$@"; then
        echo "    PASS"
    else
        echo "    FAIL ($name)"
        failures=$((failures + 1))
    fi
}

run_step "cargo fmt --check"        cargo fmt --check
run_step "cargo test --locked"      cargo test --locked --quiet
run_step "cargo check --all-targets" cargo check --locked --all-targets
run_step "cargo clippy (-D warnings)" cargo clippy --locked --all-targets --all-features -- -D warnings
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    run_step "cargo build --release" cargo build --locked --release
fi
if command -v cargo-audit >/dev/null 2>&1 || cargo audit --version >/dev/null 2>&1; then
    run_step "cargo audit" cargo audit --file Cargo.lock
else
    echo "==> cargo audit: SKIPPED (cargo-audit not installed)"
fi
run_step "git diff --check" git diff --check

echo
if [[ "$failures" -eq 0 ]]; then
    echo "All verification steps passed."
else
    echo "$failures verification step(s) failed."
    exit 1
fi
