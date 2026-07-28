use gpui::{
    Action as _, App, Entity, IntoElement, KeyBinding, KeyDownEvent, Keystroke, Unbind, prelude::*,
};
use gpui_component::{
    Sizable,
    button::{Button, ButtonVariants},
    kbd::Kbd,
    setting::{SettingField, SettingGroup, SettingItem},
};
use rust_i18n::t;

use crate::{Ashell, session::config::ConfigStore};

gpui::actions!(
    ashell_workspace,
    [
        OpenSettings,
        OpenSession,
        OpenTransfers,
        NewSsh,
        OpenSearch,
        ToggleSidebar,
        ToggleSftpZoom,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        SplitPaneLeft,
        SplitPaneRight,
        SplitPaneUp,
        SplitPaneDown,
        ClosePane,
        Copy,
        Paste,
        SaveDocument
    ]
);

pub struct KeybindingsPage;

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceAction {
    id: &'static str,
    label_key: &'static str,
    default_suffix: &'static str,
}

pub(crate) const WORKSPACE_ACTIONS: &[WorkspaceAction] = &[
    WorkspaceAction {
        id: "OpenSettings",
        label_key: "settings_open_settings",
        default_suffix: "shift-,",
    },
    WorkspaceAction {
        id: "OpenSession",
        label_key: "settings_open_session",
        default_suffix: "shift-o",
    },
    WorkspaceAction {
        id: "OpenTransfers",
        label_key: "settings_open_transfers",
        default_suffix: "shift-t",
    },
    WorkspaceAction {
        id: "NewSsh",
        label_key: "settings_new_ssh",
        default_suffix: "shift-n",
    },
    WorkspaceAction {
        id: "OpenSearch",
        label_key: "settings_open_search",
        default_suffix: "shift-f",
    },
    WorkspaceAction {
        id: "ToggleSidebar",
        label_key: "settings_toggle_sidebar",
        default_suffix: "shift-b",
    },
    WorkspaceAction {
        id: "ToggleSftpZoom",
        label_key: "settings_toggle_sftp_zoom",
        default_suffix: "shift-m",
    },
    WorkspaceAction {
        id: "FocusPaneLeft",
        label_key: "settings_focus_pane_left",
        default_suffix: "alt-left",
    },
    WorkspaceAction {
        id: "FocusPaneRight",
        label_key: "settings_focus_pane_right",
        default_suffix: "alt-right",
    },
    WorkspaceAction {
        id: "FocusPaneUp",
        label_key: "settings_focus_pane_up",
        default_suffix: "alt-up",
    },
    WorkspaceAction {
        id: "FocusPaneDown",
        label_key: "settings_focus_pane_down",
        default_suffix: "alt-down",
    },
    WorkspaceAction {
        id: "SplitPaneLeft",
        label_key: "settings_split_pane_left",
        default_suffix: "alt-shift-left",
    },
    WorkspaceAction {
        id: "SplitPaneRight",
        label_key: "settings_split_pane_right",
        default_suffix: "alt-shift-right",
    },
    WorkspaceAction {
        id: "SplitPaneUp",
        label_key: "settings_split_pane_up",
        default_suffix: "alt-shift-up",
    },
    WorkspaceAction {
        id: "SplitPaneDown",
        label_key: "settings_split_pane_down",
        default_suffix: "alt-shift-down",
    },
    WorkspaceAction {
        id: "ClosePane",
        label_key: "settings_close_pane",
        default_suffix: "shift-w",
    },
    WorkspaceAction {
        id: "Copy",
        label_key: "settings_copy",
        default_suffix: if cfg!(target_os = "macos") {
            "c"
        } else {
            "shift-c"
        },
    },
    WorkspaceAction {
        id: "Paste",
        label_key: "settings_paste",
        default_suffix: if cfg!(target_os = "macos") {
            "v"
        } else {
            "shift-v"
        },
    },
    WorkspaceAction {
        id: "SaveDocument",
        label_key: "settings_save_document",
        default_suffix: "s",
    },
];

