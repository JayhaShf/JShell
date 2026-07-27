use gpui::{App, Context, SharedString, Window, px};
use gpui_component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry};

use crate::Ashell;

pub(crate) const ASHELL_LIGHT_THEME: &str = "Ashell Light";
pub(crate) const ASHELL_DARK_THEME: &str = "Ashell Dark";
pub(crate) const VSCODE_DARK_THEME: &str = "VS Code Dark";

pub(crate) const EMBEDDED_THEME_JSONS: &[&str] = &[
    include_str!("../../assets/themes/ashell.json"),
    include_str!("../../assets/themes/vscode.json"),
];

pub(crate) fn allowed_theme_names() -> [&'static str; 3] {
    [ASHELL_LIGHT_THEME, ASHELL_DARK_THEME, VSCODE_DARK_THEME]
}

pub(crate) fn validated_theme_name(name: &str, is_dark: bool) -> &'static str {
    match name {
        ASHELL_LIGHT_THEME => ASHELL_LIGHT_THEME,
        ASHELL_DARK_THEME => ASHELL_DARK_THEME,
        VSCODE_DARK_THEME => VSCODE_DARK_THEME,
        _ if is_dark => ASHELL_DARK_THEME,
        _ => ASHELL_LIGHT_THEME,
    }
}

pub(crate) fn load_fonts(cx: &mut App) -> anyhow::Result<()> {
    set_theme_font_names(cx.global_mut::<Theme>(), "Noto Sans CJK SC");
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

pub(crate) fn set_theme_font_names(theme: &mut Theme, ui_font_family: &str) {
    theme.font_family = ui_font_family.into();
    theme.mono_font_family = ui_font_family.into();
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
            .expect("Ashell Light theme must be registered");
        let dark_theme = ThemeRegistry::global(cx)
            .themes()
            .get(&self.dark_theme_name)
            .cloned()
            .expect("Ashell Dark theme must be registered");
        let theme = Theme::global_mut(cx);
        theme.light_theme = light_theme;
        theme.dark_theme = dark_theme;
        theme.font_size = px(self.ui_font_size);
        set_theme_font_names(theme, &self.ui_font_family);

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
            ["Ashell Light", "Ashell Dark", "VS Code Dark"]
        );
    }

    #[test]
    fn removed_theme_name_falls_back_to_the_matching_ashell_default() {
        assert_eq!(validated_theme_name("Tokyo Night", true), "Ashell Dark");
        assert_eq!(validated_theme_name("Solarized", false), "Ashell Light");
    }
}
