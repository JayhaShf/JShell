use gpui::{
    Context, Focusable as _, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    tooltip::Tooltip,
    v_flex,
};
use rust_i18n::t;

use crate::{Ashell, terminal::BackendCommand};

pub(crate) const HISTORY_COMPLETION_PLUGIN_ID: &str = "history-completion";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalPluginDescriptor {
    pub(crate) id: &'static str,
    pub(crate) name_key: &'static str,
}

struct TerminalCompletionContext<'a> {
    entries: &'a [String],
    input: &'a str,
    is_ssh: bool,
    limit: usize,
}

trait TerminalCompletionPlugin: Sync {
    fn descriptor(&self) -> TerminalPluginDescriptor;
    fn candidates(&self, context: &TerminalCompletionContext<'_>) -> Vec<String>;
}

struct HistoryCompletionPlugin;

impl TerminalCompletionPlugin for HistoryCompletionPlugin {
    fn descriptor(&self) -> TerminalPluginDescriptor {
        TerminalPluginDescriptor {
            id: HISTORY_COMPLETION_PLUGIN_ID,
            name_key: "plugin_history_completion",
        }
    }

    fn candidates(&self, context: &TerminalCompletionContext<'_>) -> Vec<String> {
        if !context.is_ssh {
            return Vec::new();
        }

        history_prefix_matches(context.entries, context.input, context.limit)
    }
}

static HISTORY_COMPLETION_PLUGIN: HistoryCompletionPlugin = HistoryCompletionPlugin;
static BUILTIN_TERMINAL_COMPLETION_PLUGINS: [&dyn TerminalCompletionPlugin; 1] =
    [&HISTORY_COMPLETION_PLUGIN];

fn builtin_terminal_completion_plugins() -> &'static [&'static dyn TerminalCompletionPlugin] {
    &BUILTIN_TERMINAL_COMPLETION_PLUGINS
}

fn terminal_completion_plugin_is_enabled(
    plugin_id: &str,
    history_completion_enabled: bool,
) -> bool {
    match plugin_id {
        HISTORY_COMPLETION_PLUGIN_ID => history_completion_enabled,
        _ => false,
    }
}

fn completion_candidates_for(
    entries: &[String],
    input: &str,
    is_ssh: bool,
    history_completion_enabled: bool,
) -> Vec<String> {
    let context = TerminalCompletionContext {
        entries,
        input,
        is_ssh,
        limit: 5,
    };

    builtin_terminal_completion_plugins()
        .iter()
        .filter(|plugin| {
            terminal_completion_plugin_is_enabled(
                plugin.descriptor().id,
                history_completion_enabled,
            )
        })
        .flat_map(|plugin| plugin.candidates(&context))
        .take(context.limit)
        .collect()
}

fn command_input_payload(input: &str) -> Option<Vec<u8>> {
    if input.trim().is_empty() {
        return None;
    }

    let mut payload = input.as_bytes().to_vec();
    if !payload.ends_with(b"\n") {
        payload.push(b'\n');
    }
    Some(payload)
}

fn command_bar_toggle_tooltip_key(is_visible: bool) -> &'static str {
    if is_visible {
        "command_bar_hide"
    } else {
        "command_bar_toggle"
    }
}

fn history_prefix_matches(entries: &[String], input: &str, limit: usize) -> Vec<String> {
    if input.chars().filter(|ch| !ch.is_whitespace()).count() < 2 || limit == 0 {
        return Vec::new();
    }

    entries
        .iter()
        .filter(|command| command.as_str() != input && command.starts_with(input))
        .take(limit)
        .cloned()
        .collect()
}

