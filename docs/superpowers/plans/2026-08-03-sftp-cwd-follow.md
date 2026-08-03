# SFTP Directory Follow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep each SFTP panel aligned with its active SSH terminal while bounding SFTP work and preserving SSH connection priority.

**Architecture:** Store a device-local three-state preference, cache the last valid CWD on each terminal, and coordinate automatic directory reads with a per-group single-flight state machine. Start SFTP only after the primary SSH shell is connected; automatic reads use the independent SFTP connection and request IDs, with a 3-second realtime degradation latch.

**Tech Stack:** Rust, GPUI, gpui-component settings/menu controls, tokio, russh, russh-sftp, serde, rust-i18n.

---

## File Map

- Create `src/sftp/cwd_follow.rs`: path parsing, trigger classification, request coordination, timeout latch, and focused `Ashell` integration helpers.
- Modify `src/sftp/mod.rs`: register the module and carry optional automatic request IDs through directory commands and results.
- Modify `src/terminal/mod.rs`: cache each terminal CWD, attach follow state to `SftpUiState`, and add directory-failure events.
- Modify `src/session/config.rs`: persist the device-local follow mode with a legacy-safe default.
- Modify `src/session/mod.rs`: defer SFTP startup and trigger switch-follow from centralized focus changes.
- Modify `src/app/mod.rs`: route terminal-title and SFTP result events into the coordinator and evaluate the 3-second deadline from the existing event pump.
- Modify `src/sftp/ops.rs`: distinguish manual navigation from automatic following and cancel superseded automatic intent.
- Modify `src/app/dialogs.rs`: add the three-option General settings control.
- Modify `locales/en.yml` and `locales/zh-CN.yml`: add mode labels, descriptions, and degradation status.
- Modify focused unit-test modules in the files above only after implementation feedback is complete.

### Task 1: Persist the local follow-mode preference

**Files:**
- Modify: `src/session/config.rs`

- [ ] **Step 1: Define the serialized mode**

Add beside `CursorStyle`:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SftpCwdSyncMode {
    Off,
    Realtime,
    #[default]
    #[serde(other)]
    OnTerminalSwitch,
}
```

- [ ] **Step 2: Add it to `ConfigFile` and its default**

```rust
#[serde(default)]
pub sftp_cwd_sync_mode: SftpCwdSyncMode,
```

Initialize it with `SftpCwdSyncMode::default()` in `ConfigFile::default`.
Update every complete `ConfigFile` fixture literal in the same file with the explicit default so the type remains constructible without changing portable-sync behavior.

- [ ] **Step 3: Add `ConfigStore` accessors**

```rust
pub fn sftp_cwd_sync_mode(&self) -> SftpCwdSyncMode {
    self.cache.sftp_cwd_sync_mode
}

