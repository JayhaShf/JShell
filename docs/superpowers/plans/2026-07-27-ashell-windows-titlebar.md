# Ashell Windows Title Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the macOS-like chrome, bundled fonts, and unrestricted theme catalog with the approved Windows-oriented design while preventing duplicate connected SSH tabs.

**Architecture:** The GPUI title bar stays integrated but is rendered as a single Fluent-style Windows row that consumes only active `Theme` colors. A small theme catalog module defines the two monochrome Ashell themes and VS Code Dark; dialog menus filter to those names. Saved-session activation uses a pure tab-lookup helper before delegating to the existing tab activation path.

**Tech Stack:** Rust 2024, GPUI, gpui-component, serde, Cargo tests.

## Global Constraints

- UI and terminal default font family: `Noto Sans CJK SC`.
- Remove all Maple Mono assets and runtime loading code.
- Expose only `Ashell Light`, `Ashell Dark`, and `VS Code Dark` in theme menus.
- Ashell Light and Ashell Dark use monochrome palettes; VS Code Dark keeps blue active accents and a white terminal prompt.
- The custom title bar uses active theme colors, Windows controls on the right, and radii no greater than 8 px.
- A connected saved SSH session is activated instead of duplicated; disconnected matches do not suppress a new connection.
- Run `cargo test` and `cargo build --release` before the final commit.

---

### Task 1: Define the restricted theme catalog and new font defaults

**Files:**
- Create: `assets/themes/ashell.json`
- Create: `assets/themes/vscode.json`
- Modify: `src/app/theme.rs`
- Modify: `src/session/config.rs`
- Modify: `src/app/dialogs.rs`
- Delete: `assets/fonts/MapleMono-NF-CN-Regular.ttf`, `assets/fonts/MapleMono-NF-CN-Bold.ttf`, `assets/fonts/LICENSE.txt`, `assets/themes/matrix.json`, `assets/themes/tokyonight.json`, `assets/themes/gruvbox.json`, `assets/themes/solarized.json`, `assets/themes/phygerr.json`

**Interfaces:**
- Produces `ASHELL_LIGHT_THEME`, `ASHELL_DARK_THEME`, `VSCODE_DARK_THEME`, `allowed_theme_names()`, and `validated_theme_name()` in `app::theme`.
- Consumes `ThemeRegistry` and existing `ConfigStore` theme preference fields.

- [ ] **Step 1: Write the failing tests for the allowed theme names and new font defaults.**

```rust
#[test]
fn default_fonts_use_noto_sans_cjk_sc() {
    let config = ConfigFile::default();
    assert_eq!(config.ui_font_family, "Noto Sans CJK SC");
    assert_eq!(config.terminal_font_family, "Noto Sans CJK SC");
}

#[test]
fn allowed_theme_names_are_the_only_selectable_themes() {
    assert_eq!(crate::app::theme::allowed_theme_names(), [
        "Ashell Light",
        "Ashell Dark",
        "VS Code Dark",
    ]);
}

#[test]
fn removed_theme_name_falls_back_to_the_matching_ashell_default() {
    assert_eq!(crate::app::theme::validated_theme_name("Tokyo Night", true), "Ashell Dark");
    assert_eq!(crate::app::theme::validated_theme_name("Solarized", false), "Ashell Light");
}
```

- [ ] **Step 2: Run the focused tests and verify the expected failure.**

Run: `cargo test default_fonts_use_noto_sans_cjk_sc; cargo test allowed_theme_names_are_the_only_selectable_themes; cargo test removed_theme_name_falls_back_to_the_matching_ashell_default`

Expected: compile failure because the catalog helper and Noto defaults do not yet exist.

- [ ] **Step 3: Add the two JSON theme files and minimal catalog implementation.**

```rust
pub(crate) const ASHELL_LIGHT_THEME: &str = "Ashell Light";
pub(crate) const ASHELL_DARK_THEME: &str = "Ashell Dark";
pub(crate) const VSCODE_DARK_THEME: &str = "VS Code Dark";

pub(crate) fn allowed_theme_names() -> [&'static str; 3] {
    [ASHELL_LIGHT_THEME, ASHELL_DARK_THEME, VSCODE_DARK_THEME]
}

pub(crate) fn validated_theme_name(name: &str, is_dark: bool) -> &'static str {
    if allowed_theme_names().contains(&name) {
        match name {
            ASHELL_LIGHT_THEME => ASHELL_LIGHT_THEME,
            ASHELL_DARK_THEME => ASHELL_DARK_THEME,
            VSCODE_DARK_THEME => VSCODE_DARK_THEME,
            _ => unreachable!(),
        }
    } else if is_dark {
        ASHELL_DARK_THEME
    } else {
        ASHELL_LIGHT_THEME
    }
}
```

Load only `ashell.json` and `vscode.json`, remove Maple font registration, set both configuration defaults to Noto Sans CJK SC, validate persisted theme names before reading the registry, and filter light/dark menus by `allowed_theme_names()` and each registry theme mode.

- [ ] **Step 4: Run focused tests and verify they pass.**

Run: `cargo test default_fonts_use_noto_sans_cjk_sc; cargo test allowed_theme_names_are_the_only_selectable_themes; cargo test removed_theme_name_falls_back_to_the_matching_ashell_default`

Expected: both tests pass.

### Task 2: Replace the title bar with the approved Windows layout