fn apply_terminal_input_bytes(input: &mut String, bytes: &[u8]) {
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.is_empty()
        && text.chars().all(|ch| !ch.is_control())
    {
        input.push_str(text);
        return;
    }

    for &byte in bytes {
        match byte {
            b'\r' | b'\n' | 0x03 | 0x15 => input.clear(),
            0x08 | 0x7f => {
                input.pop();
            }
            0x17 => {
                while input.ends_with(char::is_whitespace) {
                    input.pop();
                }
                while input.chars().last().is_some_and(|ch| !ch.is_whitespace()) {
                    input.pop();
                }
            }
            b'\t' | 0x01 | 0x05 | 0x0b | 0x1b => {
                input.clear();
                return;
            }
            b' '..=b'~' => input.push(byte as char),
            byte if byte.is_ascii_control() => {
                input.clear();
                return;
            }
            _ => {}
        }
    }
}

impl Ashell {
    pub(crate) fn active_connected_ssh_tab_id(&self) -> Option<String> {
        let active_id = self.active_tab.as_ref()?;
        self.tabs
            .iter()
            .find(|tab| {
                &tab.id == active_id && tab.kind == crate::terminal::TabKind::Ssh && tab.connected
            })
            .map(|tab| tab.id.clone())
    }

    pub(crate) fn start_command_history_load(&mut self, tab_id: &str) -> bool {
        let Some(tab) = self.tabs.iter().find(|tab| {
            tab.id == tab_id && tab.kind == crate::terminal::TabKind::Ssh && tab.connected
        }) else {
            return false;
        };

        tab.send_backend(BackendCommand::LoadCommandHistory);
        let history = self
            .command_history_by_tab
            .entry(tab_id.to_string())
            .or_default();
        history.loading = true;
        history.error = None;
        true
    }

    fn request_command_history(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        if !self.start_command_history_load(tab_id) {
            return;
        }
        cx.notify();
    }

    pub(crate) fn record_terminal_input_bytes(&mut self, tab_id: &str, bytes: &[u8]) {
        if !self.tabs.iter().any(|tab| {
            tab.id == tab_id && tab.kind == crate::terminal::TabKind::Ssh && tab.connected
        }) {
            return;
        }
        if !self.config.history_completion_plugin_enabled() {
            if let Some(history) = self.command_history_by_tab.get_mut(tab_id) {
                history.input_buffer.clear();
            }
            return;
        }
        let history = self
            .command_history_by_tab
            .entry(tab_id.to_string())
            .or_default();
        apply_terminal_input_bytes(&mut history.input_buffer, bytes);
    }

    pub(crate) fn clear_terminal_completion_inputs(&mut self) {
        for history in self.command_history_by_tab.values_mut() {
            history.input_buffer.clear();
        }
    }

    pub(crate) fn record_terminal_paste(&mut self, tab_id: &str, text: &str) {
        if text.contains(['\r', '\n']) {
            if let Some(history) = self.command_history_by_tab.get_mut(tab_id) {
                history.input_buffer.clear();
            }
            return;
        }
        self.record_terminal_input_bytes(tab_id, text.as_bytes());
    }