pub(crate) fn action_key_context(action_id: &str) -> Option<&'static str> {
    match action_id {
        "OpenSearch" | "ToggleSidebar" | "ToggleSftpZoom" | "FocusPaneLeft" | "FocusPaneRight"
        | "FocusPaneUp" | "FocusPaneDown" | "SplitPaneLeft" | "SplitPaneRight" | "SplitPaneUp"
        | "SplitPaneDown" | "ClosePane" | "Copy" | "Paste" => {
            Some(crate::app::constants::TERMINAL_KEY_CONTEXT)
        }
        "SaveDocument" => Some(crate::app::constants::DOCUMENT_KEY_CONTEXT),
        _ => None,
    }
}

pub(crate) fn default_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    }
}

pub(crate) fn default_keystroke(action_id: &str) -> Option<String> {
    WORKSPACE_ACTIONS
        .iter()
        .find(|action| action.id == action_id)
        .map(|action| format!("{}-{}", default_modifier(), action.default_suffix))
}

pub(crate) fn configured_keystroke(config: &ConfigStore, action_id: &str) -> Option<String> {
    config
        .key_bindings()
        .get(action_id)
        .cloned()
        .or_else(|| default_keystroke(action_id))
}

pub(crate) fn normalize_recorded_keystroke(event: &KeyDownEvent) -> Option<String> {
    let key = event.keystroke.key.trim();
    if key.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    if event.keystroke.modifiers.control {
        parts.push("ctrl".to_string());
    }
    if event.keystroke.modifiers.alt {
        parts.push("alt".to_string());
    }
    if event.keystroke.modifiers.shift {
        parts.push("shift".to_string());
    }
    if event.keystroke.modifiers.platform {
        parts.push("cmd".to_string());
    }
    if event.keystroke.modifiers.function {
        parts.push("fn".to_string());
    }

    parts.push(key.to_ascii_lowercase());
    let keystroke = parts.join("-");
    Keystroke::parse(&keystroke).ok().map(|_| keystroke)
}

pub(crate) fn format_keystroke(keystroke: &str) -> String {
    Keystroke::parse(keystroke)
        .map(|stroke| Kbd::format(&stroke))
        .unwrap_or_else(|_| keystroke.to_string())
}

pub(crate) fn bind_workspace_keys_from_config(cx: &mut App, config: &ConfigStore) {
    bind_workspace_actions(cx, config);
}

/// Unbind all workspace keybindings (used when entering keybinding settings to prevent interference).
pub(crate) fn unbind_all_workspace_keys(cx: &mut App, config: &ConfigStore) {
    let mut bindings = Vec::new();

    macro_rules! unbind_action {
        ($id:literal, $action:expr) => {
            let default = default_keystroke($id).expect("workspace action has default key");
            let configured = configured_keystroke(config, $id).unwrap_or_else(|| default.clone());
            let action_name = $action.name();

            // Unbind both the default and configured keystroke
            let context = action_key_context($id);
            bindings.push(KeyBinding::new(
                &default,
                Unbind(action_name.into()),
                context,
            ));
            if configured != default && configured != "none" && !configured.is_empty() {
                bindings.push(KeyBinding::new(
                    &configured,
                    Unbind(action_name.into()),
                    context,
                ));
            }
        };
    }

    unbind_action!("OpenSettings", crate::OpenSettings);
    unbind_action!("OpenSession", crate::OpenSession);
    unbind_action!("OpenTransfers", crate::OpenTransfers);
    unbind_action!("NewSsh", crate::NewSsh);
    unbind_action!("OpenSearch", crate::OpenSearch);
    unbind_action!("ToggleSidebar", crate::ToggleSidebar);
    unbind_action!("ToggleSftpZoom", crate::ToggleSftpZoom);
    unbind_action!("FocusPaneLeft", crate::FocusPaneLeft);
    unbind_action!("FocusPaneRight", crate::FocusPaneRight);
    unbind_action!("FocusPaneUp", crate::FocusPaneUp);
    unbind_action!("FocusPaneDown", crate::FocusPaneDown);
    unbind_action!("SplitPaneLeft", crate::SplitPaneLeft);
    unbind_action!("SplitPaneRight", crate::SplitPaneRight);
    unbind_action!("SplitPaneUp", crate::SplitPaneUp);
    unbind_action!("SplitPaneDown", crate::SplitPaneDown);
    unbind_action!("ClosePane", crate::ClosePane);
    unbind_action!("Copy", crate::Copy);
    unbind_action!("Paste", crate::Paste);
    unbind_action!("SaveDocument", crate::SaveDocument);

    cx.bind_keys(bindings);
}