**Files:**
- Modify: `src/app/startup.rs`
- Modify: `src/app/mod.rs`
- Modify: `src/app/ui.rs`
- Modify: `src/app/dialogs.rs`
- Modify: `src/session/config.rs`
- Modify: `locales/en.yml`
- Modify: `locales/zh-CN.yml`

**Interfaces:**
- Consumes `ActiveTheme`, `WindowControlArea`, `Window`, `Ashell::render_tab_bar`, and theme catalog colors.
- Produces an always-integrated custom title bar with no persisted title-bar style preference.

- [ ] **Step 1: Write a failing configuration test proving title-bar style is no longer persisted.**

```rust
#[test]
fn merged_preferences_do_not_restore_a_title_bar_style() {
    let mut preferences = ConfigFile::default();
    preferences.ui_font_size = 16.0;
    assert_eq!(preferences.ui_font_size, 16.0);
    assert!(!serde_json::to_string(&preferences).unwrap().contains("title_bar_style"));
}
```

- [ ] **Step 2: Run the test and verify the expected failure.**

Run: `cargo test merged_preferences_do_not_restore_a_title_bar_style`

Expected: failure because `ConfigFile` currently serializes `title_bar_style`.

- [ ] **Step 3: Remove style configuration and render the new title bar.**

Always set `WindowOptions::titlebar` for the custom frame. Delete `TitleBarStyle`, its config field and accessors, its settings dropdown and locale keys, and the native-style conditional branches. Replace `render_window_controls` with a right-aligned `Min`, `Max`, and `Close` control group using `WindowControlArea`; use `cx.theme().tab_bar`, `border`, `foreground`, `muted_foreground`, and `primary` for all non-close presentation. Keep the active session rail and title-bar drag area in the single 44 px row.

- [ ] **Step 4: Run the focused test and compile check.**

Run: `cargo test merged_preferences_do_not_restore_a_title_bar_style && cargo check`

Expected: test passes and the crate compiles.

### Task 3: Reuse an existing connected SSH tab

**Files:**
- Modify: `src/session/mod.rs`

**Interfaces:**
- Produces `connected_session_tab_id(tabs, session_id) -> Option<String>`.
- Consumes `TerminalTab::id`, `TerminalTab::connected`, and `TerminalTab::session`.
- Updates `connect_saved_session(session_id, window, cx)` callers to provide `Window`.

- [ ] **Step 1: Write failing tests for connected and disconnected saved sessions.**

```rust
#[test]
fn connected_saved_session_returns_existing_tab_id() {
    let session = Session { id: "prod".into(), ..test_session() };
    let entries = vec![("first", false, Some(&session)), ("second", true, Some(&session))];
    assert_eq!(connected_session_tab_id(entries, "prod"), Some("second".into()));
}

#[test]
fn disconnected_saved_session_does_not_block_a_new_connection() {
    let session = Session { id: "prod".into(), ..test_session() };
    assert_eq!(connected_session_tab_id(vec![("old", false, Some(&session))], "prod"), None);
}
```

Add `test_session()` in the existing `session::mod` test module with all required `Session` fields initialized.

- [ ] **Step 2: Run the focused tests and verify the expected failure.**

Run: `cargo test connected_saved_session`

Expected: compile failure because `connected_session_tab_id` does not exist.

- [ ] **Step 3: Implement the lookup and activate-before-connect behavior.**

```rust
if let Some(tab_id) = connected_session_tab_id(
    self.tabs.iter().map(|tab| (tab.id.as_str(), tab.connected, tab.session.as_ref())),
    &session_id,
) {
    self.activate_tab(tab_id, window, cx);
    return;
}
```

Pass `window` from selector, saved-session list, and all other `connect_saved_session` call sites. Preserve serial-session creation and the existing SSH creation path when no connected tab matches.

- [ ] **Step 4: Run focused tests and the session test module.**

Run: `cargo test connected_saved_session && cargo test session::`

Expected: all focused tests pass.

### Task 4: Update user documentation and remove stale asset claims

**Files:**
- Modify: `README.md`
- Modify: `README.en.md`

**Interfaces:**
- Consumes the completed theme, font, title-bar, and saved-session behavior.
- Produces accurate Chinese and English feature descriptions.

- [ ] **Step 1: Update the documentation.**

Replace the embedded Maple Mono claim with Noto Sans CJK SC as the default system font. Describe the custom Windows-style title bar, the three-theme catalog, and opening an already connected saved SSH session by focusing it instead of duplicating it.

- [ ] **Step 2: Verify stale claims are absent.**

Run: `rg -n "Maple Mono|Tokyo Night|Gruvbox|Solarized|Matrix|Phygerr" README.md README.en.md src assets`

Expected: no matches, except intentional historical text removed in this task.

### Task 5: Full verification and final commit

**Files:**
- Modify: all files from Tasks 1-4 and the approved design/plan documents.

- [ ] **Step 1: Format and inspect the diff.**

Run: `cargo fmt --check && git diff --check && git status --short`

Expected: formatter and whitespace checks pass; changed files match the approved scope.

- [ ] **Step 2: Run the full test suite.**

Run: `cargo test`

Expected: all Rust tests pass.

- [ ] **Step 3: Build the release artifact before committing.**

Run: `cargo build --release`

Expected: release build exits successfully and creates `target/release/ashell.exe` on Windows.

- [ ] **Step 4: Commit the verified implementation.**

Run:

```powershell
git add README.md README.en.md Cargo.lock Cargo.toml assets src locales docs
git commit -m "feat: modernize Windows title bar and themes"
```

Expected: one commit contains the implementation, documentation, and design records.
