# SSH Accept-New Host Key Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow first-time SSH/SFTP connections to persist a genuinely new server host key automatically, while blocking changed or unsafe keys and publishing the behavior in the `v0.1.0-beta.3` Chinese Release Notes.

**Architecture:** `session::host_keys` remains the single verification boundary and gains structured outcomes, localized permanent errors, and a process-wide check-and-append transaction. SSH and SFTP continue to use the same verifier; SFTP adds a blocked state for permanent host-key failures so it waits for an explicit reconnect instead of retrying forever. The release workflow reads a tag-specific checked-in Chinese notes file.

**Tech Stack:** Rust 2024, `russh`/`ssh-key`, `anyhow`, `thiserror`, GPUI backend events, `rust-i18n`, GitHub Actions, GitHub CLI.

**Execution Constraint:** The user asked to defer command-based verification until all implementation and release files are complete. Write regression tests before the corresponding production changes, but do not run targeted red/green commands during Tasks 1-6. Run the complete verification set once in Task 7. Do not perform a separate full-project audit and do not run `cargo audit` locally.

**Status:** Tasks 1-7 are complete, including mixed-record fail-closed regression coverage and the consolidated verification pass. Task 8 is in progress.

---

## File Map

- Modify `src/session/host_keys.rs`: classify known-host records, format fingerprints, append new keys safely, serialize concurrent SSH/SFTP verification, and expose permanent-error detection.
- Modify `src/backend/ssh.rs`: update the handler regression test for accept-new behavior.
- Modify `src/sftp/connection.rs`: add a blocked state and explicit manual-reconnect transition.
- Modify `src/sftp/mod.rs`: classify permanent host-key failures, stop automatic retry, queue replayable work while blocked, and update handler tests.
- Modify `src/terminal/mod.rs`: add a typed SFTP blocked event.
- Modify `src/app/mod.rs`: display the blocked status and mark related remote documents offline.
- Modify `locales/en.yml` and `locales/zh-CN.yml`: add host-key and SFTP blocked messages.
- Modify `Cargo.toml` and `Cargo.lock`: bump the package version to `0.1.0-beta.3`.
- Modify `README.md` and `README.en.md`: update the source version and remove stale hardcoded “latest public release” claims.
- Modify `.github/workflows/release.yml`: use tag-specific Chinese Release Notes.
- Create `.github/release-notes/v0.1.0-beta.3.zh-CN.md`: complete release body.
- Keep `docs/RELEASE_AUDIT_0.1.0-beta.1.md`, `docs/RELEASE_AUDIT_0.1.0-beta.2.md`, and `AUDIT_REPORT.md` unchanged.

---

### Task 1: Add Host-Key Regression Tests Before Production Changes

**Files:**
- Modify: `src/session/host_keys.rs:182`

- [ ] **Step 1: Replace the temporary-file helper with an isolated directory helper**

Use `tempfile::TempDir` so tests can cover a missing `.ssh` directory as well as an existing file:

```rust
struct TempKnownHosts {
    _root: tempfile::TempDir,
    path: PathBuf,
}

impl TempKnownHosts {
    fn existing(contents: &str) -> Self {
        let root = tempfile::tempdir().expect("create temporary known_hosts root");
        let path = root.path().join("known_hosts");
        fs::write(&path, contents).expect("write temporary known_hosts");
        Self { _root: root, path }
    }

    fn missing() -> Self {
        let root = tempfile::tempdir().expect("create temporary known_hosts root");
        let path = root.path().join(".ssh").join("known_hosts");
        Self { _root: root, path }
    }

    fn contents(&self) -> String {
        fs::read_to_string(&self.path).expect("read temporary known_hosts")
    }
}
```

- [ ] **Step 2: Add fixed key fixtures for same-algorithm and cross-algorithm conflicts**

Keep the two Ed25519 fixtures and add this valid ECDSA public key:

```rust
const ECDSA_KEY: &str =
    "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBHwf2HMM5TRXvo2SQJjsNkiDD5KqiiNjrGVv3UUh+mMT5RHxiRtOnlqvjhQtBq0VpmpCV/PwUdhOig4vkbqAcEc=";
```

- [ ] **Step 3: Replace unknown/missing rejection tests with accept-new tests**

Add tests with these exact behaviors:

