[中文](README.md) | [English](README.en.md)

# JShell

JShell is a Rust and GPUI desktop terminal workspace for local terminals, SSH sessions, SFTP file management, and remote text editing. The current source version is `1.0.0`; the UI follows the system language by default and can be changed in settings, while Chinese remains the default documentation entry.

![JShell workspace](assets/screenshots/compact-workspace.png)

## Highlights

- Local terminal, SSH, and serial sessions with saved-session folders and connection reuse.
- Horizontal and vertical splits that can mix terminals and remote text editors.
- Direct, global-proxy, and per-session proxy modes with SOCKS5, SOCKS5H, HTTP, and HTTPS support.
- SFTP browsing, Linux permission display, upload, download, directory creation, and deletion.
- Single-click row focus, checkbox-only selection, and double-click navigation or file opening.
- Automatic SFTP connection supervision without replaying destructive or transfer operations.
- Persistent soft wrap, explicit save states, conflict detection, and conservative disconnect handling.
- Detachable editor windows that reuse the same document, undo history, and save state.
- Tree-sitter highlighting for common shell, systems, web, data, and configuration languages.
- Dynamic Windows taskbar titles such as `JShell - nginx.conf*`.

## Save and reconnect behavior

Disconnecting never auto-saves an edited document. Local text remains in memory, and the user saves manually after reconnecting. If a write may have reached the server but its result cannot be confirmed, JShell keeps the document dirty and reports an unknown save outcome instead of retrying blindly.

## Downloads

Prebuilt archives are published on [GitHub Releases](https://github.com/JayhaShf/JShell/releases). Archive names follow these patterns:

- `jshell-<version>-windows-x86_64.zip`
- `jshell-<version>-linux-x86_64.tar.gz`
- `jshell-<version>-macos-aarch64.zip`
- `jshell-<version>-macos-x86_64.zip`

The prebuilt Linux archive targets Ubuntu 22.04 compatibility and requires glibc 2.35 or newer together with the GTK 3, Ayatana AppIndicator 3, XCB, xkbcommon/xkbcommon-x11, Wayland, and OpenGL runtime libraries. Package names vary by distribution; build from source on systems below that glibc baseline.

Every release archive includes the project GPL-3.0-or-later license, the OFL-1.1 license for the embedded Noto CJK fonts, and the font source notice.

The macOS bundle is ad-hoc signed. After verifying the download source, remove quarantine if macOS blocks the first launch:

```bash
xattr -cr /Applications/JShell.app
```

## Configuration

The default configuration file is `~/.config/jshell/sessions.json`. JShell migrates the legacy `~/.config/ashell` location and old theme names. The `.ashell-*` remote temporary-file protocol remains intentionally compatible with previous versions.

## Development

Rust `1.92.0` or newer is required.

```bash
cargo run --locked
cargo build --locked --release
```

Use `./scripts/package-macos-app.sh` to create a local macOS application bundle.

## Verification

```bash
cargo fmt --check
cargo test --locked --quiet
cargo check --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --release
cargo audit --deny warnings --file Cargo.lock
git diff --check
```

You can run the same local gate with `./scripts/verify.sh`. It requires `cargo-audit` and fails when that tool is unavailable.

## License

This project is licensed under [GPL-3.0-or-later](LICENSE).

## Acknowledgements

JShell evolves from [rust-kotlin/ashell](https://github.com/rust-kotlin/ashell), created by [TomZz](https://github.com/TomZz). Thank you to TomZz, the upstream maintainers, and every upstream contributor.