    pub(crate) fn toggle_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_bar_open {
            self.command_bar_open = false;
            self.command_history_panel_open = false;
            self.command_history_target_tab = None;
            self.focus_handle.focus(window, cx);
        } else if self.active_connected_ssh_tab_id().is_some() {
            self.search_active = false;
            self.command_bar_open = true;
            self.focus_command_bar_input(window, cx);
        }
        cx.notify();
    }

    fn focus_command_bar_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.command_bar_input.clone();
        cx.on_next_frame(window, move |_this, window, cx| {
            input.update(cx, |state, cx| {
                state.focus_handle(cx).focus(window, cx);
            });
        });
    }

    pub(crate) fn execute_command_bar_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.active_connected_ssh_tab_id() else {
            return;
        };
        let command = self.command_bar_input.read(cx).text().to_string();
        let Some(payload) = command_input_payload(&command) else {
            return;
        };

        self.record_terminal_input_bytes(&tab_id, &payload);
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        if tab.render_snapshot(false).display_offset > 0 {
            tab.scroll_to_bottom();
        }
        tab.clear_selection();
        tab.send_backend(BackendCommand::Input(payload));
        Self::set_input_value(&self.command_bar_input, "", window, cx);
        self.focus_command_bar_input(window, cx);
        cx.notify();
    }

    fn toggle_command_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_connected_ssh_tab_id() else {
            return;
        };

        if self.command_history_panel_open
            && self.command_history_target_tab.as_deref() == Some(tab_id.as_str())
        {
            self.close_command_history(window, cx);
            return;
        }

        self.search_active = false;
        self.command_bar_open = true;
        self.command_history_panel_open = true;
        self.command_history_target_tab = Some(tab_id.clone());
        self.command_history_scroll_handle = gpui::ScrollHandle::new();

        let input = self.command_history_input.clone();
        input.update(cx, |state, cx| state.set_value("", window, cx));
        let focus_input = input.clone();
        cx.on_next_frame(window, move |_this, window, cx| {
            focus_input.update(cx, |state, cx| {
                state.focus_handle(cx).focus(window, cx);
            });
        });

        self.request_command_history(&tab_id, cx);
    }

    fn close_command_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_history_panel_open = false;
        self.command_history_target_tab = None;
        if self.command_bar_open {
            self.focus_command_bar_input(window, cx);
        } else {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn insert_history_command(
        &mut self,
        tab_id: &str,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_connected_ssh_tab_id().as_deref() != Some(tab_id) {
            return;
        }
        Self::set_input_value(&self.command_bar_input, command, window, cx);
        self.close_command_history(window, cx);
    }

    fn accept_terminal_completion(
        &mut self,
        tab_id: &str,
        candidate: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_connected_ssh_tab_id().as_deref() != Some(tab_id) {
            return;
        }
        Self::set_input_value(&self.command_bar_input, candidate, window, cx);
        self.focus_command_bar_input(window, cx);
        cx.notify();
    }

    pub(crate) fn render_terminal_completion(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        let Some(tab_id) = self.active_connected_ssh_tab_id() else {
            return div().into_any_element();
        };
        if !self.command_bar_open {
            return div().into_any_element();
        }
        let Some(history) = self.command_history_by_tab.get(&tab_id) else {
            return div().into_any_element();
        };
        let input = self.command_bar_input.read(cx).text().to_string();
        let candidates = completion_candidates_for(
            &history.entries,
            &input,
            true,
            self.config.history_completion_plugin_enabled(),
        );
        if candidates.is_empty() {
            return div().into_any_element();
        }

        div()
            .absolute()
            .left(px(0.))
            .bottom(px(44.))
            .w(px(520.))
            .max_h(px(170.))
            .rounded(px(6.))
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                v_flex().children(
                    candidates
                        .into_iter()
                        .enumerate()
                        .map(|(index, candidate)| {
                            let candidate_to_accept = candidate.clone();
                            let candidate_tab_id = tab_id.clone();
                            let prefix_len = input.len();
                            let (prefix, suffix) = candidate.split_at(prefix_len);
                            h_flex()
                                .id(("terminal-completion", index))
                                .h(px(32.))
                                .w_full()
                                .px_3()
                                .items_center()
                                .overflow_hidden()
                                .border_b_1()
                                .border_color(cx.theme().border.opacity(0.55))
                                .cursor_pointer()
                                .hover(|this| this.bg(cx.theme().secondary_hover))
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.accept_terminal_completion(
                                        &candidate_tab_id,
                                        &candidate_to_accept,
                                        window,
                                        cx,
                                    );
                                }))
                                .child(
                                    div().min_w(px(0.)).truncate().text_size(rems(0.833)).child(
                                        h_flex()
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(prefix.to_string()),
                                            )
                                            .child(div().child(suffix.to_string())),
                                    ),
                                )
                        }),
                ),
            )
            .into_any_element()
    }

    pub(crate) fn render_command_bar_button(
        &self,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        let tooltip_key = command_bar_toggle_tooltip_key(self.command_bar_open);
        Button::new("command-bar-toggle")
            .ghost()
            .small()
            .icon(IconName::SquareTerminal)
            .tooltip(t!(tooltip_key).to_string())
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_command_bar(window, cx);
            }))
    }

    pub(crate) fn render_command_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        if self.active_connected_ssh_tab_id().is_none() {
            return div().into_any_element();
        }
        let has_command = !self
            .command_bar_input
            .read(cx)
            .text()
            .to_string()
            .trim()
            .is_empty();

        div()
            .id("command-bar")
            .flex_none()
            .relative()
            .mt(px(8.))
            .h(px(36.))
            .w_full()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .size_full()
                    .gap_1()
                    .p_1()
                    .rounded(px(6.))
                    .bg(cx.theme().popover)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .px_2()
                            .text_size(rems(0.75))
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("command_input").to_string()),
                    )
                    .child(Input::new(&self.command_bar_input).small().flex_1())
                    .child(
                        Button::new("command-bar-history")
                            .ghost()
                            .xsmall()
                            .icon(IconName::BookOpen)
                            .tooltip(t!("command_history").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_command_history(window, cx);
                            })),
                    )
                    .child(
                        Button::new("command-bar-send")
                            .primary()
                            .xsmall()
                            .icon(IconName::Play)
                            .disabled(!has_command)
                            .tooltip(t!("command_send").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.execute_command_bar_input(window, cx);
                            })),
                    )
                    .child(
                        Button::new("command-bar-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(t!("command_bar_hide").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_command_bar(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_command_history_panel(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        let Some(tab_id) = self.command_history_target_tab.clone() else {
            return div().into_any_element();
        };
        if self.active_connected_ssh_tab_id().as_deref() != Some(tab_id.as_str()) {
            return div().into_any_element();
        }

        let state = self
            .command_history_by_tab
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        let query = self
            .command_history_input
            .read(cx)
            .text()
            .to_string()
            .trim()
            .to_lowercase();
        let entries = state
            .entries
            .iter()
            .filter(|command| query.is_empty() || command.to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        let refresh_tab_id = tab_id.clone();

        div()
            .absolute()
            .left(px(0.))
            .bottom(px(44.))
            .w(px(520.))
            .h(px(320.))
            .rounded(px(6.))
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(44.))
                            .px_3()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t!("command_history").to_string()),
                            )
                            .child(
                                Button::new("command-history-refresh")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Redo2)
                                    .disabled(state.loading)
                                    .tooltip(t!("command_history_refresh").to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.request_command_history(&refresh_tab_id, cx);
                                    })),
                            )
                            .child(
                                Button::new("command-history-close")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_command_history(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .p_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if event.keystroke.key.as_str() == "escape" {
                                        this.close_command_history(window, cx);
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .child(Input::new(&self.command_history_input).small()),
                    )
                    .child(
                        div()
                            .id("command-history-list")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .track_scroll(&self.command_history_scroll_handle)
                            .when(state.loading && state.entries.is_empty(), |this| {
                                this.child(
                                    div()
                                        .p_4()
                                        .text_size(rems(0.833))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t!("command_history_loading").to_string()),
                                )
                            })
                            .when_some(state.error.clone(), |this, error| {
                                this.child(
                                    div()
                                        .p_4()
                                        .text_size(rems(0.833))
                                        .text_color(cx.theme().danger)
                                        .child(format!(
                                            "{}: {}",
                                            t!("command_history_failed"),
                                            error
                                        )),
                                )
                            })
                            .when(
                                !state.loading && state.error.is_none() && entries.is_empty(),
                                |this| {
                                    this.child(
                                        div()
                                            .p_4()
                                            .text_size(rems(0.833))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(t!("command_history_empty").to_string()),
                                    )
                                },
                            )
                            .children(entries.into_iter().enumerate().map(|(index, command)| {
                                let command_to_insert = command.clone();
                                let command_tab_id = tab_id.clone();
                                let command_tooltip = command.clone();
                                div()
                                    .id(("command-history-entry", index))
                                    .h(px(34.))
                                    .w_full()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(cx.theme().border.opacity(0.55))
                                    .cursor_pointer()
                                    .hover(|this| this.bg(cx.theme().secondary_hover))
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(command_tooltip.clone()).build(window, cx)
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.insert_history_command(
                                            &command_tab_id,
                                            &command_to_insert,
                                            window,
                                            cx,
                                        );
                                    }))
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .flex_1()
                                            .truncate()
                                            .text_size(rems(0.833))
                                            .child(command),
                                    )
                            })),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod completion_tests {
    use super::{
        HISTORY_COMPLETION_PLUGIN_ID, apply_terminal_input_bytes,
        builtin_terminal_completion_plugins, command_bar_toggle_tooltip_key, command_input_payload,
        completion_candidates_for, history_prefix_matches,
    };

    #[test]
    fn builtin_plugins_register_history_completion_with_a_stable_id() {
        let plugins = builtin_terminal_completion_plugins();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].descriptor().id, HISTORY_COMPLETION_PLUGIN_ID);
        assert_eq!(
            plugins[0].descriptor().name_key,
            "plugin_history_completion"
        );
    }

    #[test]
    fn history_completion_only_runs_when_enabled_for_ssh() {
        let entries = vec!["git status".to_string(), "git stash".to_string()];

        assert_eq!(
            completion_candidates_for(&entries, "git ", true, true),
            vec!["git status".to_string(), "git stash".to_string()]
        );
        assert!(completion_candidates_for(&entries, "git ", true, false).is_empty());
        assert!(completion_candidates_for(&entries, "git ", false, true).is_empty());
    }

    #[test]
    fn command_bar_payload_skips_blank_input_and_appends_a_newline() {
        assert_eq!(command_input_payload("   "), None);
        assert_eq!(
            command_input_payload("git status"),
            Some(b"git status\n".to_vec())
        );
        assert_eq!(command_input_payload("pwd\n"), Some(b"pwd\n".to_vec()));
    }

    #[test]
    fn history_matches_keep_server_order_and_exclude_exact_input() {
        let entries = vec![
            "git status".to_string(),
            "git stash list".to_string(),
            "git status --short".to_string(),
            "grep TODO".to_string(),
        ];

        assert_eq!(
            history_prefix_matches(&entries, "git sta", 2),
            vec!["git status".to_string(), "git stash list".to_string()]
        );
        assert_eq!(
            history_prefix_matches(&entries, "git status", 5),
            vec!["git status --short".to_string()]
        );
    }

    #[test]
    fn history_matches_require_two_non_whitespace_characters() {
        let entries = vec!["ls".to_string(), "ls -la".to_string()];

        assert!(history_prefix_matches(&entries, "l", 5).is_empty());
        assert!(history_prefix_matches(&entries, "  ", 5).is_empty());
    }

    #[test]
    fn terminal_input_tracker_handles_text_and_common_deletions() {
        let mut input = String::new();
        apply_terminal_input_bytes(&mut input, b"git status");
        apply_terminal_input_bytes(&mut input, &[0x7f]);
        assert_eq!(input, "git statu");

        apply_terminal_input_bytes(&mut input, &[0x17]);
        assert_eq!(input, "git ");

        apply_terminal_input_bytes(&mut input, &[0x15]);
        assert!(input.is_empty());
    }

    #[test]
    fn terminal_input_tracker_resets_on_submit_or_ambiguous_sequences() {
        let mut input = "cargo tes".to_string();
        apply_terminal_input_bytes(&mut input, b"\r");
        assert!(input.is_empty());

        input.push_str("cargo");
        apply_terminal_input_bytes(&mut input, b"\x1b[D");
        assert!(input.is_empty());

        input.push_str("cargo");
        apply_terminal_input_bytes(&mut input, b"\t");
        assert!(input.is_empty());
    }

    #[test]
    fn command_bar_toggle_tooltip_key_matches_visibility() {
        assert_eq!(command_bar_toggle_tooltip_key(false), "command_bar_toggle");
        assert_eq!(command_bar_toggle_tooltip_key(true), "command_bar_hide");
    }
}