```rust
#[test]
fn accepts_and_persists_new_host_in_existing_file() {
    let known_hosts = TempKnownHosts::existing(&format!("other.test {KNOWN_KEY}\n"));

    let result = verify_server_key_at_path(
        "new.test",
        22,
        &public_key(CHANGED_KEY),
        &known_hosts.path,
    )
    .expect("new host should be accepted");

    assert_eq!(result, HostKeyVerification::AcceptedNew);
    let contents = known_hosts.contents();
    assert!(contents.starts_with(&format!("other.test {KNOWN_KEY}\n")));
    assert_eq!(contents.matches("new.test ssh-ed25519 ").count(), 1);
}

#[test]
fn accepts_and_creates_missing_known_hosts_file() {
    let known_hosts = TempKnownHosts::missing();

    assert_eq!(
        verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect("missing known_hosts should be created"),
        HostKeyVerification::AcceptedNew,
    );

    let contents = known_hosts.contents();
    assert!(contents.starts_with("example.test ssh-ed25519 "));
    assert!(!contents.starts_with('\n'));
    assert_eq!(
        verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect("persisted key should be trusted"),
        HostKeyVerification::Trusted,
    );
}
```

- [ ] **Step 4: Add persistence-format and idempotency tests**

Add:

```rust
#[test]
fn appends_after_file_without_trailing_newline() {
    let known_hosts = TempKnownHosts::existing(&format!("other.test {KNOWN_KEY}"));
    verify_server_key_at_path(
        "example.test",
        22,
        &public_key(CHANGED_KEY),
        &known_hosts.path,
    )
    .expect("append new host");

    let lines = known_hosts.contents().lines().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("other.test ssh-ed25519 "));
    assert!(lines[1].starts_with("example.test ssh-ed25519 "));
}

#[test]
fn writes_bracketed_target_for_non_default_port() {
    let known_hosts = TempKnownHosts::missing();
    verify_server_key_at_path(
        "example.test",
        2222,
        &public_key(KNOWN_KEY),
        &known_hosts.path,
    )
    .expect("append non-default port");

    assert!(known_hosts.contents().starts_with("[example.test]:2222 ssh-ed25519 "));
}

#[test]
fn does_not_duplicate_an_already_accepted_key() {
    let known_hosts = TempKnownHosts::missing();
    let verifier = || {
        verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect("verify host key")
    };

    assert_eq!(verifier(), HostKeyVerification::AcceptedNew);
    assert_eq!(verifier(), HostKeyVerification::Trusted);
    assert_eq!(known_hosts.contents().matches("example.test ssh-ed25519 ").count(), 1);
}
```

- [ ] **Step 5: Strengthen changed-key tests so every existing key conflicts regardless of algorithm**

Add one same-algorithm and one cross-algorithm test. Both must compare the file before and after:

```rust
#[test]
fn rejects_changed_key_without_modifying_file() {
    let original = format!("example.test {KNOWN_KEY}\n");
    let known_hosts = TempKnownHosts::existing(&original);

    let error = verify_server_key_at_path(
        "example.test",
        22,
        &public_key(CHANGED_KEY),
        &known_hosts.path,
    )
    .expect_err("changed key must be rejected");

    assert!(matches!(error, HostKeyError::Changed { .. }));
    assert_eq!(known_hosts.contents(), original);
}

#[test]
fn rejects_different_algorithm_for_known_target_without_modifying_file() {
    let original = format!("example.test {KNOWN_KEY}\n");
    let known_hosts = TempKnownHosts::existing(&original);

    let error = verify_server_key_at_path(
        "example.test",
        22,
        &public_key(ECDSA_KEY),
        &known_hosts.path,
    )
    .expect_err("a different key algorithm for an existing target is still a conflict");

    assert!(matches!(error, HostKeyError::Changed { .. }));
    assert_eq!(known_hosts.contents(), original);
}
```

- [ ] **Step 6: Add concurrent single-write coverage**

Use two threads and a barrier; assert one caller learns and the other trusts:

```rust
#[test]
fn concurrent_accept_new_writes_one_entry() {
    use std::sync::{Arc, Barrier};

    let known_hosts = TempKnownHosts::missing();
    let barrier = Arc::new(Barrier::new(2));
    let results = (0..2)
        .map(|_| {
            let path = known_hosts.path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                verify_server_key_at_path(
                    "example.test",
                    22,
                    &public_key(KNOWN_KEY),
                    &path,
                )
                .expect("concurrent verification")
            })
        })
        .map(|thread| thread.join().expect("verification thread"))
        .collect::<Vec<_>>();

    assert!(results.contains(&HostKeyVerification::AcceptedNew));
    assert!(results.contains(&HostKeyVerification::Trusted));
    assert_eq!(known_hosts.contents().matches("example.test ssh-ed25519 ").count(), 1);
}
```

