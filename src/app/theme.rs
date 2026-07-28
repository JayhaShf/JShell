use anyhow::{Context as _, Result};
use gpui::{App, Context, Font, SharedString, TextRun, Window, black, px};
use gpui_component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry};

use crate::{
    Ashell, app::constants::terminal_cell_width_from_measurement,
    session::config::SYSTEM_MONOSPACE_FONT,
};

pub(crate) const JSHELL_LIGHT_THEME: &str = "JShell Light";
pub(crate) const JSHELL_DARK_THEME: &str = "JShell Dark";
pub(crate) const VSCODE_DARK_THEME: &str = "VS Code Dark";
pub(crate) const BUNDLED_TERMINAL_FONT: &str = "Noto Sans Mono CJK SC";

#[cfg(target_os = "windows")]
const SYSTEM_MONOSPACE_CANDIDATES: &[&str] = &["Cascadia Mono", "Consolas", BUNDLED_TERMINAL_FONT];
#[cfg(target_os = "macos")]
const SYSTEM_MONOSPACE_CANDIDATES: &[&str] = &["SF Mono", "Menlo", "Monaco", BUNDLED_TERMINAL_FONT];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const SYSTEM_MONOSPACE_CANDIDATES: &[&str] = &[
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    BUNDLED_TERMINAL_FONT,
];

pub(crate) const EMBEDDED_THEME_JSONS: &[&str] = &[
    include_str!("../../assets/themes/ashell.json"),
    include_str!("../../assets/themes/vscode.json"),
];

pub(crate) fn allowed_theme_names() -> [&'static str; 3] {
    [JSHELL_LIGHT_THEME, JSHELL_DARK_THEME, VSCODE_DARK_THEME]
}

pub(crate) fn validated_theme_name(name: &str, is_dark: bool) -> &'static str {
    match name {
        JSHELL_LIGHT_THEME | "Ashell Light" => JSHELL_LIGHT_THEME,
        JSHELL_DARK_THEME | "Ashell Dark" => JSHELL_DARK_THEME,
        VSCODE_DARK_THEME => VSCODE_DARK_THEME,
        _ if is_dark => JSHELL_DARK_THEME,
        _ => JSHELL_LIGHT_THEME,
    }
}

