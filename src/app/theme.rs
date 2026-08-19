use anyhow::{Context as _, Result};
use gpui::{App, Context, Font, Hsla, Rgba, SharedString, TextRun, Window, black, px};
use gpui_component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry};

use crate::{
    Ashell,
    app::constants::terminal_cell_width_from_measurement,
    session::config::{ConfigStore, SYSTEM_MONOSPACE_FONT},
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

fn resolved_display_locale(configured: &str, system_locale: Option<&str>) -> String {
    match configured {
        "zh-CN" => "zh-CN".to_string(),
        "en" => "en".to_string(),
        _ => {
            if system_locale.is_some_and(|locale| locale.starts_with("zh")) {
                "zh-CN".to_string()
            } else {
                "en".to_string()
            }
        }
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

pub(crate) fn configured_theme_mode(mode: &str) -> ThemeMode {
    match mode {
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::Light,
    }
}

/// Install the application theme definitions before a window is created.
///
/// GPUI Component starts with its built-in light theme. Installing the
/// persisted definitions first prevents a newly created window from briefly
/// using that default palette while `Ashell` is being constructed.
fn install_theme_preferences(
    light_theme_name: &str,
    dark_theme_name: &str,
    ui_font_size: f32,
    ui_font_family: &str,
    terminal_font_family: &str,
    cx: &mut App,
) -> (SharedString, SharedString) {
    let light_name: SharedString = validated_theme_name(light_theme_name, false).into();
    let dark_name: SharedString = validated_theme_name(dark_theme_name, true).into();
    let (light_theme, dark_theme) = {
        let themes = ThemeRegistry::global(cx).themes();
        let light_theme = themes
            .get(&light_name)
            .cloned()
            .expect("JShell Light theme must be registered");
        let dark_theme = themes
            .get(&dark_name)
            .cloned()
            .expect("JShell Dark theme must be registered");
        (light_theme, dark_theme)
    };

    let theme = Theme::global_mut(cx);
    theme.light_theme = light_theme;
    theme.dark_theme = dark_theme;
    theme.font_size = px(ui_font_size);
    set_theme_font_names(theme, ui_font_family, terminal_font_family);

    (light_name, dark_name)
}

/// Prepare the persisted theme before `App::open_window` exposes a native
/// window. System-following mode is intentionally resolved later from the
/// window's concrete appearance, which is more reliable on Linux than the
/// application-wide appearance during platform startup.
pub(crate) fn prepare_startup_theme(config: &ConfigStore, cx: &mut App) {
    let installed_fonts = cx.text_system().all_font_names();
    let terminal_font_family =
        resolve_terminal_font_family(config.terminal_font_family(), &installed_fonts);
    install_theme_preferences(
        config.light_theme_name(),
        config.dark_theme_name(),
        config.ui_font_size(),
        config.ui_font_family(),
        &terminal_font_family,
        cx,
    );

    if !config.follow_system_theme() {
        Theme::change(configured_theme_mode(config.theme_mode()), None, cx);
    }
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

const WORKSPACE_TAB_FILE_BLUE: u32 = 0x2f7faa;

pub(crate) fn workspace_tab_palette(theme: &Theme) -> [Hsla; 5] {
    // The JShell themes use a neutral base.blue, so keep the file accent local to tabs.
    let file_blue = if theme.blue.s <= 0.05 {
        gpui::rgb(WORKSPACE_TAB_FILE_BLUE).into()
    } else {
        theme.blue
    };
    [
        theme.success,
        file_blue,
        theme.warning,
        theme.danger,
        theme.muted_foreground,
    ]
}

fn color_contrast_ratio(foreground: Hsla, background: Hsla) -> f32 {
    fn relative_luminance(color: Hsla) -> f32 {
        fn linearize(component: f32) -> f32 {
            if component <= 0.03928 {
                component / 12.92
            } else {
                ((component + 0.055) / 1.055).powf(2.4)
            }
        }

        let rgba: Rgba = color.into();
        0.2126 * linearize(rgba.r) + 0.7152 * linearize(rgba.g) + 0.0722 * linearize(rgba.b)
    }

    let foreground = relative_luminance(foreground);
    let background = relative_luminance(background);
    let (lighter, darker) = if foreground > background {
        (foreground, background)
    } else {
        (background, foreground)
    };
    (lighter + 0.05) / (darker + 0.05)
}

pub(crate) fn workspace_tab_accent(color: Hsla, background: Hsla) -> Hsla {
    const MINIMUM_CONTRAST: f32 = 3.0;

    if color_contrast_ratio(color, background) >= MINIMUM_CONTRAST {
        return color;
    }

    let target_lightness = if background.l >= 0.5 { 0.0 } else { 1.0 };
    let mut adjusted = color;
    for step in 1..=20 {
        let amount = step as f32 / 20.0;
        adjusted.l = color.l + (target_lightness - color.l) * amount;
        if color_contrast_ratio(adjusted, background) >= MINIMUM_CONTRAST {
            return adjusted;
        }
    }

    adjusted
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
        self.apply_runtime_display_language(locale, window, cx);
        self.save_preferences_background();
    }

    pub(crate) fn apply_runtime_display_language(
        &mut self,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let system_locale = sys_locale::get_locale();
        let active_locale = resolved_display_locale(locale, system_locale.as_deref());
        rust_i18n::set_locale(&active_locale);
        gpui_component::set_locale(&active_locale);
        window.refresh();
        cx.notify();
    }

    pub(crate) fn apply_theme_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (light_theme_name, dark_theme_name) = install_theme_preferences(
            &self.light_theme_name,
            &self.dark_theme_name,
            self.ui_font_size,
            &self.ui_font_family,
            &self.terminal_font_family,
            cx,
        );
        self.light_theme_name = light_theme_name;
        self.dark_theme_name = dark_theme_name;

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
    fn configured_theme_mode_defaults_invalid_values_to_light() {
        assert_eq!(configured_theme_mode("light"), ThemeMode::Light);
        assert_eq!(configured_theme_mode("dark"), ThemeMode::Dark);
        assert_eq!(configured_theme_mode(""), ThemeMode::Light);
        assert_eq!(configured_theme_mode("unexpected"), ThemeMode::Light);
    }

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
    fn display_locale_resolves_system_language_and_preserves_explicit_choices() {
        assert_eq!(
            resolved_display_locale("system", Some("zh-Hans-CN")),
            "zh-CN"
        );
        assert_eq!(resolved_display_locale("system", Some("en-US")), "en");
        assert_eq!(resolved_display_locale("system", None), "en");
        assert_eq!(resolved_display_locale("zh-CN", Some("en-US")), "zh-CN");
        assert_eq!(resolved_display_locale("en", Some("zh-CN")), "en");
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
        assert_eq!(light["base.blue"], "#202020");

        let dark = &ashell_theme["themes"][1]["colors"];
        assert_eq!(dark["base.green"], "#a7d797");
        assert_eq!(dark["base.yellow"], "#d8ca77");
        assert_eq!(dark["base.red"], "#d75050");
        assert_eq!(dark["sidebar.background"], "#171717");
        assert_eq!(dark["tab.active.background"], "#f5f5f5");
        assert_eq!(dark["base.blue"], "#f5f5f5");

        let vscode = &vscode_theme["themes"][0]["colors"];
        assert_eq!(vscode["muted.foreground"], "#a5a5a5");
        assert_eq!(vscode["sidebar.background"], "#252526");
        assert_eq!(vscode["tab.active.background"], "#094771");
        assert_eq!(vscode["base.blue"], "#569cd6");
    }

    #[test]
    fn workspace_tab_palette_reads_each_semantic_theme_color() {
        let success = gpui::rgb(0x117711).into();
        let blue = gpui::rgb(0x2255cc).into();
        let warning = gpui::rgb(0xddaa22).into();
        let danger = gpui::rgb(0xdd2233).into();
        let muted = gpui::rgb(0x667788).into();
        let theme = Theme::from(&gpui_component::ThemeColor {
            success,
            blue,
            warning,
            danger,
            muted_foreground: muted,
            ..Default::default()
        });

        assert_eq!(
            workspace_tab_palette(&theme),
            [success, blue, warning, danger, muted]
        );
    }

    #[test]
    fn workspace_tab_palette_uses_a_dedicated_file_blue() {
        let theme = Theme::from(&gpui_component::ThemeColor {
            blue: gpui::rgb(0x202020).into(),
            ..Default::default()
        });

        assert_eq!(
            workspace_tab_palette(&theme)[1],
            gpui::rgb(WORKSPACE_TAB_FILE_BLUE).into()
        );
    }

    #[test]
    fn workspace_tab_selection_accent_reaches_three_to_one_contrast_on_ssh_black() {
        let accent = workspace_tab_accent(gpui::rgb(0x151515).into(), gpui::black());

        assert!(color_contrast_ratio(accent, gpui::black()) >= 3.0);
    }

    #[test]
    fn bundled_workspace_tab_accents_cover_idle_active_hover_and_ssh_backgrounds() {
        fn color(colors: &serde_json::Value, key: &str) -> Hsla {
            let hex = colors[key].as_str().unwrap().strip_prefix('#').unwrap();
            gpui::rgb(u32::from_str_radix(hex, 16).unwrap()).into()
        }

        let ashell: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/themes/ashell.json")).unwrap();
        let vscode: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/themes/vscode.json")).unwrap();

        for theme in ashell["themes"]
            .as_array()
            .unwrap()
            .iter()
            .chain(vscode["themes"].as_array().unwrap())
        {
            let name = theme["name"].as_str().unwrap();
            let colors = &theme["colors"];
            let palette_theme = Theme::from(&gpui_component::ThemeColor {
                success: color(colors, "base.green"),
                blue: color(colors, "base.blue"),
                warning: color(colors, "base.yellow"),
                danger: color(colors, "danger.background"),
                muted_foreground: color(colors, "muted.foreground"),
                ..Default::default()
            });

            let backgrounds = [
                color(colors, "tab.background"),
                color(colors, "tab.active.background"),
                color(colors, "secondary.hover.background"),
                gpui::black(),
            ];
            for background in backgrounds {
                for accent in workspace_tab_palette(&palette_theme) {
                    let adjusted = workspace_tab_accent(accent, background);
                    assert!(
                        color_contrast_ratio(adjusted, background) >= 3.0,
                        "{name} workspace tab accent did not meet 3:1 contrast"
                    );
                }
            }
        }
    }
}