- [ ] **Step 7: Convert revoked, negated, certificate-authority, malformed, hashed, wildcard, and non-default-port assertions to structured outcomes**

Use `matches!` against `HostKeyError::{Revoked, Excluded, UnsupportedMarker, InvalidEntry}` and exact `HostKeyVerification::Trusted` assertions. For every rejected case, save the original bytes and assert the file remains unchanged.

- [ ] **Step 8: Do not run tests yet**

Per the execution constraint, leave the new tests unexecuted until Task 7.

---

### Task 2: Implement Structured Accept-New Verification

**Files:**
- Modify: `src/session/host_keys.rs:1-180`
- Modify: `locales/en.yml:160-175`
- Modify: `locales/zh-CN.yml:160-175`

- [ ] **Step 1: Add verification, pattern, and check-result types**

Introduce:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostKeyVerification {
    Trusted,
    AcceptedNew,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostPatternMatch {
    None,
    Positive,
    Negated,
}

#[derive(Debug)]
enum HostKeyCheck {
    Trusted,
    Unknown,
    Changed { stored: Vec<String> },
    Revoked { line: usize, fingerprint: String },
    Excluded { line: usize },
    UnsupportedMarker { line: usize, marker: String },
}
```

- [ ] **Step 2: Add a downcastable permanent error type with localized display text**

Create `pub(crate) enum HostKeyError` with variants:

```rust
HomeDirectoryUnavailable,
ReadKnownHosts { target: String, path: PathBuf, source: std::io::Error },
InvalidEntry { target: String, path: PathBuf, line: usize, source: russh::keys::ssh_key::Error },
Changed { target: String, path: PathBuf, stored: String, received: String },
Revoked { target: String, path: PathBuf, line: usize, fingerprint: String },
Excluded { target: String, path: PathBuf, line: usize },
UnsupportedMarker { target: String, path: PathBuf, line: usize, marker: String },
EncodeKey { target: String, path: PathBuf, source: russh::keys::ssh_key::Error },
CreateDirectory { path: PathBuf, source: std::io::Error },
WriteKnownHosts { path: PathBuf, source: std::io::Error },
```

Implement `Display` with `rust_i18n::t!`, implement `std::error::Error::source` for variants carrying an underlying error, and expose:

```rust
pub(crate) fn is_permanent_host_key_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<HostKeyError>().is_some())
}
```

- [ ] **Step 3: Add localized messages including the approved changed-key prompt**

Add these keys in both locale files:

```yaml
ssh_host_key_changed: "SSH host key changed; connection blocked.\nTarget: %{target}\nSaved: %{stored}\nReceived: %{received}\nKnown hosts: %{path}\nVerify the server fingerprint. If the server intentionally changed keys, update or remove the old record and reconnect. JShell will not overwrite it automatically."
ssh_host_key_revoked: "SSH host key is revoked; connection blocked. Target: %{target}; fingerprint: %{fingerprint}; known_hosts: %{path}:%{line}"
ssh_host_key_excluded: "SSH host is excluded by known_hosts; connection blocked. Target: %{target}; known_hosts: %{path}:%{line}"
ssh_host_key_marker_unsupported: "SSH host key marker %{marker} cannot be used as a direct host key. Target: %{target}; known_hosts: %{path}:%{line}"
ssh_known_hosts_read_failed: "Could not read known_hosts for %{target}: %{path} (%{error})"
ssh_known_hosts_invalid_entry: "Invalid known_hosts entry for %{target}: %{path}:%{line} (%{error})"
ssh_known_hosts_write_failed: "Could not save the SSH host key to %{path}: %{error}"
```

The `zh-CN` value for `ssh_host_key_changed` must be:

```yaml
ssh_host_key_changed: "SSH 主机密钥已发生变化，连接已阻止。\n目标：%{target}\n已保存：%{stored}\n当前收到：%{received}\n记录文件：%{path}\n请先核验服务器指纹。确认服务器已更换密钥后，手动删除或更新该目标的旧记录，再重新连接。JShell 不会自动覆盖旧密钥。"
ssh_host_key_revoked: "SSH 主机密钥已被吊销，连接已阻止。目标：%{target}；指纹：%{fingerprint}；记录位置：%{path}:%{line}"
ssh_host_key_excluded: "目标被 known_hosts 否定规则排除，连接已阻止。目标：%{target}；记录位置：%{path}:%{line}"
ssh_host_key_marker_unsupported: "主机密钥标记 %{marker} 不能作为直接主机密钥使用。目标：%{target}；记录位置：%{path}:%{line}"
ssh_known_hosts_read_failed: "无法读取 %{target} 的 known_hosts：%{path}（%{error}）"
ssh_known_hosts_invalid_entry: "known_hosts 中存在无效记录。目标：%{target}；位置：%{path}:%{line}（%{error}）"
ssh_known_hosts_write_failed: "无法将 SSH 主机密钥保存到 %{path}：%{error}"
```

- [ ] **Step 4: Make target and fingerprint formatting single-source**

Add:

```rust
fn known_hosts_target(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn key_fingerprint(key: &PublicKey) -> String {
    format!(
        "{} {}",
        key.algorithm().as_str(),
        key.fingerprint(russh::keys::HashAlg::Sha256)
    )
}
```

- [ ] **Step 5: Replace boolean host-pattern matching with three-way matching**

`HostPatterns::HashedName` returns `Positive` or `None`. `HostPatterns::Patterns` returns `Negated` when a matching `!pattern` is found, otherwise `Positive` when any positive pattern matches, otherwise `None`. Preserve case-insensitive regular matching and original/lowercase hashed-host attempts.

- [ ] **Step 6: Implement read-only classification**

Add:

```rust
fn check_server_key_at_path(
    host: &str,
    port: u16,
    key: &PublicKey,
    path: &Path,
) -> Result<HostKeyCheck, HostKeyError>
```

Rules in order:

1. `NotFound` is an empty database and returns `Unknown`; every other read failure returns `ReadKnownHosts`.
2. Parse every non-comment line; malformed input returns `InvalidEntry` before any write.
3. Track negated matches, unsupported markers, matching regular keys, and all different regular/revoked keys for the target.
4. Final priority is `Revoked` for the presented key, then `Excluded`, then `UnsupportedMarker`, then `Changed` for any existing target key regardless of algorithm, then `Trusted`, then `Unknown`.
5. A trusted regular record does not override a blocking or conflicting record for the same target; an exact `@revoked` record remains the highest-priority failure.

- [ ] **Step 7: Implement append without rewriting existing content**

Add `append_server_key_at_path(host, port, key, path, existing_contents)`. Encode the key before opening the file, create the parent directory, open with `create + append`, and use Unix mode `0o600` for a newly created file. Build the complete append buffer first:

```rust
let mut record = String::new();
if !existing_contents.is_empty() && !existing_contents.ends_with('\n') {
    record.push('\n');
}
record.push_str(&known_hosts_target(host, port));
record.push(' ');
record.push_str(&key.to_openssh().map_err(|source| HostKeyError::EncodeKey {
    target: known_hosts_target(host, port),
    path: path.to_path_buf(),
    source,
})?);
record.push('\n');
file.write_all(record.as_bytes())?;
file.flush()?;
```

Map directory and file errors to `CreateDirectory` and `WriteKnownHosts`. Do not call russh’s current `learn_known_hosts_path`, because it inserts a leading blank line in a new empty file and does not expose the required structured errors.

- [ ] **Step 8: Serialize the full check-and-append transaction**

Add a module-level lock shared by every verifier:

```rust
static KNOWN_HOSTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

Recover a poisoned lock with `unwrap_or_else(std::sync::PoisonError::into_inner)`. Hold the guard while reading, classifying, appending, and flushing. Map `Trusted` to `HostKeyVerification::Trusted`, map `Unknown` to append plus `AcceptedNew`, and convert the other checks to `HostKeyError`.

- [ ] **Step 9: Change verifier signatures**

Use:

```rust
pub(crate) fn new(host: &str, port: u16) -> Result<Self, HostKeyError>

pub(crate) fn verify(
    &self,
    key: &PublicKey,
) -> Result<HostKeyVerification, HostKeyError>
```

The SSH and SFTP handlers may continue discarding the successful verification enum and returning `Ok(true)`.

- [ ] **Step 10: Do not run tests yet**

Continue to Task 3 without executing Cargo commands.

---

### Task 3: Update SSH and SFTP Handler Contract Tests

**Files:**
- Modify: `src/backend/ssh.rs:823-850`
- Modify: `src/sftp/mod.rs:2378-2410`

- [ ] **Step 1: Replace the terminal handler rejection test**

Rename it to `client_handler_accepts_and_records_new_host_key`. After `check_server_key`, assert `Ok(true)`, assert the file exists, and assert it contains exactly one `[example.test]:2222` record.

- [ ] **Step 2: Replace the SFTP handler rejection test**

Rename it to `sftp_handler_accepts_and_records_new_host_key` with the same assertions for `[files.example.test]:2222`.

- [ ] **Step 3: Add a handler conflict test**

Create an existing file containing `KNOWN_KEY`, present `CHANGED_KEY`, and assert the handler error chain contains `HostKeyError`:

```rust
let error = russh::client::Handler::check_server_key(&mut handler, &changed_key)
    .await
    .expect_err("changed host key must fail");
assert!(error.downcast_ref::<HostKeyError>().is_some());
assert_eq!(fs::read_to_string(path).expect("read known_hosts"), original);
```

- [ ] **Step 4: Do not run tests yet**

Continue to the SFTP state-machine task.

---

### Task 4: Add a Blocked SFTP Connection State

**Files:**
- Modify: `src/sftp/connection.rs:21-160`
- Test: `src/sftp/connection.rs:162-250`

- [ ] **Step 1: Write blocked-state tests**

Add:

```rust
#[test]
fn permanent_failure_waits_for_manual_reconnect() {
    let mut supervisor = ConnectionSupervisor::new();
    let generation = supervisor.generation();

    assert!(supervisor.block(generation));
    assert_eq!(supervisor.state(), ConnectionState::Blocked);
    assert_eq!(supervisor.generation(), generation.next());
    assert_eq!(supervisor.begin_connecting(), None);
    assert!(supervisor.disconnect(supervisor.generation()).is_none());
}

#[test]
fn manual_reconnect_resumes_blocked_connection() {
    let mut supervisor = ConnectionSupervisor::new();
    let generation = supervisor.generation();
    assert!(supervisor.block(generation));

    let resumed = supervisor
        .manual_reconnect()
        .expect("blocked connection can be manually retried");

    assert_eq!(supervisor.state(), ConnectionState::Connecting);
    assert_eq!(resumed, generation.next());
    assert_eq!(supervisor.begin_connecting(), Some(resumed));
}
```

- [ ] **Step 2: Add `ConnectionState::Blocked` and transitions**

Implement:

```rust
pub fn block(&mut self, generation: SftpGeneration) -> bool {
    if matches!(self.state, ConnectionState::Closed | ConnectionState::Blocked)
        || self.generation != generation
    {
        return false;
    }
    self.state = ConnectionState::Blocked;
    self.generation = self.generation.next();
    true
}

pub fn manual_reconnect(&mut self) -> Option<SftpGeneration> {
    if self.state == ConnectionState::Closed {
        return None;
    }
    self.state = ConnectionState::Connecting;
    self.backoff = ReconnectBackoff::new();
    Some(self.generation)
}

pub const fn is_blocked(&self) -> bool {
    self.state == ConnectionState::Blocked
}
```

Make `begin_connecting`, `mark_connected`, and `disconnect` reject the blocked state. `close()` remains terminal and still disables manual reconnect.

- [ ] **Step 3: Do not run tests yet**

Continue without invoking Cargo.

---

### Task 5: Stop SFTP Auto-Retry for Permanent Host-Key Errors

**Files:**
- Modify: `src/sftp/mod.rs:481-635`
- Modify: `src/terminal/mod.rs:46-120`
- Modify: `src/app/mod.rs:1320-1366`
- Modify: `locales/en.yml:1-20`
- Modify: `locales/zh-CN.yml:1-20`

- [ ] **Step 1: Write retry-policy tests**

Add a private enum and tests in `src/sftp/mod.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SftpRetryPolicy {
    Backoff,
    Manual,
}

#[test]
fn host_key_errors_require_manual_reconnect() {
    const CHANGED_HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    let root = tempfile::tempdir().expect("create temporary known_hosts root");
    let path = root.path().join("known_hosts");
    fs::write(&path, format!("example.test {TEST_HOST_KEY}\n"))
        .expect("write known_hosts");
    let verifier = HostKeyVerifier::with_known_hosts_path("example.test", 22, path);
    let changed = russh::keys::ssh_key::PublicKey::from_openssh(CHANGED_HOST_KEY)
        .expect("parse changed host key");
    let error = anyhow::Error::new(
        verifier.verify(&changed).expect_err("changed key must fail"),
    )
    .context("connect example.test:22 failed");

    assert_eq!(sftp_retry_policy(&error), SftpRetryPolicy::Manual);
}

#[test]
fn ordinary_connection_errors_keep_backoff() {
    assert_eq!(
        sftp_retry_policy(&anyhow!("network reset")),
        SftpRetryPolicy::Backoff,
    );
}
```

If sharing `TempKnownHosts` across modules would add test-only coupling, create a local temporary file directly with `tempfile::tempdir()` in this test.

- [ ] **Step 2: Implement retry classification through the full anyhow source chain**

```rust
fn sftp_retry_policy(error: &anyhow::Error) -> SftpRetryPolicy {
    if crate::session::host_keys::is_permanent_host_key_error(error) {
        SftpRetryPolicy::Manual
    } else {
        SftpRetryPolicy::Backoff
    }
}
```

- [ ] **Step 3: Add a typed backend event**

Add to `BackendEvent`:

```rust
SftpConnectionBlocked {
    tab_id: String,
    generation: u64,
    reason: String,
},
```

- [ ] **Step 4: Branch worker failure handling by retry policy**

For `Ok(Err(error))`, keep the detailed `{error:#}` reason. On `Backoff`, retain the existing `disconnect` and timer behavior. On `Manual`:

1. Call `connection.block(active_worker.generation)`.
2. Send `BackendEvent::SftpConnectionBlocked` using the failed worker’s generation so the current UI generation accepts it.
3. Do not create `retry_timer`.

- [ ] **Step 5: Add a blocking command branch instead of busy-yielding**

Before the final `else { tokio::task::yield_now().await; }`, add an `else if connection.is_blocked()` branch which awaits `commands.recv()`:

- `None` or `Close`: exit.
- `ReconnectNow`: call `connection.manual_reconnect()`; the next loop spawns a worker with the advanced generation.
- Replayable commands: pass to `queue_pending_command` with `connection.generation()`.
- Non-replayable commands: pass to `reject_unavailable_command`.

Remove the now-unreachable busy-yield branch if every live state awaits a worker, timer, or command.

- [ ] **Step 6: Handle the typed event in the app**

On `SftpConnectionBlocked`:

1. Format `t!("sftp_reconnect_blocked", reason = reason.clone())`.
2. Update the matching `SftpUiState.status` when the generation is current.
3. Set every related document’s `DocumentConnectionState` to `Offline(reason.clone())`.
4. Update the global status when the connection group is active.

Keep `SftpStatus` string-prefix handling for ordinary connected/reconnecting states.

- [ ] **Step 7: Add localized blocked text**

```yaml
sftp_reconnect_blocked: "SFTP host-key verification failed; automatic reconnect stopped: %{reason}"
```

```yaml
sftp_reconnect_blocked: "SFTP 主机密钥校验失败，已停止自动重连：%{reason}"
```

- [ ] **Step 8: Do not run tests yet**

All functional code is now present; proceed to release preparation.

---

### Task 6: Prepare `v0.1.0-beta.3` and Chinese Release Notes

**Files:**
- Modify: `Cargo.toml:3`
- Modify: `Cargo.lock:3978`
- Modify: `README.md:1-55`
- Modify: `README.en.md:1-35`
- Modify: `.github/workflows/release.yml:242-264`
- Create: `.github/release-notes/v0.1.0-beta.3.zh-CN.md`

- [ ] **Step 1: Bump only the root package version**

Change both package entries from `0.1.0-beta.2` to `0.1.0-beta.3`. Do not refresh unrelated dependency versions.

- [ ] **Step 2: Remove stale release-status prose from both READMEs**

State that the source version is `0.1.0-beta.3` and direct users to GitHub Releases for available binaries. Do not hardcode which public release is newest, so the README does not become false immediately after publication.

- [ ] **Step 3: Create complete Simplified Chinese release notes**

Use this structure and include all points:

```markdown
# JShell v0.1.0-beta.3

## 本次更新

- SSH 与 SFTP 首次连接真正未知的主机时，会自动将服务器公钥写入用户的 `~/.ssh/known_hosts`，随后继续连接。
- SSH 终端与 SFTP 同时握手时，共享串行化的检查与写入流程，同一目标只会生成一条记录。
- 同一主机和端口存在不同旧密钥时，无论密钥算法是否相同，都会阻止连接并显示已保存与当前收到的 SHA-256 指纹。
- 永久主机密钥错误不再触发 SFTP 无限退避重试；核验并修正记录后，可使用重新连接操作再次尝试。

## 安全规则

- JShell 采用 `accept-new` 语义，只自动接受没有历史记录的新目标。
- 不会自动覆盖、删除或追加与现有目标冲突的密钥。
- `@revoked`、否定主机模式、`@cert-authority` 直接密钥、损坏文件和权限错误仍会失败关闭。
- 不提供关闭主机密钥校验或忽略密钥变化的选项。

## 记录格式

- 默认端口 22 使用 `host`。
- 非默认端口使用 `[host]:port`，因此同一主机的不同端口互不影响。
- 保存的内容仅包含服务器公钥，不包含用户名、密码、客户端私钥或会话配置。

## 升级与故障处理

首次连接不会弹出指纹确认窗口。建议在高安全环境中通过独立渠道核验服务器公布的指纹。

如果服务器已确认更换密钥，请先核验新指纹，再更新对应记录：

```powershell
ssh-keygen -R example.com
ssh-keygen -R "[example.com]:2222"
```

完成后在 JShell 中重新连接，新密钥会作为新记录写入。
```

- [ ] **Step 4: Make the workflow require tag-specific Chinese notes**

In `publish.steps`, add the same pinned checkout action already used elsewhere, before downloading artifacts:

```yaml
- uses: actions/checkout@93cb6efe18208431cddfb8368fd83d5badbf9bfd # v5.0.1
  with:
    persist-credentials: false

- name: Verify Chinese release notes
  shell: bash
  run: |
    set -euo pipefail
    NOTES=".github/release-notes/${GITHUB_REF_NAME}.zh-CN.md"
    if [ ! -s "$NOTES" ]; then
      echo "::error::Missing or empty Chinese release notes: $NOTES"
      exit 1
    fi
```

Keep artifact download after checkout. Update `softprops/action-gh-release`:

```yaml
with:
  name: JShell ${{ github.ref_name }}
  prerelease: ${{ contains(github.ref_name, '-') }}
  body_path: .github/release-notes/${{ github.ref_name }}.zh-CN.md
  files: dist/**/*
  fail_on_unmatched_files: true
```

Remove `generate_release_notes: true`.

- [ ] **Step 5: Leave historical audit documents unchanged**

Do not edit beta.1/beta.2 audit records or `AUDIT_REPORT.md`; they describe previous candidates and the user explicitly excluded a new full-project audit.

---

### Task 7: Run One Consolidated Verification Pass

**Files:**
- Verify all modified files.

- [ ] **Step 1: Format the Rust code**

Run:

```powershell
cargo fmt
```

Expected: exit code `0`.

- [ ] **Step 2: Run the focused host-key, handler, and SFTP state tests**

Run:

```powershell
cargo test session::host_keys -- --nocapture
cargo test client_handler_accepts_and_records_new_host_key -- --nocapture
cargo test sftp_handler_accepts_and_records_new_host_key -- --nocapture
cargo test sftp::connection -- --nocapture
cargo test host_key_errors_require_manual_reconnect -- --nocapture
```

Expected: every command exits `0`; accept-new tests show one persisted record, conflict tests remain rejected, and blocked-state tests pass.

- [ ] **Step 3: Run the complete test suite**

Run:

```powershell
cargo test --locked
```

Expected: exit code `0`, no failed tests.

- [ ] **Step 4: Run build-quality checks without a separate audit**

Run:

```powershell
cargo check --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
```

Expected: all commands exit `0`. Do not run `cargo audit`; the existing hosted Release quality gate remains unchanged.

- [ ] **Step 5: Check repository and release metadata consistency**

Run:

```powershell
git diff --check
cargo pkgid --locked
$stale = Select-String -Path Cargo.toml,Cargo.lock,README.md,README.en.md,.github/release-notes/*.md,.github/workflows/release.yml -Pattern '0\.1\.0-beta\.2|v0\.1\.0-beta\.2'
if ($stale) {
    $stale
    throw "active release files still reference beta.2"
}
git status --short
```

Expected:

- `git diff --check` exits `0`.
- `cargo pkgid --locked` ends in `#jshell@0.1.0-beta.3`.
- The `rg` command finds no stale beta.2 reference in the listed active release files.
- `git status --short` lists only the intended feature, localization, documentation, version, and workflow files.

- [ ] **Step 6: Review the final diff by scope, not as a full audit**

Run:

```powershell
git diff -- src/session/host_keys.rs src/backend/ssh.rs src/sftp/connection.rs src/sftp/mod.rs src/terminal/mod.rs src/app/mod.rs locales/en.yml locales/zh-CN.yml
git diff -- Cargo.toml Cargo.lock README.md README.en.md .github/workflows/release.yml .github/release-notes/v0.1.0-beta.3.zh-CN.md docs/superpowers/specs/2026-08-01-ssh-accept-new-host-key-design.md docs/superpowers/plans/2026-08-01-ssh-accept-new-host-key.md
```

Confirm every change maps to this plan and no unrelated refactor is present.

---

### Task 8: Commit, Push, Tag, and Verify the Release

**Files:**
- Commit all verified changes.

- [ ] **Step 1: Create the feature commit after verification**

Stage the implementation, tests, locales, design, and plan:

```powershell
git add src/session/host_keys.rs src/backend/ssh.rs src/sftp/connection.rs src/sftp/mod.rs src/terminal/mod.rs src/app/mod.rs locales/en.yml locales/zh-CN.yml docs/superpowers/specs/2026-08-01-ssh-accept-new-host-key-design.md docs/superpowers/plans/2026-08-01-ssh-accept-new-host-key.md
git commit -m "fix(ssh): accept new host keys safely"
```

- [ ] **Step 2: Create the release-preparation commit**

```powershell
git add Cargo.toml Cargo.lock README.md README.en.md .github/workflows/release.yml .github/release-notes/v0.1.0-beta.3.zh-CN.md
git commit -m "chore(release): prepare v0.1.0-beta.3"
```

- [ ] **Step 3: Push `main` and wait for branch CI**

```powershell
git push origin main
$headSha = git rev-parse HEAD
$branchRunId = $null
for ($attempt = 0; $attempt -lt 30 -and -not $branchRunId; $attempt++) {
    $runs = gh run list --workflow CI --branch main --limit 20 --json databaseId,headSha | ConvertFrom-Json
    $branchRunId = ($runs | Where-Object { $_.headSha -eq $headSha } | Select-Object -First 1).databaseId
    if (-not $branchRunId) { Start-Sleep -Seconds 2 }
}
if (-not $branchRunId) { throw "CI run for $headSha was not found" }
gh run watch $branchRunId --exit-status
```

Select the newest CI run for the pushed commit. Do not create the tag until it exits successfully.

- [ ] **Step 4: Create and push the annotated release tag**

```powershell
git tag -a v0.1.0-beta.3 -m "JShell v0.1.0-beta.3"
git push origin v0.1.0-beta.3
```

- [ ] **Step 5: Wait for the Release workflow**

```powershell
$releaseRunId = $null
for ($attempt = 0; $attempt -lt 30 -and -not $releaseRunId; $attempt++) {
    $runs = gh run list --workflow Release --limit 20 --json databaseId,headBranch | ConvertFrom-Json
    $releaseRunId = ($runs | Where-Object { $_.headBranch -eq 'v0.1.0-beta.3' } | Select-Object -First 1).databaseId
    if (-not $releaseRunId) { Start-Sleep -Seconds 2 }
}
if (-not $releaseRunId) { throw "Release run for v0.1.0-beta.3 was not found" }
gh run watch $releaseRunId --exit-status
```

Select the run for tag `v0.1.0-beta.3` and wait for completion.

- [ ] **Step 6: Verify the published release body and assets**

```powershell
gh release view v0.1.0-beta.3 --json name,isPrerelease,body,assets,url
```

Confirm:

- Name is `JShell v0.1.0-beta.3`.
- `isPrerelease` is `true`.
- Body is the checked-in Simplified Chinese notes and includes the security rules and troubleshooting commands.
- Assets include Windows x86_64 ZIP, Linux x86_64 tar.gz, macOS aarch64 ZIP, and macOS x86_64 ZIP.

- [ ] **Step 7: Report the exact commits, tag, workflow results, and Release URL**

Include any hosted warning or failure honestly. Do not move or overwrite an existing published tag.
