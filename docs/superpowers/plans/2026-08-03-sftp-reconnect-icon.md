# SFTP Reconnect Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the SFTP reconnect and directory refresh buttons distinct icons without changing either action's behavior or visibility.

**Architecture:** Keep the change inside `src/app/ui.rs`. Define explicit private icon constants for the two toolbar actions, wire the existing buttons to those constants, and add a unit test that locks both mappings.

**Tech Stack:** Rust 2024, GPUI, `gpui-component`, Cargo.

---

## File Map

- Modify `src/app/ui.rs`: define the reconnect and refresh icon mappings, use them in the existing toolbar buttons, and add focused regression coverage.
- Create `docs/superpowers/plans/2026-08-03-sftp-reconnect-icon.md`: record the test-first implementation and local build procedure.

### Task 1: Add the icon-mapping regression test

**Files:**
- Modify: `src/app/ui.rs:3971-3985`

- [ ] **Step 1: Add a failing unit test for both toolbar mappings**

Add this test to the existing `tests` module:

```rust
#[test]
fn sftp_toolbar_uses_distinct_reconnect_and_refresh_icons() {
    assert!(matches!(SFTP_RECONNECT_ICON, IconName::Network));
    assert!(matches!(SFTP_REFRESH_ICON, IconName::Redo2));
}
```

- [ ] **Step 2: Run the test and verify the red state**

Run:

```powershell
cargo test --locked app::ui::tests::sftp_toolbar_uses_distinct_reconnect_and_refresh_icons
```

Expected: compilation fails because `SFTP_RECONNECT_ICON` and `SFTP_REFRESH_ICON` do not exist yet. This proves the test is tied to the new explicit mapping.

### Task 2: Implement the minimal icon change

**Files:**
- Modify: `src/app/ui.rs:1-20`
- Modify: `src/app/ui.rs:249-266`

- [ ] **Step 1: Define the private mappings**

Add these constants near the other module-level UI constants:

```rust
const SFTP_RECONNECT_ICON: IconName = IconName::Network;
const SFTP_REFRESH_ICON: IconName = IconName::Redo2;
```

- [ ] **Step 2: Wire the existing buttons to the mappings**

Keep every other button property unchanged and replace only the icon expressions:

```rust
Button::new("sftp-reconnect")
    .ghost()
    .small()
    .icon(SFTP_RECONNECT_ICON)
    .tooltip(t!("sftp_retry_now").to_string())
```

```rust
Button::new("sftp-refresh")
    .ghost()
    .small()
    .icon(SFTP_REFRESH_ICON)
    .tooltip(t!("refresh").to_string())
```

- [ ] **Step 3: Run the focused test and verify the green state**

Run:

```powershell
cargo test --locked app::ui::tests::sftp_toolbar_uses_distinct_reconnect_and_refresh_icons
```

Expected: one test passes with no failures.

### Task 3: Verify and build the local Release binary

**Files:**
- Verify: `src/app/ui.rs`
- Verify: `docs/superpowers/specs/2026-08-03-sftp-reconnect-icon-design.md`
- Verify: `docs/superpowers/plans/2026-08-03-sftp-reconnect-icon.md`

- [ ] **Step 1: Format and run the complete test suite**

Run:

```powershell
cargo fmt
cargo test --locked
```

Expected: formatting succeeds and every test passes.

- [ ] **Step 2: Run build-quality checks**

Run:

```powershell
cargo check --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
git diff --check
```

Expected: every command exits with code `0`.

- [ ] **Step 3: Build the local optimized executable**

Run:

```powershell
cargo build --locked --release
```

Expected: the build exits with code `0` and creates `target/release/jshell.exe`.

- [ ] **Step 4: Inspect the artifact and scoped diff**

Run:

```powershell
Get-Item target/release/jshell.exe | Select-Object FullName,Length,LastWriteTime
git diff -- src/app/ui.rs docs/superpowers/plans/2026-08-03-sftp-reconnect-icon.md
git status --short --branch
```

Expected: the executable exists; the diff contains only the approved icon mapping, test, and implementation plan.

### Task 4: Commit without merging

**Files:**
- Commit: `src/app/ui.rs`
- Commit: `docs/superpowers/plans/2026-08-03-sftp-reconnect-icon.md`

- [ ] **Step 1: Stage the verified files**

Run:

```powershell
git add src/app/ui.rs docs/superpowers/plans/2026-08-03-sftp-reconnect-icon.md
```

- [ ] **Step 2: Create the fix commit**

Run:

```powershell
git commit -m "fix(ui): distinguish sftp reconnect icon"
```

- [ ] **Step 3: Confirm delivery state**

Run:

```powershell
git status --short --branch
git log -2 --oneline
```

Expected: the working tree is clean on `codex/sftp-reconnect-icon`; the design and fix commits are present; `main` remains unchanged and no merge or push is performed.
