# Ashell Windows Title Bar Design

## Scope

This change updates the default typography, title bar, theme catalog, and saved SSH session activation behavior for the Windows-oriented Ashell workspace.

## Typography

Both the UI and terminal defaults will use the system-installed `Noto Sans CJK SC` family. The application will no longer embed or load Maple Mono font files, and `assets/fonts` will be removed. Runtime font fallback remains the responsibility of the platform text system when Noto Sans CJK SC is unavailable.

## Title Bar

Ashell will keep a custom GPUI-integrated title bar, replacing the existing macOS-inspired presentation. The new one-row Fluent Mica layout has:

- An application identity area on the left.
- A compact session rail in the center, with a small state marker and a thin active indicator.
- Search and overflow actions before the window controls.
- Standard Windows window controls on the right: minimize, maximize or restore, and close.
- A maximum 8 px outer corner radius and 6 px radius for compact interactive surfaces.

The title bar is always custom and no longer exposes the native versus integrated title-bar setting. It reads `tab_bar`, `border`, `foreground`, `primary`, and related colors from the active GPUI theme instead of maintaining separate colors.

## Themes

The settings UI exposes exactly three selectable themes:

- `Ashell Light`: black text on white and neutral-gray surfaces.
- `Ashell Dark`: white text on black and neutral-gray surfaces.
- `VS Code Dark`: VS Code-inspired dark surfaces and blue active accents, with white terminal prompt text.

The existing Matrix, Tokyo Night, Gruvbox, Solarized, and Phygerr bundled theme definitions and selection entries will be removed. Stored references to a removed theme fall back to the corresponding Ashell Light or Ashell Dark default.

## SSH Session Activation

When a saved SSH session is opened, the application first searches terminal tabs for a connected SSH tab whose saved session ID matches. If found, it activates that tab and focuses its terminal pane. If none is connected, the normal connection path creates a new tab. Disconnected matching tabs do not prevent a new connection.

## Verification

Focused Rust tests will cover the saved-session tab lookup and theme fallback behavior. The full Rust test suite and `cargo build --release` will run before the final commit. `README.md` will describe the three-theme catalog, Noto Sans CJK SC default, and Windows-style custom title bar.