fn pick_installed_font(installed: &[String], preferred: &[&str], fallback: &str) -> String {
    preferred
        .iter()
        .find_map(|preferred_name| {
            installed
                .iter()
                .find(|installed_name| installed_name.eq_ignore_ascii_case(preferred_name))
        })
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn resolve_terminal_font_family(configured: &str, installed: &[String]) -> String {
    if !configured.is_empty() && configured != SYSTEM_MONOSPACE_FONT {
        return configured.to_string();
    }

    pick_installed_font(
        installed,
        SYSTEM_MONOSPACE_CANDIDATES,
        BUNDLED_TERMINAL_FONT,
    )
}

pub(crate) fn measure_terminal_cell_width(
    font_family: SharedString,
    font_size: f32,
    window: &mut Window,
) -> f32 {
    const SAMPLE: &str = "00000000";
    let shaped = window.text_system().shape_line(
        SAMPLE.into(),
        px(font_size),
        &[TextRun {
            len: SAMPLE.len(),
            font: Font {
                family: font_family,
                ..Font::default()
            },
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let measured = shaped.width().as_f32() / SAMPLE.len() as f32;
    terminal_cell_width_from_measurement(measured, font_size)
}

pub(crate) fn load_fonts(cx: &mut App) -> Result<()> {
    let ui_font = std::borrow::Cow::Borrowed(
        include_bytes!("../../assets/fonts/NotoSansCJKsc-Regular.otf").as_slice(),
    );
    let terminal_font = std::borrow::Cow::Borrowed(
        include_bytes!("../../assets/fonts/NotoSansMonoCJKsc-Regular.otf").as_slice(),
    );
    cx.text_system()
        .add_fonts(vec![ui_font, terminal_font])
        .context("load bundled Noto Sans CJK SC fonts")?;
    set_theme_font_names(
        cx.global_mut::<Theme>(),
        "Noto Sans CJK SC",
        "Noto Sans Mono CJK SC",
    );
    Ok(())
}

pub(crate) fn load_embedded_themes(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for theme_json in EMBEDDED_THEME_JSONS {
        if let Err(err) = registry.load_themes_from_str(theme_json) {
            tracing::warn!("failed to load embedded theme: {err:#}");
        }
    }
}

pub(crate) fn set_theme_font_names(
    theme: &mut Theme,
    ui_font_family: &str,
    terminal_font_family: &str,
) {
    theme.font_family = ui_font_family.into();
    theme.mono_font_family = terminal_font_family.into();
}

impl Ashell {
    pub(crate) fn switch_theme_mode(
        &mut self,
        mode: ThemeMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_system_theme = false;
        self.theme_mode = mode;
        self.apply_theme_preferences(window, cx);
        self.status = format!("theme mode: {}", cx.theme().mode.name()).into();
        self.persist_theme_preferences();
        cx.notify();
    }

    pub(crate) fn apply_theme(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_allowed = allowed_theme_names().iter().any(|allowed| name == *allowed);
        if !is_allowed {
            self.status = format!("theme not allowed: {name}").into();
            cx.notify();
            return;
        }

        let Some(theme_config) = ThemeRegistry::global(cx).themes().get(&name).cloned() else {
            self.status = format!("theme not found: {name}").into();
            cx.notify();
            return;
        };

        if theme_config.mode.is_dark() {
            self.dark_theme_name = name.clone();
        } else {
            self.light_theme_name = name.clone();
        }
        self.apply_theme_preferences(window, cx);
        self.status = format!("theme: {name}").into();
        self.persist_theme_preferences();
        window.refresh();
        cx.notify();
    }

    pub(crate) fn set_follow_system_theme(
        &mut self,
        follow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_system_theme = follow;
        if follow {
            self.status = "theme mode: system".into();
        } else {
            self.status = format!("theme mode: {}", cx.theme().mode.name()).into();
        }
        self.apply_theme_preferences(window, cx);
        self.persist_theme_preferences();
        cx.notify();
    }

    pub(crate) fn set_display_language(
        &mut self,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set_locale(locale);
        let mut active_locale = locale.to_string();
        if active_locale == "system" {
            active_locale = sys_locale::get_locale().unwrap_or_else(|| "en".to_string());
            if active_locale.starts_with("zh") {
                active_locale = "zh-CN".to_string();
            } else {
                active_locale = "en".to_string();
            }
        }
        rust_i18n::set_locale(&active_locale);
        gpui_component::set_locale(&active_locale);
        self.save_preferences_background();
        window.refresh();
        cx.notify();
    }

    pub(crate) fn apply_theme_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.light_theme_name = validated_theme_name(&self.light_theme_name, false).into();
        self.dark_theme_name = validated_theme_name(&self.dark_theme_name, true).into();
        let light_theme = ThemeRegistry::global(cx)
            .themes()
            .get(&self.light_theme_name)
            .cloned()
            .expect("JShell Light theme must be registered");
        let dark_theme = ThemeRegistry::global(cx)
            .themes()
            .get(&self.dark_theme_name)
            .cloned()
            .expect("JShell Dark theme must be registered");
        let theme = Theme::global_mut(cx);
        theme.light_theme = light_theme;
        theme.dark_theme = dark_theme;
        theme.font_size = px(self.ui_font_size);
        set_theme_font_names(theme, &self.ui_font_family, &self.terminal_font_family);

        if self.follow_system_theme {
            Theme::sync_system_appearance(Some(window), cx);
        } else {
            Theme::change(self.theme_mode, Some(window), cx);
        }
    }

    pub(crate) fn persist_theme_preferences(&mut self) {
        let theme_mode_str = match self.theme_mode {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        self.config.set_theme_preferences(
            self.follow_system_theme,
            theme_mode_str,
            self.light_theme_name.to_string(),
            self.dark_theme_name.to_string(),
        );
        self.save_preferences_background();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_theme_names_are_the_only_selectable_themes() {
        assert_eq!(
            allowed_theme_names(),
            ["JShell Light", "JShell Dark", "VS Code Dark"]
        );
    }

    #[test]
    fn legacy_and_removed_theme_names_fall_back_to_the_matching_jshell_default() {
        assert_eq!(validated_theme_name("Ashell Dark", true), "JShell Dark");
        assert_eq!(validated_theme_name("Ashell Light", false), "JShell Light");
        assert_eq!(validated_theme_name("Tokyo Night", true), "JShell Dark");
        assert_eq!(validated_theme_name("Solarized", false), "JShell Light");
    }

    #[test]
    fn installed_monospace_font_selection_uses_preference_order_case_insensitively() {
        let installed = vec![
            "Consolas".to_string(),
            "CASCADIA MONO".to_string(),
            "Noto Sans Mono CJK SC".to_string(),
        ];

        assert_eq!(
            pick_installed_font(
                &installed,
                &["Cascadia Mono", "Consolas"],
                "Noto Sans Mono CJK SC",
            ),
            "CASCADIA MONO"
        );
    }

    #[test]
    fn explicit_terminal_font_bypasses_system_font_resolution() {
        let installed = vec!["Cascadia Mono".to_string(), "Consolas".to_string()];

        assert_eq!(
            resolve_terminal_font_family("JetBrains Mono", &installed),
            "JetBrains Mono"
        );
    }

    #[test]
    fn bundled_themes_match_the_workspace_preview_palette() {
        let ashell_theme: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/themes/ashell.json")).unwrap();
        let vscode_theme: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/themes/vscode.json")).unwrap();

        let light = &ashell_theme["themes"][0]["colors"];
        assert_eq!(light["panel.background"], "#f5f5f5");
        assert_eq!(light["muted.background"], "#ededed");
        assert_eq!(light["secondary.hover.background"], "#e6e6e6");
        assert_eq!(light["sidebar.background"], "#f5f5f5");
        assert_eq!(light["tab.active.background"], "#151515");

        let dark = &ashell_theme["themes"][1]["colors"];
        assert_eq!(dark["base.green"], "#a7d797");
        assert_eq!(dark["base.yellow"], "#d8ca77");
        assert_eq!(dark["base.red"], "#d75050");
        assert_eq!(dark["sidebar.background"], "#171717");
        assert_eq!(dark["tab.active.background"], "#f5f5f5");

        let vscode = &vscode_theme["themes"][0]["colors"];
        assert_eq!(vscode["muted.foreground"], "#a5a5a5");
        assert_eq!(vscode["sidebar.background"], "#252526");
        assert_eq!(vscode["tab.active.background"], "#094771");
    }
}