pub fn set_sftp_cwd_sync_mode(&mut self, mode: SftpCwdSyncMode) {
    self.cache.sftp_cwd_sync_mode = mode;
}
```

- [ ] **Step 4: Include the field in the preference merge whitelist**

Add this to `save_merged_preferences` without adding the field to `PortablePreferencesV2`:

```rust
disk_config.sftp_cwd_sync_mode = local_config.sftp_cwd_sync_mode;
```

### Task 2: Add the isolated CWD-follow state machine

**Files:**
- Create: `src/sftp/cwd_follow.rs`
- Modify: `src/sftp/mod.rs`
- Modify: `src/terminal/mod.rs`

- [ ] **Step 1: Define trigger and request state types**

```rust
pub(crate) const SFTP_CWD_REALTIME_LIMIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CwdFollowTrigger {
    TerminalSwitch,
    Realtime,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingCwdFollow {
    pub(crate) path: String,
    pub(crate) trigger: CwdFollowTrigger,
}

#[derive(Clone, Debug)]
pub(crate) struct InFlightCwdFollow {
    pub(crate) request_id: u64,
    pub(crate) path: String,
    pub(crate) started_at: Instant,
    pub(crate) accept_result: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CwdFollowCompletion {
    pub(crate) accept_result: bool,
    pub(crate) next: Option<(u64, String)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SftpCwdFollowState {
    next_request_id: u64,
    ready: bool,
    realtime_paused: bool,
    in_flight: Option<InFlightCwdFollow>,
    pending: Option<PendingCwdFollow>,
}
```

- [ ] **Step 2: Implement explicit state transitions**

The state API must expose these operations:

```rust
pub(crate) fn enqueue(
    &mut self,
    path: String,
    trigger: CwdFollowTrigger,
    configured_mode: SftpCwdSyncMode,
    now: Instant,
) -> Option<(u64, String)>;

pub(crate) fn mark_ready(
    &mut self,
    configured_mode: SftpCwdSyncMode,
    now: Instant,
) -> Option<(u64, String)>;

pub(crate) fn mark_unavailable(&mut self);

pub(crate) fn complete(
    &mut self,
    request_id: u64,
    configured_mode: SftpCwdSyncMode,
    now: Instant,
) -> CwdFollowCompletion;

pub(crate) fn expire_realtime(
    &mut self,
    configured_mode: SftpCwdSyncMode,
    now: Instant,
) -> bool;

pub(crate) fn cancel_for_manual_navigation(&mut self);
pub(crate) fn resume_realtime(&mut self, now: Instant);
pub(crate) fn apply_configured_mode(&mut self, mode: SftpCwdSyncMode);
pub(crate) fn realtime_paused(&self) -> bool;
```

`enqueue` must permit switch triggers in `OnTerminalSwitch` and both triggers in `Realtime`; while realtime is paused it must permit switch triggers only. A busy state replaces `pending` with the newest path. `complete` returns whether that result may update the UI and, after clearing the matching in-flight request, starts the newest allowed pending request when SFTP is ready. `expire_realtime` latches once an in-flight automatic read reaches three seconds and drops a pending realtime-only update. `cancel_for_manual_navigation` clears pending intent and marks an in-flight result as unacceptable without pretending the remote operation was cancelled. `apply_configured_mode` removes pending triggers that the newly selected mode does not permit.

- [ ] **Step 3: Add parser and mode helpers**

```rust
pub(crate) fn parse_terminal_cwd(title: &str, home_dir: &str) -> Option<String>;
pub(crate) fn effective_cwd_sync_mode(
    configured: SftpCwdSyncMode,
    realtime_paused: bool,
) -> SftpCwdSyncMode;
```

The parser accepts `ASHELL_CWD:`, an absolute path after a conventional title colon, `/...`, `~`, and `~/...`; it returns `None` for relative or empty values.

- [ ] **Step 4: Attach runtime state**

Add `remote_cwd: Option<String>` to `TerminalTab`, initialized to `None`, and add this field to `SftpUiState`:

```rust
pub(crate) cwd_follow: crate::sftp::cwd_follow::SftpCwdFollowState,
```

### Task 3: Carry request identity through SFTP directory operations

**Files:**
- Modify: `src/sftp/mod.rs`
- Modify: `src/terminal/mod.rs`

- [ ] **Step 1: Make directory commands identify automatic reads**

Replace the tuple variant with:

```rust
SftpCommand::ListDir {
    path: String,
    request_id: Option<u64>,
    expected_generation: Option<u64>,
},
```

Keep `request_id: None` for manual and existing callers, bind UI-originated reads to the current SFTP generation, and add:

```rust
pub(crate) fn follow_dir(
    &self,
    path: String,
    request_id: u64,
    generation: u64,
) -> bool {
    self.send(SftpCommand::ListDir {
        path,
        request_id: Some(request_id),
        expected_generation: Some(generation),
    })
}
```

- [ ] **Step 2: Identify success and failure events**

Extend `BackendEvent::SftpEntries` with `request_id: Option<u64>` and add:

```rust
SftpListDirFailed {
    tab_id: String,
    generation: u64,
    request_id: Option<u64>,
    path: String,
    reason: String,
},
```

- [ ] **Step 3: Emit one terminal event for every directory request**

Change `emit_entries` to accept `request_id: Option<u64>` and pass it through success. In the `ListDir` command arm, emit `SftpListDirFailed` on error instead of leaving automatic state unresolved. Initial-home and existing manual calls pass `None`; automatic calls pass their ID.

- [ ] **Step 4: Coalesce replayable automatic commands while disconnected**

Before adding an automatic `ListDir` to the supervisor's `pending` queue, remove older pending automatic `ListDir` values. Do not coalesce manual navigation or refresh requests.

Before remote I/O, discard UI-originated directory commands whose expected generation does not match the worker generation. During an unavailable generation, manual navigation updates the latest UI path without sending another directory command; connected fallback reloads that path once.

### Task 4: Wire title detection, switching, and request completion

**Files:**
- Modify: `src/sftp/cwd_follow.rs`
- Modify: `src/app/mod.rs`
- Modify: `src/session/mod.rs`
- Modify: `src/sftp/ops.rs`

- [ ] **Step 1: Cache valid CWD events**

In `BackendEvent::TerminalTitleChanged`, update `dynamic_title`, parse against the group's SFTP home, and replace `remote_cwd` only when parsing succeeds. If that tab is active, enqueue a `Realtime` trigger.

- [ ] **Step 2: Centralize terminal-switch following**

After `focus_pane_path` changes `active_tab`, and after `activate_group` restores its focused terminal, call:

```rust
self.follow_active_terminal_cwd(CwdFollowTrigger::TerminalSwitch);
```

The helper resolves the active terminal's cached CWD and active group, then calls the per-group coordinator. It must do nothing for local/serial terminals, missing CWD, or `Off` mode.

- [ ] **Step 3: Dispatch only outside mutable group borrows**

Coordinator methods return `Option<(group_id, request_id, path)>`. After releasing the mutable `tab_groups` borrow, look up `sftp_handles[group_id]` and call `follow_dir`. If sending fails, restore the path as the latest pending intent and mark SFTP unavailable.

- [ ] **Step 4: Complete or fail automatic requests by ID**

For `SftpEntries` with an ID, accept and display entries only when the current generation and in-flight ID match and `accept_result` is true. For `SftpListDirFailed`, leave `current_path` unchanged, update status, complete the matching state entry, and dispatch the newest allowed pending path.

- [ ] **Step 5: Preserve manual control**

At the start of `navigate_sftp`, call `cancel_for_manual_navigation`. Update the existing manual `sync_cwd_from_terminal` to prefer `TerminalTab::remote_cwd`, with the existing title parser as a fallback, then use normal manual navigation.

- [ ] **Step 6: Evaluate the three-second latch without another polling channel**

From the existing 16 ms application event pump, scan only in-memory in-flight timestamps:

```rust
let cwd_follow_changed = this.expire_sftp_cwd_follow_requests(Instant::now());
```

Include the result in the existing notification condition. This is a local deadline check; it performs no network I/O.

### Task 5: Start SFTP only after the SSH shell is ready

**Files:**
- Modify: `src/session/mod.rs`
- Modify: `src/app/mod.rs`

- [ ] **Step 1: Remove eager SFTP startup**

Keep creating the group's `SftpUiState` in `open_ssh_session`, but remove the immediate `spawn_sftp` call and handle insertion.

- [ ] **Step 2: Add an idempotent starter**

```rust
pub(crate) fn ensure_sftp_started_for_terminal(&mut self, tab_id: &str) {
    // Resolve the SSH tab, its group, and cloned Session first.
    // Return when the group already has a handle.
    // Spawn with the current proxy snapshot and insert by group ID.
}
```

Call it only after a matching `BackendEvent::Connected` marks the SSH tab connected. Multiple connected events or split terminals must not create a second SFTP supervisor for the same group.

For a manually retried SSH tab with an existing SFTP supervisor, record the group and exact terminal ID. Consume that record only after the matching terminal emits `Connected`, then restart the shared SFTP supervisor once. Closing the terminal or explicitly reconnecting SFTP clears the deferred record.

- [ ] **Step 3: Coordinate reconnect state**

On `SftpGeneration`, call `mark_unavailable`. On `SftpStatus` connected, call `mark_ready` and dispatch the latest pending CWD if one exists; otherwise preserve the existing current-directory refresh behavior.

### Task 6: Add the settings control and localized status

**Files:**
- Modify: `src/app/dialogs.rs`
- Modify: `src/sftp/cwd_follow.rs`
- Modify: `locales/en.yml`
- Modify: `locales/zh-CN.yml`

- [ ] **Step 1: Add a dedicated General settings group**

Insert an `SSH / SFTP` group before Plugins. Use a small dropdown button with three checked `PopupMenuItem` choices. Each choice calls:

```rust
this.set_sftp_cwd_sync_mode(mode, cx);
```

- [ ] **Step 2: Apply selection semantics**

`set_sftp_cwd_sync_mode` updates `ConfigStore`, persists with `save_preferences_background`, clears incompatible pending work for all groups, and, when `Realtime` is selected, clears the active group's degradation latch and follows the active terminal immediately.

- [ ] **Step 3: Add localized strings**

Add matching English and Simplified Chinese keys for the group title, setting title/description, all three mode labels, and the paused status. The Chinese paused status must state that the current session stays in switch-follow mode until realtime is manually re-enabled.

### Task 7: Static implementation review checkpoint

**Files:**
- Review only the files listed above and the already changed SFTP handshake files.

- [ ] **Step 1: Inspect the working diff without formatting or compiling**

Run only read-only commands such as `git diff --stat`, `git diff`, and `git status --short`. Confirm that no portable sync schema, unrelated UI, or release metadata changed.

- [ ] **Step 2: Present the unverified diff summary**

Report exact changed files, explicit behavior, and known unverified risks to the user. Stop for further modification feedback.

### Task 8: Deferred targeted verification and branch commit

**Files:**
- Add focused tests next to `src/sftp/cwd_follow.rs`, `src/session/config.rs`, `src/terminal/mod.rs`, and affected SFTP command tests.
- Update the release changelog requested by the user before the final commit.

- [ ] **Step 1: Add focused tests after implementation feedback is complete**

Add named tests with explicit assertions:

```rust
#[test]
fn parses_supported_terminal_cwd_titles_and_rejects_relative_titles();

#[test]
fn legacy_config_defaults_to_terminal_switch_follow();

#[test]
fn follow_state_keeps_one_in_flight_and_replaces_the_pending_path();

#[test]
fn stale_request_ids_cannot_update_the_visible_directory();

#[test]
fn manual_navigation_invalidates_an_automatic_result();

#[test]
fn realtime_pauses_at_three_seconds_and_requires_manual_resume();

#[test]
fn sftp_start_is_idempotent_after_ssh_connected();
```

The config persistence fixture must serialize and decrypt `sftp_cwd_sync_mode: realtime`, while the legacy fixture omits the field and asserts `OnTerminalSwitch`.

- [ ] **Step 2: Run targeted tests**

Run:

```powershell
cargo test cwd_follow
cargo test legacy_config_defaults_to_terminal_switch_follow
cargo test sftp_start_is_idempotent_after_ssh_connected
```

Expected result: each command exits with code 0 and every selected test passes.

- [ ] **Step 3: Run formatting and compile checks**

Run:

```powershell
cargo fmt --check
cargo check
cargo build --release
```

Expected result: all commands exit with code 0 and `target/release/jshell.exe` is produced.

- [ ] **Step 4: Write the complete Simplified Chinese release changelog**

Document the reconnect-icon distinction, tolerant SFTP handshake prefix handling and stopped retry behavior, SSH-first SFTP startup, directory-follow modes, request coalescing, and three-second degradation semantics.

- [ ] **Step 5: Commit on the existing branch without merging**

Stage only the reviewed feature files and commit them on `codex/sftp-reconnect-icon`. Do not merge or push unless separately requested.