/// Check if a keystroke conflicts with any other action's binding.
/// Returns Some((conflicting_action_id, label)) if there is a conflict.
pub(crate) fn find_conflict(
    config: &ConfigStore,
    current_action_id: &str,
    new_keystroke: &str,
) -> Option<(String, String)> {
    for action in WORKSPACE_ACTIONS {
        if action.id == current_action_id {
            continue;
        }
        if action_key_context(action.id) != action_key_context(current_action_id) {
            continue;
        }
        let existing = configured_keystroke(config, action.id).unwrap_or_default();
        if !existing.is_empty() && existing != "none" && existing == new_keystroke {
            return Some((action.id.to_string(), t!(action.label_key).to_string()));
        }
    }
    None
}

fn bind_workspace_actions(cx: &mut App, config: &ConfigStore) {
    let mut bindings = Vec::new();

    macro_rules! bind_action {
        ($id:literal, $action:expr) => {
            let default = default_keystroke($id).expect("workspace action has default key");
            let configured = configured_keystroke(config, $id).unwrap_or_else(|| default.clone());
            let action_name = $action.name();

            if configured != default {
                bindings.push(KeyBinding::new(&default, Unbind(action_name.into()), None));
            }

            if configured != "none" && !configured.is_empty() {
                bindings.push(KeyBinding::new(
                    &configured,
                    $action,
                    action_key_context($id),
                ));
            }
        };
    }

    bind_action!("OpenSettings", crate::OpenSettings);
    bind_action!("OpenSession", crate::OpenSession);
    bind_action!("OpenTransfers", crate::OpenTransfers);
    bind_action!("NewSsh", crate::NewSsh);
    bind_action!("OpenSearch", crate::OpenSearch);
    bind_action!("ToggleSidebar", crate::ToggleSidebar);
    bind_action!("ToggleSftpZoom", crate::ToggleSftpZoom);
    bind_action!("FocusPaneLeft", crate::FocusPaneLeft);
    bind_action!("FocusPaneRight", crate::FocusPaneRight);
    bind_action!("FocusPaneUp", crate::FocusPaneUp);
    bind_action!("FocusPaneDown", crate::FocusPaneDown);
    bind_action!("SplitPaneLeft", crate::SplitPaneLeft);
    bind_action!("SplitPaneRight", crate::SplitPaneRight);
    bind_action!("SplitPaneUp", crate::SplitPaneUp);
    bind_action!("SplitPaneDown", crate::SplitPaneDown);
    bind_action!("ClosePane", crate::ClosePane);
    bind_action!("Copy", crate::Copy);
    bind_action!("Paste", crate::Paste);
    bind_action!("SaveDocument", crate::SaveDocument);

    cx.bind_keys(bindings);
}

