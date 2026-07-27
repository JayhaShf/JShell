# Compact Workspace Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a denser Windows workspace with a narrow navigation rail, compact sidebar telemetry and sessions, a smaller bottom SFTP header, and a rewritten Chinese README.

**Architecture:** Reuse the existing GPUI layout, SFTP operations, telemetry sampler, and saved-session connection flow. The layout changes remain in `src/app/ui.rs` and constants; a small session helper opens multiple saved sessions while preserving the existing no-duplicate-tab rule.

**Tech Stack:** Rust, GPUI, gpui-component, Cargo tests, Markdown.

## Global Constraints

- Keep the existing Noto Sans CJK SC defaults and the three allowed themes.
- Do not alter SSH/SFTP protocol behavior or remove existing settings.
- The release must pass `cargo fmt --check`, `cargo test`, and `cargo build --release`.

---

### Task 1: Compact Navigation and SFTP Chrome

**Files:**
- Modify: `src/app/constants.rs`
- Modify: `src/app/ui.rs`

- [ ] Reduce the collapsed navigation rail to 48px and default expanded sidebar width to 260px.
- [ ] Reduce sidebar padding, heading scale, and saved-session row padding without changing click and context-menu behavior.
- [ ] Reduce the bottom SFTP header to 30px and use icon-first controls for refresh, hidden files, upload, download, and collapse.
- [ ] Run `cargo fmt --check`.

### Task 2: Batch Saved-Session Connection

**Files:**
- Modify: `src/session/mod.rs`
- Modify: `src/app/ui.rs`
- Test: `src/session/mod.rs`

- [ ] Add a failing test for deriving only disconnected saved-session IDs from the open tabs.
- [ ] Add the smallest helper needed by `connect_saved_sessions` to preserve existing open tabs.
- [ ] Add an icon action beside the saved-session heading that opens all saved SSH/serial sessions.
- [ ] Run `cargo test` and confirm the new test passes.

### Task 3: Release Documentation and Preview

**Files:**
- Create: `assets/screenshots/compact-workspace.png`
- Modify: `README.md`

- [ ] Copy the verified workspace preview into the repository as the README image.
- [ ] Replace the Chinese README with concise build, run, configuration, feature, and acknowledgement sections.
- [ ] Credit TomZz and link the original upstream project.
- [ ] Run `git diff --check`.

### Task 4: Release Verification

**Files:**
- Verify: `target/release/ashell.exe`

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo test`.
- [ ] Run `cargo build --release` after closing any running release binary.
- [ ] Stage only source, assets, docs, and README changes; exclude `.superpowers`.
- [ ] Commit the completed release changes.
