# SFTP Directory Follow Design

## Goal

Keep the SFTP directory aligned with the active SSH terminal without reducing SSH terminal responsiveness or connection reliability.

## User-visible behavior

The local preference `sftp_cwd_sync_mode` has three values:

- `off`: never follow automatically; the existing manual sync action remains available.
- `on-terminal-switch`: synchronize once when the active SSH terminal changes. This is the default for new and legacy configurations.
- `realtime`: synchronize when the active terminal reports a new working directory and when the active terminal changes.

The setting appears under a new `SSH / SFTP` group on the General settings page. It is stored as a device-local preference and is not added to the strict portable sync payload in this change.

## Working-directory detection

Each terminal caches its last reliably parsed remote working directory. Detection remains event-driven and accepts the existing title formats:

- `ASHELL_CWD:/absolute/path`
- a conventional terminal title ending in an absolute path after `:`, such as `user@host:/srv/app`
- `~` and `~/...`, resolved against the SFTP home directory

The application does not poll the terminal or inject shell commands. If no reliable path has been observed, automatic following does nothing and leaves the current SFTP directory unchanged.

## Connection priority

At least one associated SSH terminal connection is established first. The independent SFTP connection starts only after an SSH terminal in that group emits its connected event. This avoids competing initial SSH and SFTP handshakes on high-latency servers or servers with connection limits, while allowing an already connected split terminal to unblock the shared SFTP connection.

The same ordering applies when an SSH terminal is manually retried: any requested SFTP restart is held for that terminal until its new SSH generation emits `Connected`.

Directory following never sends commands through the terminal SSH channel. SFTP directory reads remain on the independent SFTP connection, so terminal input and SSH keepalive handling do not wait for them.

## Request coordination

Automatic directory reads carry a request identifier. Each SFTP group allows at most one automatic directory read in flight and retains at most one pending path, always replacing it with the newest detected path.

UI-originated directory reads are also bound to the SFTP generation that created them. A command left in a supervisor queue or channel cannot execute against a newer connection generation; the current automatic intent or latest manual path is reissued after that generation becomes ready.

Matching results by request identifier prevents stale A -> B -> A responses from being accepted as current. Manual SFTP navigation cancels pending automatic intent; an already running remote operation may finish, but its stale result cannot move the user away from the manually selected directory.

While SFTP is unavailable, manual navigation updates only the latest desired path. The new connection reads that path once after it becomes ready. Repeated title events for an unchanged terminal CWD do not override a newer manual navigation.

When SFTP is unavailable, the group retains only the newest detected path. Once SFTP reconnects, the latest applicable path is synchronized.

## High-latency degradation

An automatic directory read that has not completed within 3 seconds latches the current SFTP group out of realtime mode for the remainder of that session. Its effective mode becomes `on-terminal-switch`:

- further realtime CWD updates are coalesced or discarded rather than queued;
- terminal-switch synchronization remains available;
- the slow SFTP operation is not force-cancelled, because it runs on the independent SFTP connection;
- the UI reports that realtime following was paused because SFTP exceeded 3 seconds.

The latch never clears automatically. Re-selecting `realtime` clears it for the current session and attempts to follow the active terminal again.

## Errors

- An unparseable terminal title does not replace the last known valid CWD.
- A missing or inaccessible remote directory leaves the displayed SFTP directory unchanged and uses the existing SFTP status area for the error.
- Stale results, results from an old SFTP generation, and results for superseded request identifiers are ignored.
- The existing manual CWD sync and refresh actions remain available in every mode.

## Scope

This change includes the preference, settings control, per-terminal CWD cache, terminal-switch and realtime triggers, deferred SFTP startup, bounded automatic request coordination, degradation state, and localized UI text.

It does not inject remote shell integration, periodically execute `pwd`, change the portable configuration schema, audit unrelated SFTP behavior, merge the branch, or push it.

## Verification plan

Verification is deferred until implementation feedback is complete. The later targeted verification will cover path parsing, mode defaults and persistence, switch/realtime triggering, request coalescing and stale-result rejection, the 3-second latch, manual re-enable, and SSH-before-SFTP startup ordering. A Release build will then be produced before the changes are committed on the existing feature branch.
