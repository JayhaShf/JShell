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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    /// Loads a rust-i18n flat `key: "value"` locale file into a map.
    fn load_locale(name: &str) -> BTreeMap<String, String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("locales")
            .join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        serde_yaml::from_str::<BTreeMap<String, String>>(&content)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()))
    }

    #[test]
    fn locale_keys_match_between_english_and_chinese() {
        let en = load_locale("en.yml");
        let zh = load_locale("zh-CN.yml");

        let missing_in_zh: Vec<&String> = en.keys().filter(|k| !zh.contains_key(*k)).collect();
        let missing_in_en: Vec<&String> = zh.keys().filter(|k| !en.contains_key(*k)).collect();

        assert!(
            missing_in_zh.is_empty(),
            "keys present in en.yml but missing from zh-CN.yml: {missing_in_zh:?}",
        );
        assert!(
            missing_in_en.is_empty(),
            "keys present in zh-CN.yml but missing from en.yml: {missing_in_en:?}",
        );
    }

    #[test]
    fn locale_values_are_non_empty_in_both_languages() {
        for (name, map) in [
            ("en.yml", load_locale("en.yml")),
            ("zh-CN.yml", load_locale("zh-CN.yml")),
        ] {
            let empty: Vec<&String> = map
                .iter()
                .filter(|(_, v)| v.is_empty())
                .map(|(k, _)| k)
                .collect();
            assert!(empty.is_empty(), "empty translations in {name}: {empty:?}");
        }
    }
}