impl KeybindingsPage {
    pub fn render_groups(view: &Entity<Ashell>, cx: &mut App) -> Vec<SettingGroup> {
        let groups = [
            (
                "settings_group_keybind_general",
                vec![
                    "OpenSettings",
                    "OpenSession",
                    "OpenTransfers",
                    "NewSsh",
                    "OpenSearch",
                    "Copy",
                    "Paste",
                ],
            ),
            (
                "settings_group_keybind_zoom",
                vec!["ToggleSidebar", "ToggleSftpZoom"],
            ),
            (
                "settings_group_keybind_focus",
                vec![
                    "FocusPaneLeft",
                    "FocusPaneRight",
                    "FocusPaneUp",
                    "FocusPaneDown",
                ],
            ),
            (
                "settings_group_keybind_panel",
                vec![
                    "SplitPaneLeft",
                    "SplitPaneRight",
                    "SplitPaneUp",
                    "SplitPaneDown",
                    "ClosePane",
                ],
            ),
            ("settings_group_keybind_document", vec!["SaveDocument"]),
        ];

        let mut result = Vec::new();

        for (group_label, action_ids) in groups {
            let mut group = SettingGroup::new().title(t!(group_label).to_string());

            for action_id in action_ids {
                let action = WORKSPACE_ACTIONS
                    .iter()
                    .find(|a| a.id == action_id)
                    .expect("action exists");

                let recording = view.read(cx).recording_action.as_deref() == Some(action.id);
                let has_error = view
                    .read(cx)
                    .keybind_error
                    .as_ref()
                    .is_some_and(|(id, _)| id == action.id);
                let error_msg = if has_error {
                    view.read(cx)
                        .keybind_error
                        .as_ref()
                        .map(|(_, msg)| msg.clone())
                } else {
                    None
                };

                let keystroke = {
                    let config = &view.read(cx).config;
                    configured_keystroke(config, action.id).unwrap_or_default()
                };

                let btn_label = if recording {
                    t!("press_new_key").to_string()
                } else if keystroke.is_empty() {
                    t!("none").to_string()
                } else {
                    format_keystroke(&keystroke)
                };

                let mut item = SettingItem::new(
                    t!(action.label_key).to_string(),
                    SettingField::render({
                        let view = view.clone();
                        let action_id = action.id.to_string();
                        move |_, _window, _cx| {
                            Button::new(gpui::SharedString::from(format!("keybind-{action_id}")))
                                .label(btn_label.clone())
                                .small()
                                .when(recording, |this| this.primary())
                                .when(has_error, |this| this.danger())
                                .on_click({
                                    let view = view.clone();
                                    let action_id = action_id.clone();
                                    move |_event, window, cx| {
                                        view.update(cx, |this, cx| {
                                            // Clear any previous error when starting new recording
                                            this.keybind_error = None;
                                            this.recording_action = Some(action_id.clone());
                                            this.focus_handle.focus(window, cx);
                                            cx.notify();
                                        });
                                    }
                                })
                                .into_any_element()
                        }
                    }),
                );

                if let Some(msg) = error_msg {
                    item = item.description(msg);
                }

                group = group.item(item);
            }

            result.push(group);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::{WORKSPACE_ACTIONS, action_key_context, default_keystroke};
    use crate::app::constants::{DOCUMENT_KEY_CONTEXT, TERMINAL_KEY_CONTEXT};
    use gpui::Keystroke;

    #[test]
    fn assigns_terminal_and_document_actions_to_separate_contexts() {
        assert_eq!(
            action_key_context("FocusPaneLeft"),
            Some(TERMINAL_KEY_CONTEXT)
        );
        assert_eq!(action_key_context("OpenSearch"), Some(TERMINAL_KEY_CONTEXT));
        assert_eq!(
            action_key_context("SaveDocument"),
            Some(DOCUMENT_KEY_CONTEXT)
        );
        assert_eq!(action_key_context("OpenSettings"), None);
    }

    #[test]
    fn terminal_defaults_do_not_capture_plain_control_letters() {
        for action in WORKSPACE_ACTIONS {
            if action_key_context(action.id) != Some(TERMINAL_KEY_CONTEXT) {
                continue;
            }

            let keystroke = default_keystroke(action.id).unwrap();
            assert!(
                Keystroke::parse(&keystroke).is_ok(),
                "{} has an invalid default key {}",
                action.id,
                keystroke
            );
            let is_plain_control_letter = keystroke
                .strip_prefix("ctrl-")
                .is_some_and(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic());
            assert!(
                !is_plain_control_letter,
                "{} must not capture Linux terminal key {}",
                action.id, keystroke
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn windows_and_linux_defaults_use_terminal_safe_modifiers() {
        assert_eq!(
            default_keystroke("OpenSettings").as_deref(),
            Some("ctrl-shift-,")
        );
        assert_eq!(
            default_keystroke("OpenSession").as_deref(),
            Some("ctrl-shift-o")
        );
        assert_eq!(default_keystroke("Copy").as_deref(), Some("ctrl-shift-c"));
        assert_eq!(default_keystroke("Paste").as_deref(), Some("ctrl-shift-v"));
        assert_eq!(
            default_keystroke("FocusPaneLeft").as_deref(),
            Some("ctrl-alt-left")
        );
        assert_eq!(
            default_keystroke("SplitPaneRight").as_deref(),
            Some("ctrl-alt-shift-right")
        );
    }
}
