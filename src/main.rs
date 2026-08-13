#![windows_subsystem = "windows"]

use gpui::KeyBinding;
use gpui_component_assets::Assets;

mod app;
mod backend;
mod document;
mod session;
mod sftp;
mod sync;
mod system;
mod terminal;

rust_i18n::i18n!("locales", fallback = "en");

gpui::actions!(ashell_terminal, [TerminalTabKey, TerminalBacktabKey]);

pub(crate) use app::keybinding_recorder::{
    ClosePane, Copy, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp, NewSsh, OpenSearch,
    OpenSession, OpenSettings, OpenTransfers, Paste, SaveDocument, SplitPaneDown, SplitPaneLeft,
    SplitPaneRight, SplitPaneUp, ToggleSftpZoom, ToggleSidebar,
};

pub(crate) use app::{Ashell, PaneLayout, PaneLeaf, SelectorEntry, SftpContextMenuState, TabGroup};

fn main() {
    app::startup::sync_macos_launch_environment();
    app::startup::init_logging();

    let instance_events = match app::single_instance::acquire() {
        app::single_instance::AcquireOutcome::First(events) => Some(events),
        app::single_instance::AcquireOutcome::Second => {
            // Another instance is already running and has been asked to show
            // its window, so this process has nothing left to do.
            return;
        }
    };
    let signal_events = app::signals::install();

    #[cfg(target_os = "macos")]
    let app = gpui_platform::application()
        .with_assets(Assets)
        .with_quit_mode(gpui::QuitMode::LastWindowClosed);

    #[cfg(not(target_os = "macos"))]
    let app = gpui_platform::application().with_assets(Assets);

    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            app::startup::open_main_window(cx);
        } else if let Some(handle) = cx.windows().first().cloned() {
            // Restore the window when the dock icon is clicked while hidden in the tray.
            handle
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        }
    });
    app.run(move |cx| {
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new(
                "tab",
                TerminalTabKey,
                Some(app::constants::TERMINAL_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "shift-tab",
                TerminalBacktabKey,
                Some(app::constants::TERMINAL_KEY_CONTEXT),
            ),
        ]);
        let startup_config = app::startup::load_startup_config_and_bind_workspace_keys(cx);
        app::theme::load_embedded_themes(cx);
        if let Err(err) = app::theme::load_fonts(cx) {
            tracing::warn!("failed to load theme fonts: {err:#}");
        }
        app::startup::open_main_window_with_config(
            cx,
            startup_config,
            instance_events,
            signal_events,
        );
    });
}
