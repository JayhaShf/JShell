use crate::app::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui::{
    Context, ElementId, Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, PathBuilder, Pixels, Render, Role,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point,
    prelude::FluentBuilder as _, px, relative, rems, uniform_list,
};
use gpui_component::{
    ActiveTheme, Disableable as _, ElementExt, Icon, IconName, Root, Sizable as _, Size,
    WindowExt as _,
    animation::Transition,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::Input,
    menu::{ContextMenuExt as _, PopupMenuItem},
    notification::Notification,
    progress::Progress,
    scroll::{ScrollableElement as _, Scrollbar, ScrollbarShow},
    v_flex,
};
use rust_i18n::t;

use crate::{
    Ashell, PaneLayout, PaneLeaf,
    app::constants::{
        COLLAPSED_SIDEBAR_WIDTH, COMPACT_ICON_SIZE, SFTP_STATUS_HEIGHT, SFTP_TOOLBAR_HEIGHT,
        SIDEBAR_PRIMARY_ACTION_HEIGHT, SIDEBAR_SECTION_HEIGHT, SIDEBAR_WIDTH, TAB_BAR_HEIGHT,
        TERMINAL_KEY_CONTEXT, TERMINAL_PADDING_X, TERMINAL_PADDING_Y, TERMINAL_SCROLLBAR_GUTTER,
    },
    app::workspace_tabs::{
        WorkspaceTabColorRole, WorkspaceTabKeyboardAction, WorkspaceTabStatus,
        WorkspaceTabVisualKind, aggregate_terminal_workspace_status, document_workspace_status,
        terminal_tab_status, workspace_tab_accessibility_label, workspace_tab_color_role,
        workspace_tab_keyboard_action,
    },
    sftp::ops::{SftpEntryAction, SftpEntryGesture, is_editable_text_file, sftp_entry_action},
    sftp::{
        format_mtime,
        permissions::{RemoteFileType, format_permissions},
    },
    system::format_bytes,
    terminal,
};

#[derive(Clone)]
enum WorkspaceCloseTarget {
    Session(String),
    Document(String),
}

struct WorkspaceTabRenderData {
    workspace_id: String,
    label: String,
    visual_kind: WorkspaceTabVisualKind,
    status: WorkspaceTabStatus,
    dirty: bool,
    connected_ssh: bool,
    close_target: WorkspaceCloseTarget,
}

impl Ashell {
    fn render_home_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .h_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .text_size(rems(2.333))
                    .font_weight(FontWeight::BOLD)
                    .child("JShell"),
            )
            .child(
                div()
                    .text_size(rems(1.083))
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("open_local_or_ssh")),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("home-open-local")
                            .primary()
                            .label(t!("local_terminal").to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.open_local(cx))),
                    )
                    .child(
                        Button::new("home-open-session")
                            .ghost()
                            .label(t!("open_session").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_selector_dialog(window, cx)
                            })),
                    ),
            )
    }

    pub(crate) fn toggle_sftp_minimized(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.body_panels.clone();
        let minimized = self.sftp_panel_minimized;

        if !minimized {
            let sizes = state.read(cx).sizes();
            if sizes.len() > 1 {
                self.prev_monitoring_size = Some(sizes[1]);
            }
            self.sftp_panel_minimized = true;
        } else {
            self.sftp_panel_minimized = false;
            let prev_size = self.prev_monitoring_size.unwrap_or(px(328.));

            cx.on_next_frame(
                window,
                move |_this: &mut crate::app::Ashell,
                      window: &mut gpui::Window,
                      cx: &mut gpui::Context<crate::app::Ashell>| {
                    cx.on_next_frame(
                        window,
                        move |this: &mut crate::app::Ashell,
                              window: &mut gpui::Window,
                              cx: &mut gpui::Context<crate::app::Ashell>| {
                            this.body_panels.update(cx, |state, cx| {
                                let sizes = state.sizes();
                                let c_size_f32: f32 = sizes.iter().map(|s| s.as_f32()).sum();
                                let c_size = px(c_size_f32);

                                if c_size > px(0.0) && prev_size < c_size {
                                    let target_p0 = c_size - prev_size;
                                    state.resize_panel(0, target_p0, window, cx);
                                }
                            });
                            cx.notify();
                        },
                    );
                },
            );
        }
        self.config
            .set_sftp_panel_minimized(self.sftp_panel_minimized);
        self.save_preferences_background();
        cx.notify();
    }

    fn render_sftp_panel(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_sftp = self.active_sftp();

        // Compute active download progress for status bar and minimized header
        let build_summary = |kind: crate::terminal::TransferType| -> Option<(String, String, f32)> {
            let active: Vec<&crate::terminal::Transfer> = self
                .transfers
                .iter()
                .filter(|t| {
                    matches!(
                        t.state,
                        crate::terminal::TransferState::Running
                            | crate::terminal::TransferState::Paused
                    ) && t.info.kind == kind
                })
                .collect();
            if active.is_empty() {
                return None;
            }
            Some(if active.len() == 1 {
                let t = &active[0];
                let pct = t.total.and_then(|total| {
                    if total > 0 {
                        Some((t.transferred as f64 / total as f64 * 100.0) as f32)
                    } else {
                        None
                    }
                });
                match pct {
                    Some(pct) => (t.info.name.clone(), format!("{:.0}%", pct), pct),
                    None => (t.info.name.clone(), "-".to_string(), 0.0),
                }
            } else {
                let total_transferred: u64 = active.iter().map(|t| t.transferred).sum();
                let total_total: u64 = active.iter().filter_map(|t| t.total).sum();
                let pct = if total_total > 0 {
                    Some((total_transferred as f64 / total_total as f64 * 100.0) as f32)
                } else {
                    None
                };
                let label = match kind {
                    crate::terminal::TransferType::Download => {
                        t!("files_downloading", count = active.len()).to_string()
                    }
                    crate::terminal::TransferType::Upload => {
                        t!("files_uploading", count = active.len()).to_string()
                    }
                };
                match pct {
                    Some(pct) => (label, format!("{:.0}%", pct), pct),
                    None => (label, "-".to_string(), 0.0),
                }
            })
        };
        let dl_summary = build_summary(crate::terminal::TransferType::Download);
        let ul_summary = build_summary(crate::terminal::TransferType::Upload);
        let has_transfers = dl_summary.is_some() || ul_summary.is_some();

        let header = h_flex()
            .flex_none()
            .h(px(SFTP_TOOLBAR_HEIGHT))
            .px_3()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tab_bar)
            .child(
                div()
                    .text_size(rems(0.833))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().primary)
                    .child(t!("remote_files")),
            )
            .child(div().flex_1())
            .when_some(active_sftp, |this, sftp| {
                let selected_entries = sftp.selected_entries.clone();
                let delete_in_progress = !sftp.deleting_entries.is_empty();
                this.child(
                    Button::new("sftp-sync-cwd")
                        .ghost()
                        .small()
                        .icon(IconName::SquareTerminal)
                        .tooltip(t!("sync_cwd_tooltip").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.sync_cwd_from_terminal(window, cx);
                        })),
                )
                .child(
                    Button::new("sftp-reconnect")
                        .ghost()
                        .small()
                        .icon(IconName::Redo2)
                        .tooltip(t!("sftp_retry_now").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reconnect_active_sftp(cx);
                        })),
                )
                .child(
                    Button::new("sftp-refresh")
                        .ghost()
                        .small()
                        .icon(IconName::Redo2)
                        .tooltip(t!("refresh").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_sftp(cx))),
                )
                .child(
                    Button::new("sftp-new-folder")
                        .ghost()
                        .small()
                        .icon(IconName::Folder)
                        .tooltip(t!("new_folder").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.sftp_creating_folder = true;
                            this.sftp_new_folder_input.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus_handle(cx).focus(window, cx);
                            });
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("sftp-delete-selected")
                        .ghost()
                        .small()
                        .icon(IconName::Delete)
                        .tooltip(t!("delete_selected").to_string())
                        .disabled(selected_entries.is_empty() || delete_in_progress)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_delete_confirm_dialog(window, cx);
                        })),
                )
                .child(
                    Button::new("sftp-upload-file")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip(t!("upload_file").to_string())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.upload_sftp_files(window, cx)),
                        ),
                )
                .child(
                    Button::new("sftp-upload-folder")
                        .ghost()
                        .small()
                        .icon(IconName::Folder)
                        .tooltip(t!("upload_folder").to_string())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.upload_sftp_folder(window, cx)),
                        ),
                )
                .child(
                    Button::new("sftp-download-selected")
                        .ghost()
                        .small()
                        .icon(IconName::ArrowDown)
                        .tooltip(t!("download").to_string())
                        .disabled(selected_entries.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.download_selected_sftp_entries(window, cx);
                        })),
                )
                .child(
                    Checkbox::new("sftp-show-hidden")
                        .small()
                        .label(t!("hidden").to_string())
                        .checked(self.show_hidden_files)
                        .tab_stop(false)
                        .on_click(cx.listener(|this, checked, _, cx| {
                            this.show_hidden_files = *checked;
                            this.config.set_show_hidden_files(*checked);
                            this.save_preferences_background();
                            cx.notify();
                        })),
                )
            });
        let Some(sftp) = active_sftp else {
            let mut outer = v_flex()
                .gap_0()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .flex_1()
                .child(
                    v_flex()
                        .flex_1()
                        .min_h(px(0.))
                        .when(self.sftp_panel_minimized, |this| this.hidden())
                        .child(header)
                        .child(
                            v_flex()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .p_3()
                                .child(
                                    div()
                                        .text_size(rems(1.0))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t!("open_ssh_tab_sftp")),
                                ),
                        ),
                );
            outer = outer.child(
                h_flex()
                    .flex_none()
                    .h(px(24.))
                    .px_3()
                    .items_center()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(div().flex_1())
                    .when(self.active_connected_ssh_tab_id().is_some(), |this| {
                        this.child(self.render_command_bar_button(cx))
                    })
                    .child(
                        Button::new("open-transfers")
                            .ghost()
                            .small()
                            .when(has_transfers, |this| {
                                let mut content = h_flex().items_center().gap_2();
                                if let Some((ref label, ref pct_display, pct)) = dl_summary {
                                    content = content.child(
                                        h_flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                Icon::new(IconName::ArrowDown)
                                                    .with_size(Size::Small)
                                                    .text_color(cx.theme().primary),
                                            )
                                            .child(
                                                div()
                                                    .text_size(rems(0.833))
                                                    .text_color(cx.theme().primary)
                                                    .child(label.clone()),
                                            )
                                            .child(
                                                Progress::new("sftp-status-dl")
                                                    .with_size(px(4.))
                                                    .value(pct)
                                                    .color(cx.theme().primary)
                                                    .w(px(50.0)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(rems(0.833))
                                                    .text_color(cx.theme().primary)
                                                    .child(pct_display.clone()),
                                            ),
                                    );
                                }
                                if let Some((ref label, ref pct_display, pct)) = ul_summary {
                                    if dl_summary.is_some() {
                                        content = content.child(div().w(px(6.)));
                                    }
                                    content = content.child(
                                        h_flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                Icon::new(IconName::ArrowUp)
                                                    .with_size(Size::Small)
                                                    .text_color(cx.theme().primary),
                                            )
                                            .child(
                                                div()
                                                    .text_size(rems(0.833))
                                                    .text_color(cx.theme().primary)
                                                    .child(label.clone()),
                                            )
                                            .child(
                                                Progress::new("sftp-status-ul")
                                                    .with_size(px(4.))
                                                    .value(pct)
                                                    .color(cx.theme().primary)
                                                    .w(px(50.0)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(rems(0.833))
                                                    .text_color(cx.theme().primary)
                                                    .child(pct_display.clone()),
                                            ),
                                    );
                                }
                                this.child(content)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_transfers_dialog(window, cx);
                                    }))
                            })
                            .when(!has_transfers, |this| {
                                this.icon(IconName::ArrowDown)
                                    .label(t!("transfers").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_transfers_dialog(window, cx);
                                    }))
                            }),
                    )
                    .child(
                        Button::new("sftp-minimize-toggle")
                            .ghost()
                            .small()
                            .icon(if self.sftp_panel_minimized {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .label(if self.sftp_panel_minimized {
                                t!("panel_expand").to_string()
                            } else {
                                t!("panel_minimize").to_string()
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sftp_minimized(window, cx);
                            })),
                    ),
            );
            return outer.into_any_element();
        };

        let selected_path = sftp.selected_path.clone();
        let preview = sftp.preview.clone();
        let entries = sftp
            .entries
            .clone()
            .into_iter()
            .filter(|entry| self.show_hidden_files || !entry.name.starts_with('.'))
            .collect::<Vec<_>>();
        let status = sftp.status.clone();
        let selected_entries = sftp.selected_entries.clone();
        let all_selected = !entries.is_empty()
            && entries
                .iter()
                .all(|e| selected_entries.contains(&e.full_path));
        let parent_path = Self::sftp_parent_path(&sftp.current_path);
        let view = cx.entity();
        let icon_col_width = px(14.);
        let permissions_col_width = px(148.);
        let size_col_width = px(96.);
        let modified_col_width = px(152.);

        let mut outer = v_flex()
            .gap_0()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .flex_1()
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                    let paths_to_upload: Vec<String> = paths
                        .0
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    this.upload_sftp_files_batch(paths_to_upload, cx);
                }),
            );

        let preview_panel = match preview {
            Some(preview) => v_flex()
                .flex_none()
                .h(px(96.))
                .min_h(px(0.))
                .flex_shrink_1()
                .w_full()
                .gap_1()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted.opacity(0.25))
                .overflow_hidden()
                .when(self.sftp_panel_minimized, |this| this.hidden())
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(
                            div()
                                .text_size(rems(0.833))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().primary)
                                .child(preview.title),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_size(rems(0.75))
                                .text_color(cx.theme().muted_foreground)
                                .child(preview.path),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scrollbar()
                        .text_size(rems(0.833))
                        .text_color(cx.theme().foreground)
                        .child(preview.body),
                )
                .into_any_element(),
            None => div().h(px(0.)).flex_none().into_any_element(),
        };

        outer = outer.child(
            v_flex()
                .flex_1()
                .min_h(px(0.))
                .when(self.sftp_panel_minimized, |this| this.hidden())
                .child(header)
                .child(
                    h_flex()
                        .h(px(36.))
                        .items_center()
                        .gap_2()
                        .px_3()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().muted)
                        .child(
                            Button::new("sftp-up")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronUp)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.navigate_sftp(parent_path.clone(), cx);
                                })),
                        )
                        .child(Input::new(&self.sftp_path_input).flex_1().tab_index(0))
                        .child(div().flex_none()),
                )
                .child(
                    h_flex()
                        .h(px(26.))
                        .px_3()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().muted.opacity(0.8))
                        .child(
                            h_flex()
                                .w(px(24.))
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .child(
                                    Checkbox::new("sftp-select-all")
                                        .checked(all_selected)
                                        .on_click(cx.listener(move |this, checked, _, cx| {
                                            this.toggle_all_sftp_entries(*checked, cx);
                                        })),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .items_center()
                                .gap_2()
                                .child(div().w(icon_col_width).flex_none())
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(rems(0.917))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t!("name")),
                                ),
                        )
                        .child(
                            div()
                                .w(permissions_col_width)
                                .flex_none()
                                .text_size(rems(0.917))
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("permissions")),
                        )
                        .child(
                            div()
                                .w(size_col_width)
                                .flex_none()
                                .text_size(rems(0.917))
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("size")),
                        )
                        .child(
                            div()
                                .w(modified_col_width)
                                .flex_none()
                                .text_size(rems(0.917))
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("modified")),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .relative()
                        .min_h(px(0.))
                        .child({
                            let entries = entries.clone();
                            let selected_entries = selected_entries.clone();
                            let selected_path = selected_path.clone();
                            let view = view.clone();
                            let theme = cx.theme().clone();
                            uniform_list(
                                "sftp-entries-list",
                                entries.len(),
                                move |range, window, _cx| {
                                    range
                                        .into_iter()
                                        .filter_map(|ix| {
                                            let entry = entries.get(ix)?;
                                            let entry = entry.clone();
                                            let is_checked =
                                                selected_entries.contains(&entry.full_path);
                                            let is_selected = selected_path.as_deref()
                                                == Some(entry.full_path.as_str());
                                            let name_color = if entry.is_dir {
                                                theme.primary
                                            } else {
                                                theme.foreground
                                            };
                                            let bg = if is_selected {
                                                theme.secondary
                                            } else if ix % 2 == 0 {
                                                theme.background
                                            } else {
                                                theme.muted.opacity(0.5)
                                            };
                                            Some(
                                            h_flex()
                                                .w_full()
                                                .h(px(28.))
                                                .items_center()
                                                .px_3()
                                                .gap_2()
                                                .bg(bg)
                                                .hover(|style| style.bg(theme.muted.opacity(0.8)))
                                                .border_b_1()
                                                .border_color(theme.border.opacity(0.35))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    window.listener_for(&view, {
                                                        let entry = entry.clone();
                                                        move |this, event: &MouseDownEvent, window, cx| {
                                                            this.dismiss_sftp_context_menu(cx);
                                                            let gesture = if event.click_count > 1 {
                                                                SftpEntryGesture::DoubleClick
                                                            } else {
                                                                SftpEntryGesture::SingleClick
                                                            };
                                                            match sftp_entry_action(entry.is_dir, gesture) {
                                                                SftpEntryAction::Focus => {
                                                                    this.focus_sftp_entry(&entry, cx);
                                                                }
                                                                SftpEntryAction::OpenFile => {
                                                                    this.open_remote_document(
                                                                        entry.full_path.clone(),
                                                                        window,
                                                                        cx,
                                                                    );
                                                                }
                                                                SftpEntryAction::NavigateDirectory => {
                                                                    this.navigate_sftp(
                                                                        entry.full_path.clone(),
                                                                        cx,
                                                                    );
                                                                }
                                                                SftpEntryAction::ToggleSelection => {}
                                                            }
                                                        }
                                                    }),
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Right,
                                                    window.listener_for(&view, {
                                                        let entry = entry.clone();
                                                        let remote_path = entry.full_path.clone();
                                                        move |this, event: &MouseDownEvent, _, cx| {
                                                            this.mark_sftp_entry_selected(
                                                                &entry.full_path,
                                                                cx,
                                                            );
                                                            this.open_sftp_context_menu(
                                                                remote_path.clone(),
                                                                entry.is_dir,
                                                                event.position,
                                                                cx,
                                                            );
                                                        }
                                                    }),
                                                )
                                                .child(
                                                    h_flex()
                                                        .w(px(24.))
                                                        .flex_none()
                                                        .items_center()
                                                        .justify_center()
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| cx.stop_propagation(),
                                                        )
                                                        .on_mouse_down(
                                                            MouseButton::Right,
                                                            |_, _, cx| cx.stop_propagation(),
                                                        )
                                                        .child(
                                                            Checkbox::new(ElementId::Name(
                                                                format!(
                                                                    "check-{}",
                                                                    entry.full_path
                                                                )
                                                                .into(),
                                                            ))
                                                            .checked(is_checked)
                                                            .on_click(window.listener_for(&view, {
                                                                let path = entry.full_path.clone();
                                                                let is_dir = entry.is_dir;
                                                                move |this, checked, _, cx| {
                                                                    if sftp_entry_action(
                                                                        is_dir,
                                                                        SftpEntryGesture::Checkbox,
                                                                    ) == SftpEntryAction::ToggleSelection
                                                                    {
                                                                        this.toggle_sftp_entry(
                                                                            path.clone(),
                                                                            *checked,
                                                                            cx,
                                                                        );
                                                                    }
                                                                }
                                                            })),
                                                        ),
                                                )
                                                .child(
                                                    h_flex()
                                                        .flex_1()
                                                        .min_w(px(0.))
                                                        .items_center()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .w(icon_col_width)
                                                                .flex_none()
                                                                .text_size(rems(1.0))
                                                                .text_color(name_color)
                                                                .child(match entry.file_type {
                                                                    RemoteFileType::Directory => {
                                                                        "📁"
                                                                    }
                                                                    RemoteFileType::Symlink => {
                                                                        "🔗"
                                                                    }
                                                                    _ => "📄",
                                                                }),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w(px(0.))
                                                                .overflow_hidden()
                                                                .text_size(rems(1.0))
                                                                .text_color(name_color)
                                                                .child(entry.name),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .w(permissions_col_width)
                                                        .flex_none()
                                                        .text_size(rems(0.917))
                                                        .text_color(theme.muted_foreground)
                                                        .child(format_permissions(
                                                            entry.permissions,
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .w(size_col_width)
                                                        .flex_none()
                                                        .text_size(rems(0.917))
                                                        .text_color(theme.muted_foreground)
                                                        .child(if entry.is_dir {
                                                            "-".to_string()
                                                        } else {
                                                            format_bytes(entry.size)
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .w(modified_col_width)
                                                        .flex_none()
                                                        .text_size(rems(0.917))
                                                        .text_color(theme.muted_foreground)
                                                        .child(format_mtime(entry.modified)),
                                                )
                                                .child(div().w(px(12.)).flex_none())
                                                .into_any_element(),
                                        )
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .size_full()
                            .track_scroll(&self.remote_files_scroll_handle)
                        })
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .bottom_0()
                                .w(px(16.))
                                .child(
                                    Scrollbar::vertical(&self.remote_files_scroll_handle)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        ),
                ),
        );
        outer = outer.child(preview_panel);
        outer = outer.child(
            h_flex()
                .flex_none()
                .h(px(SFTP_STATUS_HEIGHT))
                .px_3()
                .items_center()
                .border_t_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().tab_bar)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_size(rems(0.833))
                        .text_color(cx.theme().primary)
                        .child(status),
                )
                .when(self.active_connected_ssh_tab_id().is_some(), |this| {
                    this.child(self.render_command_bar_button(cx))
                })
                .child(
                    Button::new("open-transfers")
                        .ghost()
                        .small()
                        .when(has_transfers, |this| {
                            let mut content = h_flex().items_center().gap_2();
                            if let Some((ref label, ref pct_display, pct)) = dl_summary {
                                content = content.child(
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            Icon::new(IconName::ArrowDown)
                                                .with_size(Size::Small)
                                                .text_color(cx.theme().primary),
                                        )
                                        .child(
                                            div()
                                                .text_size(rems(0.833))
                                                .text_color(cx.theme().primary)
                                                .child(label.clone()),
                                        )
                                        .child(
                                            Progress::new("sftp-status-dl")
                                                .with_size(px(4.))
                                                .value(pct)
                                                .color(cx.theme().primary)
                                                .w(px(50.0)),
                                        )
                                        .child(
                                            div()
                                                .text_size(rems(0.833))
                                                .text_color(cx.theme().primary)
                                                .child(pct_display.clone()),
                                        ),
                                );
                            }
                            if let Some((ref label, ref pct_display, pct)) = ul_summary {
                                if dl_summary.is_some() {
                                    content = content.child(div().w(px(6.)));
                                }
                                content = content.child(
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            Icon::new(IconName::ArrowUp)
                                                .with_size(Size::Small)
                                                .text_color(cx.theme().primary),
                                        )
                                        .child(
                                            div()
                                                .text_size(rems(0.833))
                                                .text_color(cx.theme().primary)
                                                .child(label.clone()),
                                        )
                                        .child(
                                            Progress::new("sftp-status-ul")
                                                .with_size(px(4.))
                                                .value(pct)
                                                .color(cx.theme().primary)
                                                .w(px(50.0)),
                                        )
                                        .child(
                                            div()
                                                .text_size(rems(0.833))
                                                .text_color(cx.theme().primary)
                                                .child(pct_display.clone()),
                                        ),
                                );
                            }
                            this.child(content)
                        })
                        .when(!has_transfers, |this| {
                            this.icon(IconName::ArrowDown)
                                .label(t!("transfers").to_string())
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_transfers_dialog(window, cx);
                        })),
                )
                .child(
                    Button::new("sftp-minimize-toggle")
                        .ghost()
                        .small()
                        .icon(if self.sftp_panel_minimized {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .label(if self.sftp_panel_minimized {
                            t!("panel_expand").to_string()
                        } else {
                            t!("panel_minimize").to_string()
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_sftp_minimized(window, cx);
                        })),
                ),
        );

        outer.into_any_element()
    }

    fn render_monitoring_panel(
        &mut self,
        viewport_width: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cpu_color = cx.theme().chart_1;
        let mem_color = cx.theme().chart_2;
        let swap_color = cx.theme().chart_3;
        let net_color = cx.theme().chart_4;
        let disk_color = cx.theme().chart_5;
        let border_color = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let cpu_pct = self.system.cpu_percent;
        // Dynamic CPU line color: green <30%, amber 30-80%, red >80%
        // NOTE: Hsla.h is normalized 0..=1 (not degrees)
        let cpu_path_color = {
            let pct = cpu_pct * 100.0;
            if pct < 30.0 {
                Hsla {
                    h: 120.0 / 360.0,
                    s: 0.65,
                    l: 0.45,
                    a: 1.0,
                }
            } else if pct < 80.0 {
                Hsla {
                    h: 45.0 / 360.0,
                    s: 0.8,
                    l: 0.55,
                    a: 1.0,
                }
            } else {
                Hsla {
                    h: 0.0,
                    s: 0.8,
                    l: 0.55,
                    a: 1.0,
                }
            }
        };
        // Network TX color: derived from net_color for visual distinction from RX
        let net_tx_color = if net_color.l > 0.5 {
            Hsla {
                l: net_color.l * 0.6,
                ..net_color
            }
        } else {
            Hsla {
                l: net_color.l * 1.5,
                ..net_color
            }
        };
        let mem_pct = self.system.mem_percent;
        let swap_pct = self.system.swap_percent;
        let mem_detail = self.system.mem_detail.clone();
        let swap_detail = self.system.swap_detail.clone();
        let net_rx = self.system.net_rx.clone();
        let net_tx = self.system.net_tx.clone();

        let (disk_used, disk_total) = self.system.disks.iter().fold((0u64, 0u64), |(u, t), d| {
            (u + (d.total_bytes - d.available_bytes), t + d.total_bytes)
        });
        let disk_pct = if disk_total > 0 {
            disk_used as f64 / disk_total as f64 * 100.0
        } else {
            0.0
        };

        let cpu_spark_data = self.cpu_history.clone();
        let net_rx_history = self.net_rx_history.clone();
        let net_tx_history = self.net_tx_history.clone();
        let disks = self.system.disks.clone();
        let card_min_w = px(110.);

        let show_net_card = viewport_width > px(750.);
        let show_disk_card = viewport_width > px(600.);

        // --- CPU card ---
        let cpu_card = v_flex()
            .min_w(card_min_w)
            .flex_1()
            .h_full()
            .px_1()
            .py_1()
            .gap_0p5()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .child(
                        div()
                            .text_size(rems(0.833))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cpu_color)
                            .child(t!("cpu").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(rems(0.833))
                            .text_color(muted_fg)
                            .child(format!("{:.0}%", cpu_pct * 100.0)),
                    ),
            )
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        let n = cpu_spark_data.len();
                        if n < 2 {
                            return None;
                        }
                        let mut path = PathBuilder::stroke(px(1.5));
                        let w = bounds.size.width;
                        let h = bounds.size.height;
                        let max_val = cpu_spark_data
                            .iter()
                            .cloned()
                            .fold(0.0f32, f32::max)
                            .max(0.1);
                        for (i, &val) in cpu_spark_data.iter().enumerate() {
                            let x = bounds.origin.x + w * i as f32 / (n - 1).max(1) as f32;
                            let y = bounds.origin.y + h * (1.0 - val / max_val * 0.85);
                            let pt = point(x, y);
                            if i == 0 {
                                path.move_to(pt);
                            } else {
                                path.line_to(pt);
                            }
                        }
                        path.build().ok()
                    },
                    move |_bounds, path_opt, window, _cx| {
                        if let Some(path) = path_opt {
                            window.paint_path(path, cpu_path_color);
                        }
                    },
                )
                .flex_1()
                .w_full(),
            );

        // --- MEM card: mem + swap ---
        let mem_card = v_flex()
            .min_w(card_min_w)
            .flex_1()
            .h_full()
            .px_1()
            .py_1()
            .gap_0p5()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .child(
                        div()
                            .text_size(rems(0.833))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(mem_color)
                            .child(t!("mem").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(rems(0.833))
                            .text_color(muted_fg)
                            .child(format!("{:.0}%", mem_pct * 100.0)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1()
                    .child(
                        Progress::new("mem-progress")
                            .value(mem_pct * 100.0)
                            .color(mem_color)
                            .with_size(px(5.))
                            .flex_1(),
                    )
                    .child(
                        div()
                            .text_size(rems(0.7))
                            .text_color(muted_fg)
                            .child(mem_detail),
                    ),
            )
            .when(self.system.total_swap > 0, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_1()
                        .child(
                            Progress::new("swap-progress")
                                .value(swap_pct * 100.0)
                                .color(swap_color)
                                .with_size(px(4.))
                                .flex_1(),
                        )
                        .child(
                            div()
                                .text_size(rems(0.7))
                                .text_color(muted_fg)
                                .child(swap_detail),
                        ),
                )
            });

        // --- NET card: rx/tx text + dual sparkline ---
        let net_card = if show_net_card {
            Some(
                v_flex()
                    .min_w(card_min_w)
                    .flex_1()
                    .h_full()
                    .px_1()
                    .py_1()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .child(
                                div()
                                    .text_size(rems(0.833))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(net_color)
                                    .child(t!("net").to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(rems(0.75))
                                            .text_color(net_color)
                                            .child(format!("↓{}", net_rx)),
                                    )
                                    .child(
                                        div()
                                            .text_size(rems(0.75))
                                            .text_color(net_tx_color)
                                            .child(format!("↑{}", net_tx)),
                                    ),
                            ),
                    )
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                let n_rx = net_rx_history.len();
                                let n_tx = net_tx_history.len();
                                if n_rx < 2 || n_tx < 2 {
                                    return None;
                                }
                                let all: Vec<f32> = net_rx_history
                                    .iter()
                                    .chain(net_tx_history.iter())
                                    .cloned()
                                    .collect();
                                let max_val = all.iter().cloned().fold(0.0f32, f32::max).max(1.0);
                                let w = bounds.size.width;
                                let h = bounds.size.height;
                                let mut paths = Vec::new();

                                let mut rx_path = PathBuilder::stroke(px(1.5));
                                for (i, &val) in net_rx_history.iter().enumerate() {
                                    let x =
                                        bounds.origin.x + w * i as f32 / (n_rx - 1).max(1) as f32;
                                    let y = bounds.origin.y + h * (1.0 - val / max_val * 0.85);
                                    let pt = point(x, y);
                                    if i == 0 {
                                        rx_path.move_to(pt);
                                    } else {
                                        rx_path.line_to(pt);
                                    }
                                }
                                if let Ok(path) = rx_path.build() {
                                    paths.push((path, net_color));
                                }

                                let mut tx_path = PathBuilder::stroke(px(1.0));
                                for (i, &val) in net_tx_history.iter().enumerate() {
                                    let x =
                                        bounds.origin.x + w * i as f32 / (n_tx - 1).max(1) as f32;
                                    let y = bounds.origin.y + h * (1.0 - val / max_val * 0.85);
                                    let pt = point(x, y);
                                    if i == 0 {
                                        tx_path.move_to(pt);
                                    } else {
                                        tx_path.line_to(pt);
                                    }
                                }
                                if let Ok(path) = tx_path.build() {
                                    paths.push((path, net_tx_color));
                                }

                                Some(paths)
                            },
                            move |_bounds, paths_opt, window, _cx| {
                                if let Some(paths) = paths_opt {
                                    for (path, color) in paths {
                                        window.paint_path(path, color);
                                    }
                                }
                            },
                        )
                        .flex_1()
                        .w_full(),
                    ),
            )
        } else {
            None
        };

        // --- DISK card ---
        let disk_card = if show_disk_card {
            Some(
                v_flex()
                    .min_w(card_min_w)
                    .flex_1()
                    .h_full()
                    .px_1()
                    .py_1()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .child(
                                div()
                                    .text_size(rems(0.833))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(disk_color)
                                    .child(t!("disk").to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(rems(0.833))
                                    .text_color(muted_fg)
                                    .child(format!("{:.0}%", disk_pct)),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .child(
                                v_flex()
                                    .id("disk-scroll")
                                    .track_scroll(&self.disk_scroll_handle)
                                    .overflow_y_scroll()
                                    .size_full()
                                    .children(disks.iter().map(|disk| {
                                        let pct = if disk.total_bytes > 0 {
                                            (disk.total_bytes - disk.available_bytes) as f64
                                                / disk.total_bytes as f64
                                                * 100.0
                                        } else {
                                            0.0
                                        };
                                        let mount_short = disk.mount.clone();
                                        let mount_id = format!("disk-{}", mount_short);
                                        h_flex()
                                            .w_full()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(rems(0.667))
                                                    .text_color(muted_fg)
                                                    .child(mount_short),
                                            )
                                            .child(
                                                Progress::new(mount_id)
                                                    .value(pct as f32)
                                                    .color(disk_color)
                                                    .with_size(px(4.))
                                                    .flex_1(),
                                            )
                                            .child(
                                                div()
                                                    .text_size(rems(0.667))
                                                    .text_color(muted_fg)
                                                    .child(format!("{:.0}%", pct)),
                                            )
                                    })),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(px(8.))
                                    .child(
                                        Scrollbar::vertical(&self.disk_scroll_handle)
                                            .scrollbar_show(ScrollbarShow::Scrolling),
                                    ),
                            )
                            .into_any_element(),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        let mut panel = h_flex()
            .h(px(80.))
            .w_full()
            .flex_none()
            .px_3()
            .gap_3()
            .border_b_1()
            .border_color(border_color)
            .bg(cx.theme().muted);

        panel = panel.child(cpu_card);
        panel = panel.child(mem_card);
        if let Some(card) = net_card {
            panel = panel.child(card);
        }
        if let Some(card) = disk_card {
            panel = panel.child(card);
        }
        panel
    }

    fn render_sidebar_monitoring_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cpu_pct = self.system.cpu_percent;
        let mem_pct = self.system.mem_percent;
        let swap_pct = self.system.swap_percent;

        let cpu_color = cx.theme().chart_1;
        let mem_color = cx.theme().chart_2;
        let swap_color = cx.theme().chart_3;
        let disk_color = cx.theme().chart_5;
        let net_color = cx.theme().chart_4;
        let muted_fg = cx.theme().muted_foreground;
        let _ = (swap_pct, swap_color);
        let disk_pct = self
            .system
            .disks
            .first()
            .map(|disk| {
                if disk.total_bytes == 0 {
                    0.0
                } else {
                    (disk.total_bytes - disk.available_bytes) as f32 / disk.total_bytes as f32
                }
            })
            .unwrap_or(0.0);

        v_flex()
            .w_full()
            .h(px(154.))
            .flex_none()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .px_3()
                    .py(px(7.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .text_color(muted_fg)
                                    .child(t!("cpu")),
                            )
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{:.0}%", cpu_pct * 100.0)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(3.))
                            .bg(cx.theme().muted)
                            .child(div().h_full().w(relative(cpu_pct)).bg(cpu_color)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .px_3()
                    .py(px(7.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .text_color(muted_fg)
                                    .child(t!("mem")),
                            )
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(self.system.mem_detail.clone()),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(3.))
                            .bg(cx.theme().muted)
                            .child(div().h_full().w(relative(mem_pct)).bg(mem_color)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .px_3()
                    .py(px(7.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .text_color(muted_fg)
                                    .child(t!("net")),
                            )
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(self.system.net_rx.clone()),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(3.))
                            .bg(cx.theme().muted)
                            .child(div().h_full().w(relative(0.56)).bg(net_color)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .px_3()
                    .py(px(7.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .text_color(muted_fg)
                                    .child(t!("disk")),
                            )
                            .child(
                                div()
                                    .text_size(rems(0.708))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{:.0}%", disk_pct * 100.0)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(3.))
                            .bg(cx.theme().muted)
                            .child(div().h_full().w(relative(disk_pct)).bg(disk_color)),
                    ),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sessions = self.config.sessions().to_vec();
        let session_folders = self.config.session_folders().to_vec();
        let session_folder_menu_entries = self.config.session_folders().to_vec();
        let session_folder_members: std::collections::HashMap<_, _> = session_folders
            .iter()
            .map(|folder| {
                (
                    folder.id.clone(),
                    self.config.session_folder_sessions(&folder.id),
                )
            })
            .collect();
        let active_session_id = self.active_session_id().map(ToOwned::to_owned);
        let connected_session_ids: std::collections::HashSet<_> = self
            .tabs
            .iter()
            .filter(|tab| tab.kind == crate::terminal::TabKind::Ssh && tab.connected)
            .filter_map(|tab| tab.session.as_ref().map(|session| session.id.clone()))
            .collect();

        v_flex()
            .gap_0()
            .w_full()
            .h_full()
            .min_w(px(0.))
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .overflow_hidden()
            .child(
                v_flex()
                    .min_w(px(0.))
                    .child(
                        h_flex()
                            .h(px(SIDEBAR_PRIMARY_ACTION_HEIGHT))
                            .px_3()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(rems(0.833))
                                    .text_color(cx.theme().primary)
                                    .child(t!("system").to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("sidebar-collapse-toggle")
                                    .ghost()
                                    .small()
                                    .icon(IconName::PanelLeftClose)
                                    .tooltip(t!("settings_toggle_sidebar").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sidebar_collapsed = true;
                                        this.config.set_sidebar_collapsed(true);
                                        this.save_preferences_background();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(self.config.monitoring_position() == "Sidebar", |this| {
                this.child(self.render_sidebar_monitoring_panel(cx))
            })
            .child(
                h_flex()
                    .id("open-ssh-panel")
                    .h(px(SIDEBAR_PRIMARY_ACTION_HEIGHT))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .cursor_pointer()
                    .hover(|this| this.bg(cx.theme().secondary_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_new_ssh_dialog(window, cx)
                        }),
                    )
                    .child(
                        div()
                            .text_size(rems(0.833))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("new_ssh_connection").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(COMPACT_ICON_SIZE))
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                v_flex()
                    .gap_0()
                    .pb_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .h(px(SIDEBAR_SECTION_HEIGHT))
                            .px_3()
                            .items_center()
                            .child(
                                div()
                                    .text_size(rems(0.833))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().primary)
                                    .child(t!("session_folders").to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("new-session-folder")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Plus)
                                    .tooltip(t!("new_session_folder").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_session_folder_dialog(None, None, window, cx);
                                    })),
                            ),
                    )
                    .when(!session_folders.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_0()
                                .max_h(px(180.))
                                .overflow_y_scrollbar()
                                .children(session_folders.into_iter().enumerate().map(
                                    |(index, folder)| {
                                        let folder_id = folder.id.clone();
                                        let toggle_folder_id = folder.id.clone();
                                        let edit_folder_id = folder.id.clone();
                                        let delete_folder_id = folder.id.clone();
                                        let folder_members = session_folder_members
                                            .get(&folder.id)
                                            .cloned()
                                            .unwrap_or_default();
                                        let is_collapsed = self
                                            .collapsed_session_folder_ids
                                            .contains(&folder.id);
                                        let member_count = folder.session_ids.len();
                                        div()
                                            .id(("session-folder", index))
                                            .w_full()
                                            .px_2()
                                            .bg(cx.theme().sidebar)
                                            .hover(|this| this.bg(cx.theme().secondary))
                                            .context_menu({
                                                let view = cx.entity();
                                                move |menu, window, _| {
                                                    let edit_value = edit_folder_id.clone();
                                                    let delete_value = delete_folder_id.clone();
                                                    menu.item(
                                                        PopupMenuItem::new(t!("edit").to_string())
                                                            .on_click(window.listener_for(
                                                                &view,
                                                                move |_this, _, window, cx| {
                                                                    let folder_id = edit_value.clone();
                                                                    cx.on_next_frame(
                                                                        window,
                                                                        move |this, window, cx| {
                                                                            this.show_session_folder_dialog(
                                                                                Some(folder_id.clone()),
                                                                                None,
                                                                                window,
                                                                                cx,
                                                                            );
                                                                        },
                                                                    );
                                                                },
                                                            )),
                                                    )
                                                    .item(
                                                        PopupMenuItem::new(
                                                            t!("delete").to_string(),
                                                        )
                                                        .on_click(window.listener_for(
                                                            &view,
                                                            move |this, _, _, cx| {
                                                                this.remove_session_folder(
                                                                    delete_value.clone(),
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                    )
                                                }
                                            })
                                            .child(
                                                h_flex()
                                                    .h(px(SIDEBAR_SECTION_HEIGHT))
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        h_flex()
                                                            .flex_1()
                                                            .min_w(px(0.))
                                                            .items_center()
                                                            .gap_2()
                                                            .cursor_pointer()
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(move |this, _, _, cx| {
                                                                    this.toggle_session_folder(
                                                                        &toggle_folder_id,
                                                                        cx,
                                                                    );
                                                                }),
                                                            )
                                                            .child(
                                                                Icon::new(if is_collapsed {
                                                                    IconName::ChevronRight
                                                                } else {
                                                                    IconName::ChevronDown
                                                                })
                                                                .size(px(12.))
                                                                .text_color(cx.theme().muted_foreground),
                                                            )
                                                            .child(
                                                                Icon::new(if is_collapsed {
                                                                    IconName::Folder
                                                                } else {
                                                                    IconName::FolderOpen
                                                                })
                                                                .size(px(COMPACT_ICON_SIZE))
                                                                .text_color(cx.theme().primary),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .min_w(px(0.))
                                                                    .text_size(rems(0.833))
                                                                    .font_weight(FontWeight::MEDIUM)
                                                                    .truncate()
                                                                    .child(folder.name),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(rems(0.708))
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(member_count.to_string()),
                                                    )
                                                    .child(
                                                        Button::new(format!(
                                                            "open-session-folder-{index}"
                                                        ))
                                                        .ghost()
                                                        .small()
                                                        .icon(IconName::Play)
                                                        .tooltip(
                                                            t!("open_folder_sessions").to_string(),
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.connect_session_folder(
                                                                    &folder_id, cx,
                                                                );
                                                            },
                                                        )),
                                                    ),
                                            )
                                            .when(!is_collapsed, |this| {
                                                this.child(
                                                    v_flex()
                                                        .gap_0()
                                                        .py_1()
                                                        .children(folder_members.into_iter().enumerate().map(
                                                            |(member_index, session)| {
                                                                let session_id = session.id.clone();
                                                                let is_active = active_session_id.as_deref()
                                                                    == Some(session.id.as_str());
                                                                let is_connected = connected_session_ids
                                                                    .contains(&session.id);
                                                                let detail = self.session_detail(&session);
                                                                div()
                                                                    .id(format!(
                                                                        "session-folder-member-{index}-{member_index}"
                                                                    ))
                                                                    .relative()
                                                                    .ml_2()
                                                                    .mr_1()
                                                                    .my(px(3.))
                                                                    .h(px(40.))
                                                                    .px_2()
                                                                    .rounded(px(5.))
                                                                    .cursor_pointer()
                                                                    .bg(if is_active {
                                                                        cx.theme().secondary
                                                                    } else {
                                                                        cx.theme().sidebar
                                                                    })
                                                                    .hover(|this| {
                                                                        this.bg(cx.theme().secondary_hover)
                                                                    })
                                                                    .on_mouse_down(
                                                                        MouseButton::Left,
                                                                        cx.listener(
                                                                            move |this, _, window, cx| {
                                                                                this.connect_saved_session(
                                                                                    session_id.clone(),
                                                                                    window,
                                                                                    cx,
                                                                                );
                                                                            },
                                                                        ),
                                                                    )
                                                                    .when(is_active || is_connected, |this| {
                                                                        let rail_height = if is_active {
                                                                            px(28.)
                                                                        } else {
                                                                            px(22.)
                                                                        };
                                                                        let rail_width = if is_active {
                                                                            px(3.)
                                                                        } else {
                                                                            px(2.)
                                                                        };
                                                                        let rail_top = if is_active {
                                                                            px(6.)
                                                                        } else {
                                                                            px(9.)
                                                                        };
                                                                        let rail_color = if is_active {
                                                                            cx.theme().primary
                                                                        } else {
                                                                            cx.theme().success.opacity(0.7)
                                                                        };
                                                                        this.child(
                                                                            Transition::new(
                                                                                std::time::Duration::from_millis(140),
                                                                            )
                                                                            .fade(0.25, 1.)
                                                                            .height(px(10.), rail_height)
                                                                            .apply(
                                                                                div()
                                                                                    .absolute()
                                                                                    .left_0()
                                                                                    .top(rail_top)
                                                                                    .w(rail_width)
                                                                                    .rounded(px(2.))
                                                                                    .bg(rail_color),
                                                                                format!(
                                                                                    "session-folder-member-rail-{index}-{member_index}-{is_active}"
                                                                                ),
                                                                            ),
                                                                        )
                                                                    })
                                                                    .child(
                                                                        h_flex()
                                                                            .size_full()
                                                                            .items_center()
                                                                            .gap_2()
                                                                            .child(
                                                                                div()
                                                                                    .flex_1()
                                                                                    .min_w(px(0.))
                                                                                    .text_size(rems(0.75))
                                                                                    .text_color(cx.theme().foreground)
                                                                                    .truncate()
                                                                                    .child(session.name),
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .text_size(rems(0.667))
                                                                                    .text_color(cx.theme().muted_foreground)
                                                                                    .truncate()
                                                                                    .child(detail),
                                                                            ),
                                                                    )
                                                            },
                                                        )),
                                                )
                                            })
                                    },
                                )),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .gap_0()
                    .px_2()
                    .child(
                        h_flex()
                            .h(px(SIDEBAR_SECTION_HEIGHT))
                            .items_center()
                            .child(
                                div()
                                    .text_size(rems(0.833))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().primary)
                                    .child(t!("saved")),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("connect-all-saved-sessions")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Play)
                                    .tooltip(t!("open_all_saved_sessions").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.connect_all_saved_sessions(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .size_full()
                            .child(
                                v_flex()
                                    .size_full()
                                    .id("saved-sessions-scroll")
                                    .track_scroll(&self.saved_scroll_handle)
                                    .overflow_y_scroll()
                                    .gap_1()
                                    .py_1()
                                    .children(sessions.into_iter().enumerate().map(
                                        |(ix, session)| {
                                            let connect_id = session.id.clone();
                                            let folder_session_id = session.id.clone();
                                            let edit_id = session.id.clone();
                                            let delete_id = session.id.clone();
                                            let is_active = active_session_id.as_deref()
                                                == Some(session.id.as_str());
                                            let is_connected = connected_session_ids
                                                .contains(&session.id);
                                            let name = session.name.clone();
                                            let detail = self.session_detail(&session);
                                            div()
                                                .id(("saved-connect", ix))
                                                .relative()
                                                .w_full()
                                                .h(px(40.))
                                                .px_2()
                                                .rounded(px(5.))
                                                .bg(if is_active {
                                                    cx.theme().secondary
                                                } else {
                                                    cx.theme().sidebar
                                                })
                                                .cursor_pointer()
                                                .hover(|this| this.bg(cx.theme().secondary_hover))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, window, cx| {
                                                        this.connect_saved_session(
                                                            connect_id.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    }),
                                                )
                                                .context_menu({
                                                    let view = cx.entity();
                                                    let folders = session_folder_menu_entries.clone();
                                                    move |menu, window, cx| {
                                                        let create_session_value =
                                                            folder_session_id.clone();
                                                        let copy_session_value =
                                                            folder_session_id.clone();
                                                        let move_session_value =
                                                            folder_session_id.clone();
                                                        let copy_folders = folders.clone();
                                                        let move_folders = folders.clone();
                                                        let edit_value = edit_id.clone();
                                                        let clone_value = edit_id.clone();
                                                        let delete_value = delete_id.clone();
                                                        let menu = menu.item(
                                                            PopupMenuItem::new(
                                                                t!("create_folder_from_session")
                                                                    .to_string(),
                                                            )
                                                            .on_click(window.listener_for(
                                                                &view,
                                                                move |this, _, window, cx| {
                                                                    this.show_session_folder_dialog(
                                                                        None,
                                                                        Some(create_session_value.clone()),
                                                                        window,
                                                                        cx,
                                                                    );
                                                                },
                                                            )),
                                                        );
                                                        let menu = if copy_folders.is_empty() {
                                                            menu.item(
                                                                PopupMenuItem::new(
                                                                    t!("copy_to_session_folder").to_string(),
                                                                )
                                                                .disabled(true),
                                                            )
                                                        } else {
                                                            menu.submenu(
                                                                t!("copy_to_session_folder").to_string(),
                                                                window,
                                                                cx,
                                                                {
                                                                    let view = view.clone();
                                                                    move |menu, window, _| {
                                                                        copy_folders.iter().fold(
                                                                            menu,
                                                                            |menu, folder| {
                                                                                let folder_id = folder.id.clone();
                                                                                let session_id = copy_session_value.clone();
                                                                                menu.item(
                                                                                    PopupMenuItem::new(folder.name.clone()).on_click(
                                                                                        window.listener_for(
                                                                                            &view,
                                                                                            move |this, _, window, cx| {
                                                                                                this.add_session_to_folder(
                                                                                                    session_id.clone(),
                                                                                                    folder_id.clone(),
                                                                                                    false,
                                                                                                    window,
                                                                                                    cx,
                                                                                                );
                                                                                            },
                                                                                        ),
                                                                                    ),
                                                                                )
                                                                            },
                                                                        )
                                                                    }
                                                                },
                                                            )
                                                        };
                                                        let menu = if move_folders.is_empty() {
                                                            menu.item(
                                                                PopupMenuItem::new(
                                                                    t!("move_to_session_folder").to_string(),
                                                                )
                                                                .disabled(true),
                                                            )
                                                        } else {
                                                            menu.submenu(
                                                                t!("move_to_session_folder").to_string(),
                                                                window,
                                                                cx,
                                                                {
                                                                    let view = view.clone();
                                                                    move |menu, window, _| {
                                                                        move_folders.iter().fold(
                                                                            menu,
                                                                            |menu, folder| {
                                                                                let folder_id = folder.id.clone();
                                                                                let session_id = move_session_value.clone();
                                                                                menu.item(
                                                                                    PopupMenuItem::new(folder.name.clone()).on_click(
                                                                                        window.listener_for(
                                                                                            &view,
                                                                                            move |this, _, window, cx| {
                                                                                                this.add_session_to_folder(
                                                                                                    session_id.clone(),
                                                                                                    folder_id.clone(),
                                                                                                    true,
                                                                                                    window,
                                                                                                    cx,
                                                                                                );
                                                                                            },
                                                                                        ),
                                                                                    ),
                                                                                )
                                                                            },
                                                                        )
                                                                    }
                                                                },
                                                            )
                                                        };
                                                        menu.item(
                                                            PopupMenuItem::new(
                                                                t!("clone").to_string(),
                                                            )
                                                            .on_click(window.listener_for(
                                                                &view,
                                                                move |this, _, window, cx| {
                                                                    this.clone_saved_session(
                                                                        clone_value.clone(),
                                                                        window,
                                                                        cx,
                                                                    )
                                                                },
                                                            )),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new(
                                                                t!("edit").to_string(),
                                                            )
                                                            .on_click(window.listener_for(
                                                                &view,
                                                                move |this, _, window, cx| {
                                                                    this.edit_saved_session(
                                                                        edit_value.clone(),
                                                                        window,
                                                                        cx,
                                                                    )
                                                                },
                                                            )),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new(
                                                                t!("delete").to_string(),
                                                            )
                                                            .on_click(window.listener_for(
                                                                &view,
                                                                move |this, _, _, cx| {
                                                                    this.remove_saved_session(
                                                                        delete_value.clone(),
                                                                        cx,
                                                                    )
                                                                },
                                                            )),
                                                        )
                                                    }
                                                })
                                                .when(is_active || is_connected, |this| {
                                                    let rail_height = if is_active {
                                                        px(28.)
                                                    } else {
                                                        px(22.)
                                                    };
                                                    let rail_width = if is_active {
                                                        px(3.)
                                                    } else {
                                                        px(2.)
                                                    };
                                                    let rail_top = if is_active {
                                                        px(6.)
                                                    } else {
                                                        px(9.)
                                                    };
                                                    let rail_color = if is_active {
                                                        cx.theme().primary
                                                    } else {
                                                        cx.theme().success.opacity(0.7)
                                                    };
                                                    this.child(
                                                        Transition::new(
                                                            std::time::Duration::from_millis(140),
                                                        )
                                                        .fade(0.25, 1.)
                                                        .height(px(10.), rail_height)
                                                        .apply(
                                                            div()
                                                                .absolute()
                                                                .left_0()
                                                                .top(rail_top)
                                                                .w(rail_width)
                                                                .rounded(px(2.))
                                                                .bg(rail_color),
                                                            format!(
                                                                "saved-session-rail-{ix}-{is_active}"
                                                            ),
                                                        ),
                                                    )
                                                })
                                                .child(
                                                    v_flex()
                                                        .gap_1()
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.875))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(cx.theme().foreground)
                                                                .child(name),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.75))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(detail),
                                                        ),
                                                )
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .right_0()
                                    .child(
                                        gpui_component::scroll::Scrollbar::new(
                                            &self.saved_scroll_handle,
                                        )
                                        .id("saved-scrollbar")
                                        .axis(gpui_component::scroll::ScrollbarAxis::Vertical)
                                        .scrollbar_show(
                                            gpui_component::scroll::ScrollbarShow::Always,
                                        ),
                                    ),
                            ),
                    ),
            )
    }

    fn render_activity_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("workspace-activity-rail")
            .w(px(COLLAPSED_SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .items_center()
            .gap_1()
            .py_2()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .child(
                Button::new("activity-rail-sessions")
                    .ghost()
                    .small()
                    .icon(IconName::PanelLeftOpen)
                    .tooltip(t!("settings_toggle_sidebar").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_collapsed = false;
                        this.config.set_sidebar_collapsed(false);
                        this.save_preferences_background();
                        cx.notify();
                    })),
            )
    }

    #[cfg(any())]
    fn render_window_controls(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_macos = cfg!(target_os = "macos");
        let is_fullscreen = window.is_fullscreen();

        let is_active = cx.active_window() == Some(window.window_handle());

        h_flex()
            .group("window-controls")
            .flex_none()
            .items_center()
            .px_3()
            .gap_2()
            .when(!is_macos || is_fullscreen, |this| {
                this.child(
                    h_flex()
                        .id("window-close")
                        .size(px(12.))
                        .rounded_full()
                        .bg(if is_active {
                            hsla(3.0 / 360.0, 1.0, 0.67, 1.0)
                        } else {
                            hsla(0.0, 0.0, 0.8, 1.0)
                        }) // Red or Inactive Grey
                        .group_hover("window-controls", |s| {
                            s.bg(hsla(3.0 / 360.0, 1.0, 0.67, 1.0))
                        })
                        .when(!is_macos, |this| {
                            this.window_control_area(gpui::WindowControlArea::Close)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.request_application_close(window, cx);
                        }))
                        .hover(|s| s.bg(hsla(3.0 / 360.0, 1.0, 0.55, 1.0)))
                        .active(|s| s.bg(hsla(3.0 / 360.0, 1.0, 0.45, 1.0)))
                        .items_center()
                        .justify_center()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(7.))
                                .font_weight(FontWeight::BOLD)
                                .line_height(relative(1.0))
                                .text_color(hsla(3.0 / 360.0, 1.0, 0.15, 0.7))
                                .opacity(0.0)
                                .group_hover("window-controls", |s| s.opacity(1.0))
                                .child("✕"),
                        ),
                )
                .child(
                    h_flex()
                        .id("window-minimize")
                        .size(px(12.))
                        .rounded_full()
                        .bg(if is_active {
                            hsla(39.0 / 360.0, 1.0, 0.59, 1.0)
                        } else {
                            hsla(0.0, 0.0, 0.8, 1.0)
                        }) // Yellow or Inactive Grey
                        .group_hover("window-controls", |s| {
                            s.bg(hsla(39.0 / 360.0, 1.0, 0.59, 1.0))
                        })
                        .when(!is_macos, |this| {
                            this.window_control_area(gpui::WindowControlArea::Min)
                        })
                        .on_click(|_, window, _| window.minimize_window())
                        .hover(|s| s.bg(hsla(39.0 / 360.0, 1.0, 0.49, 1.0)))
                        .active(|s| s.bg(hsla(39.0 / 360.0, 1.0, 0.39, 1.0)))
                        .items_center()
                        .justify_center()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(7.))
                                .font_weight(FontWeight::BOLD)
                                .line_height(relative(1.0))
                                .text_color(hsla(39.0 / 360.0, 1.0, 0.15, 0.8))
                                .opacity(0.0)
                                .group_hover("window-controls", |s| s.opacity(1.0))
                                .child("−"),
                        ),
                )
                .child(
                    h_flex()
                        .id("window-maximize")
                        .size(px(12.))
                        .rounded_full()
                        .bg(if is_active {
                            hsla(127.0 / 360.0, 0.68, 0.47, 1.0)
                        } else {
                            hsla(0.0, 0.0, 0.8, 1.0)
                        }) // Green or Inactive Grey
                        .group_hover("window-controls", |s| {
                            s.bg(hsla(127.0 / 360.0, 0.68, 0.47, 1.0))
                        })
                        .when(!is_macos, |this| {
                            this.window_control_area(gpui::WindowControlArea::Max)
                        })
                        .on_click(|_, window, _| {
                            if window.is_fullscreen() {
                                window.toggle_fullscreen();
                            } else {
                                #[cfg(target_os = "macos")]
                                window.titlebar_double_click();
                                #[cfg(not(target_os = "macos"))]
                                window.zoom_window();
                            }
                        })
                        .hover(|s| s.bg(hsla(127.0 / 360.0, 0.68, 0.37, 1.0)))
                        .active(|s| s.bg(hsla(127.0 / 360.0, 0.68, 0.27, 1.0)))
                        .items_center()
                        .justify_center()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(7.))
                                .font_weight(FontWeight::BOLD)
                                .line_height(relative(1.0))
                                .text_color(hsla(127.0 / 360.0, 1.0, 0.15, 0.8))
                                .opacity(0.0)
                                .group_hover("window-controls", |s| s.opacity(1.0))
                                .child("+"),
                        ),
                )
            })
            .when(is_macos, |this| {
                this.when(!is_fullscreen, |this| this.w(px(80.)))
            })
    }

    fn render_tab_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs_data: Vec<WorkspaceTabRenderData> = self
            .workspace_tabs
            .iter()
            .filter_map(|workspace| match workspace {
                crate::document::WorkspaceTab::Session {
                    id: workspace_id,
                    group_id,
                } => {
                    let group = self.tab_groups.iter().find(|group| &group.id == group_id)?;
                    let pane_ids: Vec<String> = group
                        .pane_root
                        .tab_ids()
                        .iter()
                        .map(|id| id.to_string())
                        .collect();
                    let label = if pane_ids.len() > 1 {
                        format!("{} ({})", group.title, pane_ids.len())
                    } else {
                        group.title.clone()
                    };
                    let close_id = if self.active_group.as_ref() == Some(group_id) {
                        self.active_tab
                            .clone()
                            .or_else(|| pane_ids.first().cloned())
                            .unwrap_or_default()
                    } else {
                        pane_ids.first().cloned().unwrap_or_default()
                    };
                    let status = aggregate_terminal_workspace_status(
                        &pane_ids
                            .iter()
                            .map(|id| {
                                self.tabs.iter().find(|tab| tab.id == *id).map(|tab| {
                                    terminal_tab_status(
                                        tab.connected,
                                        tab.disconnected_reason.as_deref(),
                                    )
                                })
                            })
                            .collect::<Vec<_>>(),
                    );
                    let connected_ssh = pane_ids.iter().any(|id| {
                        self.tabs.iter().any(|tab| {
                            tab.id == *id
                                && tab.kind == crate::terminal::TabKind::Ssh
                                && tab.connected
                        })
                    });
                    Some(WorkspaceTabRenderData {
                        workspace_id: workspace_id.clone(),
                        label,
                        visual_kind: WorkspaceTabVisualKind::Terminal,
                        status,
                        dirty: false,
                        connected_ssh,
                        close_target: WorkspaceCloseTarget::Session(close_id),
                    })
                }
                crate::document::WorkspaceTab::RemoteDocument {
                    id: workspace_id,
                    document_id,
                } => {
                    let document = self.documents.get(document_id)?;
                    let dirty = document.revisions.is_dirty();
                    let status = document_workspace_status(
                        &document.load_state,
                        &document.save_state,
                        &document.connection_state,
                        dirty,
                        document
                            .large_file
                            .as_ref()
                            .is_some_and(|large_file| large_file.loading),
                        document
                            .large_file
                            .as_ref()
                            .is_some_and(|large_file| large_file.error.is_some()),
                    );
                    Some(WorkspaceTabRenderData {
                        workspace_id: workspace_id.clone(),
                        label: document.title().to_string(),
                        visual_kind: WorkspaceTabVisualKind::RemoteDocument,
                        status,
                        dirty,
                        connected_ssh: false,
                        close_target: WorkspaceCloseTarget::Document(document_id.clone()),
                    })
                }
            })
            .collect();
        let tab_focus_handles = tabs_data
            .iter()
            .map(|tab| {
                window
                    .use_keyed_state(
                        (
                            ElementId::from("workspace-tab-focus"),
                            tab.workspace_id.clone(),
                        ),
                        cx,
                        |_, cx| cx.focus_handle(),
                    )
                    .read(cx)
                    .clone()
            })
            .collect::<Vec<_>>();
        let selected = self
            .active_workspace_tab
            .as_ref()
            .and_then(|active| {
                tabs_data
                    .iter()
                    .position(|workspace| &workspace.workspace_id == active)
            })
            .unwrap_or(0);
        let has_active_document = self.active_document_id().is_some();
        let has_active_terminal = self.active_tab.is_some();
        h_flex()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .overflow_x_hidden()
                    .child({
                        h_flex()
                            .id("ashell-tab-bar")
                            .tab_group()
                            .role(Role::TabList)
                            .aria_label(t!("workspace_tabs").to_string())
                            .w_full()
                            .h_full()
                            .items_end()
                            .gap_1()
                            .track_scroll(&self.tabs_scroll_handle)
                            .overflow_x_scroll()
                            .children(tabs_data.iter().enumerate().map(|(ix, tab_data)| {
                                let workspace_id = tab_data.workspace_id.clone();
                                let click_workspace_id = workspace_id.clone();
                                let close_target = tab_data.close_target.clone();
                                let is_selected = ix == selected;
                                let connected_ssh = tab_data.connected_ssh;
                                let tab_focus_handle = tab_focus_handles[ix].clone();
                                let previous_ix =
                                    if ix == 0 { tabs_data.len() - 1 } else { ix - 1 };
                                let next_ix = (ix + 1) % tabs_data.len();
                                let keyboard_current = (workspace_id, tab_focus_handle.clone());
                                let keyboard_previous = (
                                    tabs_data[previous_ix].workspace_id.clone(),
                                    tab_focus_handles[previous_ix].clone(),
                                );
                                let keyboard_next = (
                                    tabs_data[next_ix].workspace_id.clone(),
                                    tab_focus_handles[next_ix].clone(),
                                );
                                let keyboard_first = (
                                    tabs_data[0].workspace_id.clone(),
                                    tab_focus_handles[0].clone(),
                                );
                                let keyboard_last_ix = tabs_data.len() - 1;
                                let keyboard_last = (
                                    tabs_data[keyboard_last_ix].workspace_id.clone(),
                                    tab_focus_handles[keyboard_last_ix].clone(),
                                );
                                let kind_label = match tab_data.visual_kind {
                                    WorkspaceTabVisualKind::Terminal => {
                                        t!("workspace_tab_terminal").to_string()
                                    }
                                    WorkspaceTabVisualKind::RemoteDocument => {
                                        t!("workspace_tab_remote_file").to_string()
                                    }
                                };
                                let status_label = match tab_data.status {
                                    WorkspaceTabStatus::Normal => {
                                        t!("workspace_tab_status_normal").to_string()
                                    }
                                    WorkspaceTabStatus::Attention => {
                                        t!("workspace_tab_status_attention").to_string()
                                    }
                                    WorkspaceTabStatus::Error => {
                                        t!("workspace_tab_status_error").to_string()
                                    }
                                    WorkspaceTabStatus::Unavailable => {
                                        t!("workspace_tab_status_unavailable").to_string()
                                    }
                                };
                                let accessibility_label = workspace_tab_accessibility_label(
                                    &tab_data.label,
                                    &kind_label,
                                    &status_label,
                                    tab_data.dirty,
                                    &t!("document_unsaved"),
                                );
                                let background = if connected_ssh {
                                    gpui::black()
                                } else if is_selected {
                                    cx.theme().tab_active
                                } else {
                                    cx.theme().tab_bar
                                };
                                let color_role =
                                    workspace_tab_color_role(tab_data.visual_kind, tab_data.status);
                                let palette = crate::app::theme::workspace_tab_palette(cx.theme());
                                let accent = crate::app::theme::workspace_tab_accent(
                                    palette[match color_role {
                                        WorkspaceTabColorRole::Success => 0,
                                        WorkspaceTabColorRole::Blue => 1,
                                        WorkspaceTabColorRole::Warning => 2,
                                        WorkspaceTabColorRole::Danger => 3,
                                        WorkspaceTabColorRole::MutedForeground => 4,
                                    }],
                                    background,
                                );
                                let dirty_accent = crate::app::theme::workspace_tab_accent(
                                    cx.theme().warning,
                                    background,
                                );
                                let selection_accent = crate::app::theme::workspace_tab_accent(
                                    cx.theme().primary,
                                    background,
                                );
                                let icon = match tab_data.visual_kind {
                                    WorkspaceTabVisualKind::Terminal => IconName::SquareTerminal,
                                    WorkspaceTabVisualKind::RemoteDocument => IconName::File,
                                };
                                let foreground = if connected_ssh && is_selected {
                                    gpui::white()
                                } else if connected_ssh {
                                    cx.theme().muted_foreground
                                } else if is_selected {
                                    cx.theme().tab_active_foreground
                                } else {
                                    cx.theme().tab_foreground
                                };
                                h_flex()
                                    .id((
                                        ElementId::from("workspace-tab"),
                                        tab_data.workspace_id.clone(),
                                    ))
                                    .track_focus(
                                        &tab_focus_handle
                                            .clone()
                                            .tab_index(0)
                                            .tab_stop(is_selected),
                                    )
                                    .focus_visible(|style| {
                                        style.border_1().border_color(selection_accent)
                                    })
                                    .role(Role::Tab)
                                    .aria_label(accessibility_label)
                                    .aria_selected(is_selected)
                                    .flex_none()
                                    .relative()
                                    .min_w(px(104.))
                                    .max_w(px(240.))
                                    .h(px(32.))
                                    .pr_2()
                                    .gap_2()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded_tl(px(6.))
                                    .rounded_tr(px(6.))
                                    .bg(if connected_ssh || is_selected {
                                        background
                                    } else {
                                        cx.theme().transparent
                                    })
                                    .text_color(foreground)
                                    .cursor_pointer()
                                    .when(!is_selected && !connected_ssh, |this| {
                                        this.hover(|this| this.bg(cx.theme().secondary_hover))
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.activate_workspace(
                                            click_workspace_id.clone(),
                                            window,
                                            cx,
                                        )
                                    }))
                                    .on_key_down(cx.listener(
                                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                                            if !keyboard_current.1.is_focused(window) {
                                                return;
                                            }
                                            let modifiers = &event.keystroke.modifiers;
                                            let Some(action) = workspace_tab_keyboard_action(
                                                event.keystroke.key.as_str(),
                                                modifiers.control
                                                    || modifiers.alt
                                                    || modifiers.shift
                                                    || modifiers.platform
                                                    || modifiers.function,
                                            ) else {
                                                return;
                                            };
                                            let target = match action {
                                                WorkspaceTabKeyboardAction::Current => {
                                                    &keyboard_current
                                                }
                                                WorkspaceTabKeyboardAction::Previous => {
                                                    &keyboard_previous
                                                }
                                                WorkspaceTabKeyboardAction::Next => &keyboard_next,
                                                WorkspaceTabKeyboardAction::First => {
                                                    &keyboard_first
                                                }
                                                WorkspaceTabKeyboardAction::Last => &keyboard_last,
                                            };
                                            window.prevent_default();
                                            cx.stop_propagation();
                                            this.activate_workspace(target.0.clone(), window, cx);
                                            target.1.focus(window, cx);
                                        },
                                    ))
                                    .child(div().flex_none().w(px(3.)).h(px(24.)).bg(accent))
                                    .child(Icon::new(icon).size(px(14.)).text_color(foreground))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_size(rems(0.833))
                                            .when(is_selected, |this| {
                                                this.font_weight(FontWeight::SEMIBOLD)
                                            })
                                            .child(tab_data.label.clone()),
                                    )
                                    .when(tab_data.dirty, |this| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .size(px(6.))
                                                .rounded_full()
                                                .bg(dirty_accent),
                                        )
                                    })
                                    .child(
                                        Button::new((
                                            ElementId::from("tab-close"),
                                            tab_data.workspace_id.clone(),
                                        ))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .text_color(foreground)
                                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                            window.prevent_default();
                                            cx.stop_propagation();
                                        })
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                                match &close_target {
                                                    WorkspaceCloseTarget::Session(close_id)
                                                        if !close_id.is_empty() =>
                                                    {
                                                        this.close_tab(close_id.clone(), window, cx)
                                                    }
                                                    WorkspaceCloseTarget::Document(document_id) => {
                                                        this.request_close_document(
                                                            document_id.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    }
                                                    _ => {}
                                                }
                                            }),
                                        ),
                                    )
                                    .when(connected_ssh && is_selected, |this| {
                                        this.child(
                                            div()
                                                .absolute()
                                                .left(px(6.))
                                                .right(px(6.))
                                                .bottom_0()
                                                .h(px(2.))
                                                .rounded_tl(px(2.))
                                                .rounded_tr(px(2.))
                                                .bg(selection_accent),
                                        )
                                    })
                            }))
                            .child(div().flex_none().w_3().h_full())
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .pr(px(6.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("open-selector")
                            .secondary()
                            .small()
                            .rounded(px(999.))
                            .icon(IconName::Plus)
                            .tooltip(t!("settings_open_session").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_selector_dialog(window, cx)
                            })),
                    )
                    .when(has_active_terminal && !has_active_document, |this| {
                        this.child(
                            Button::new("split-horizontal")
                                .secondary()
                                .small()
                                .rounded(px(999.))
                                .icon(IconName::PanelBottom)
                                .tooltip(t!("settings_split_pane_down").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.split_current_pane("down", cx);
                                })),
                        )
                        .child(
                            Button::new("split-vertical")
                                .secondary()
                                .small()
                                .rounded(px(999.))
                                .icon(IconName::PanelRight)
                                .tooltip(t!("settings_split_pane_right").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.split_current_pane("right", cx);
                                })),
                        )
                        .child(self.render_search_button(cx))
                    }),
            )
    }

    fn render_windows_window_controls(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("windows-window-controls")
            .flex_none()
            .h_full()
            .gap_0()
            .child(
                div()
                    .id("windows-window-minimize")
                    .w(px(44.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().foreground)
                    .window_control_area(gpui::WindowControlArea::Min)
                    .on_click(|_, window, _| window.minimize_window())
                    .hover(|this| this.bg(cx.theme().accent))
                    .child(Icon::new(IconName::WindowMinimize).size(px(COMPACT_ICON_SIZE))),
            )
            .child(
                div()
                    .id("windows-window-maximize")
                    .w(px(44.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().foreground)
                    .window_control_area(gpui::WindowControlArea::Max)
                    .on_click(|_, window, _| window.zoom_window())
                    .hover(|this| this.bg(cx.theme().accent))
                    .child(Icon::new(IconName::WindowMaximize).size(px(COMPACT_ICON_SIZE))),
            )
            .child(
                div()
                    .id("windows-window-close")
                    .w(px(48.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().foreground)
                    .window_control_area(gpui::WindowControlArea::Close)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.request_application_close(window, cx);
                    }))
                    .hover(|this| {
                        this.bg(cx.theme().danger)
                            .text_color(cx.theme().primary_foreground)
                    })
                    .child(Icon::new(IconName::WindowClose).size(px(COMPACT_ICON_SIZE))),
            )
    }

    fn render_windows_title_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("windows-title-bar")
            .flex_none()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .w_full()
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .text_color(cx.theme().foreground)
                    .child(Icon::new(IconName::SquareTerminal).size(px(16.)))
                    .child(div().font_weight(FontWeight::MEDIUM).child("JShell")),
            )
            .child(
                div()
                    .id("windows-title-bar-drag")
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .overflow_x_hidden()
                    .child(self.render_tab_bar(window, cx)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h_full()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .child(
                        Button::new("windows-title-settings")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .tooltip(t!("settings_open_settings").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_settings_dialog(window, cx);
                            })),
                    ),
            )
            .child(self.render_windows_window_controls(window, cx))
    }

    fn render_terminal_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.terminal_cell_width = crate::app::theme::measure_terminal_cell_width(
            self.terminal_font_family.clone(),
            self.terminal_font_size,
            window,
        );
        let has_active = self.pane_root.leaf_count() > 0;
        let focused_is_terminal = matches!(
            self.pane_root.focused_leaf(&self.focused_pane_path),
            Some(PaneLeaf::Terminal(_))
        );
        let pane_tree = self.pane_root.clone();
        let view = cx.entity();

        div()
            .size_full()
            .relative()
            .px(px(TERMINAL_PADDING_X))
            .py(px(TERMINAL_PADDING_Y))
            .child(
                v_flex()
                    .size_full()
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .on_prepaint(move |bounds, _window, cx| {
                                view.update(cx, |this, cx| {
                                    if this.terminal_panel_bounds != Some(bounds) {
                                        this.terminal_panel_bounds = Some(bounds);
                                        cx.notify();
                                    }
                                });
                            })
                            .child(if has_active {
                                Self::render_pane_tree(self, &pane_tree, &[], window, cx)
                                    .into_any_element()
                            } else {
                                self.render_home_page(cx).into_any_element()
                            }),
                    )
                    .when(
                        has_active && focused_is_terminal && self.command_bar_open,
                        |this| this.child(self.render_command_bar(window, cx)),
                    ),
            )
            // Search bar overlay — only when search is active.
            .when(self.search_active && focused_is_terminal, |el| {
                el.child(self.render_search_bar(window, cx))
            })
            .when(
                self.command_history_panel_open && focused_is_terminal,
                |el| el.child(self.render_command_history_panel(window, cx)),
            )
            .when(
                focused_is_terminal && !self.search_active && !self.command_history_panel_open,
                |el| el.child(self.render_terminal_completion(cx)),
            )
    }

    fn render_pane_tree(
        this: &mut Ashell,
        layout: &PaneLayout,
        path: &[usize],
        window: &mut Window,
        cx: &mut Context<Ashell>,
    ) -> impl IntoElement {
        match layout {
            PaneLayout::Leaf(PaneLeaf::Empty) => this.render_home_page(cx).into_any_element(),
            PaneLayout::Leaf(PaneLeaf::Document(document_id)) => {
                let document_id_for_focus = document_id.clone();
                let focus_path = path.to_vec();
                let is_focused = path == this.focused_pane_path.as_slice();
                let is_detached = this.is_document_detached(document_id);
                let document_title = this
                    .documents
                    .get(document_id)
                    .map(|document| document.title().to_string())
                    .unwrap_or_else(|| document_id.clone());
                let detached_is_opening = this.detaching_document_ids.contains(document_id);
                let editor = if is_detached {
                    let document_id_for_button = document_id.clone();
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .bg(cx.theme().background)
                        .child(
                            div()
                                .font_family(this.terminal_font_family.clone())
                                .text_size(rems(1.0))
                                .child(document_title),
                        )
                        .child(div().text_color(cx.theme().muted_foreground).child(
                            if detached_is_opening {
                                t!("document_detach_opening").to_string()
                            } else {
                                t!("document_detached").to_string()
                            },
                        ))
                        .child(
                            Button::new(format!("document-detached-focus-{document_id}"))
                                .secondary()
                                .icon(IconName::ExternalLink)
                                .label(t!("document_detached").to_string())
                                .disabled(detached_is_opening)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.activate_detached_document(&document_id_for_button, cx);
                                })),
                        )
                        .into_any_element()
                } else {
                    crate::document::ui::render_document(this, document_id, window, cx)
                };
                div()
                    .size_full()
                    .overflow_hidden()
                    .when(!is_focused, |element| element.opacity(0.92))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.focus_pane_path(focus_path.clone());
                            if this.activate_detached_document(&document_id_for_focus, cx) {
                                cx.notify();
                                return;
                            }
                            if let Some(document) = this.documents.get(&document_id_for_focus) {
                                document.editor.focus_handle(cx).focus(window, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(editor)
                    .into_any_element()
            }
            PaneLayout::Leaf(PaneLeaf::Terminal(tab_id)) => {
                let is_focused = path == this.focused_pane_path.as_slice();
                let keyword_highlight = this.config.keyword_highlight();
                let snapshot = this
                    .tabs
                    .iter()
                    .find(|t| &t.id == tab_id)
                    .map(|t| t.render_snapshot(keyword_highlight));
                let Some(snapshot) = snapshot else {
                    return div().into_any_element();
                };
                let tab_id_clone2 = tab_id.clone();
                let tab_id_for_scroll = tab_id.clone();
                let focus_handle = this.focus_handle.clone();
                let marked_text = if is_focused {
                    this.terminal_marked_text.clone()
                } else {
                    None
                };
                let font_family = this.terminal_font_family.clone();
                let font_size = px(this.terminal_font_size);
                let line_height = px(this.terminal_line_height());
                let cell_width = px(this.terminal_cell_width());
                let is_url_hovered = this
                    .hovered_url
                    .as_ref()
                    .is_some_and(|hu| hu.tab_id == *tab_id && this.terminal_link_ctrl_pressed);
                let mut el = div()
                    .size_full()
                    .overflow_hidden()
                    .pr(px(TERMINAL_SCROLLBAR_GUTTER))
                    .track_focus(&this.focus_handle)
                    .key_context(TERMINAL_KEY_CONTEXT)
                    .when(is_url_hovered, |d| d.cursor_pointer())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.focus_pane_with_id(tab_id_clone2.clone());
                            this.focus_terminal(event, window, cx);
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(Self::on_terminal_right_click),
                    )
                    .on_mouse_move(cx.listener(Self::on_terminal_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_terminal_mouse_up))
                    .on_key_down(cx.listener(Self::on_terminal_key_down))
                    .on_key_up(cx.listener(Self::on_terminal_key_up))
                    .on_action(cx.listener(Self::on_terminal_tab_action))
                    .on_action(cx.listener(Self::on_terminal_backtab_action))
                    .on_scroll_wheel(cx.listener(move |this, event, window, cx| {
                        this.on_terminal_scroll(&tab_id_for_scroll, event, window, cx);
                    }))
                    .child(terminal::element::TerminalElement::new(
                        terminal::element::TerminalElementOptions {
                            view: cx.entity(),
                            focus_handle,
                            snapshot,
                            marked_text,
                            font_family,
                            font_size,
                            line_height,
                            cell_width,
                            tab_id: tab_id.to_string(),
                            search_highlights: this.search_highlight_map(
                                tab_id,
                                cx.theme().danger.opacity(0.35),
                                cx.theme().danger.opacity(0.70),
                            ),
                        },
                    ));
                let scrollbar = this.terminal_scrollbars.entry(tab_id.clone()).or_default();
                el = el.vertical_scrollbar(scrollbar);

                // When disconnected, overlay a reconnect bar at the bottom of the terminal.
                // Uses absolute positioning so the terminal element itself is unchanged,
                // keeping panel size stable in multi-panel layouts.
                let disconnected_reason = this
                    .tabs
                    .iter()
                    .find(|t| t.id == *tab_id)
                    .and_then(|tab| tab.disconnected_reason.clone());
                if let Some(reason) = disconnected_reason {
                    let tab_id_for_bar_reconnect = tab_id.clone();
                    let tab_id_for_button_reconnect = tab_id.clone();
                    let reconnect_button_id = format!("terminal-reconnect-{tab_id}");
                    el = div().size_full().relative().child(el).child(
                        div().absolute().bottom_0().left_0().right_0().child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_1()
                                .bg(cx.theme().danger.opacity(0.15))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_size(rems(0.85))
                                        .text_color(cx.theme().danger)
                                        .child(
                                            t!("session_disconnected", "reason" = reason)
                                                .to_string(),
                                        ),
                                )
                                .child(
                                    div().flex_none().child(
                                        Button::new(reconnect_button_id)
                                            .secondary()
                                            .small()
                                            .icon(IconName::Redo2)
                                            .label(t!("reconnect").to_string())
                                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.retry_disconnected_tab(
                                                    &tab_id_for_button_reconnect,
                                                    cx,
                                                );
                                            })),
                                    ),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.retry_disconnected_tab(&tab_id_for_bar_reconnect, cx);
                                    }),
                                ),
                        ),
                    );
                }
                let indicator_color = this
                    .tabs
                    .iter()
                    .find(|t| t.id == *tab_id)
                    .map(|tab| {
                        if tab.connected {
                            cx.theme().success
                        } else {
                            cx.theme().danger
                        }
                    })
                    .unwrap_or(cx.theme().success);
                let has_multiple_panes = this.pane_root.tab_ids().len() > 1;

                if !is_focused {
                    el = el.opacity(0.85);
                }

                let mut wrapper = div().size_full();
                if has_multiple_panes {
                    if is_focused {
                        wrapper = wrapper
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .left(px(1.))
                                    .right(px(1.))
                                    .h(px(1.))
                                    .bg(indicator_color),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(1.))
                                    .left(px(1.))
                                    .right(px(1.))
                                    .h(px(1.))
                                    .bg(indicator_color),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(1.))
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .w(px(1.))
                                    .bg(indicator_color),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(1.))
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .w(px(1.))
                                    .bg(indicator_color),
                            )
                            .p(px(4.))
                            .child(el);
                    } else {
                        wrapper = wrapper.p(px(4.)).child(el);
                    }
                } else {
                    wrapper = wrapper.child(el);
                }

                wrapper.into_any_element()
            }
            PaneLayout::Horizontal(children, ratio) => {
                v_flex()
                    .size_full()
                    .children(children.iter().enumerate().flat_map(|(i, child)| {
                        let mut items: Vec<gpui::AnyElement> = Vec::new();
                        if i > 0 {
                            let splitter_path = path.to_vec(); // path to the CONTAINER that has the ratio
                            items.push(
                                div()
                                    .h(px(4.))
                                    .w_full()
                                    .flex_none()
                                    .cursor_row_resize()
                                    .bg(cx.theme().border)
                                    .hover(|s| s.bg(cx.theme().accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, event, window, cx| {
                                            window.prevent_default();
                                            cx.stop_propagation();
                                            this.start_drag_split(
                                                splitter_path.clone(),
                                                i,
                                                event,
                                                window,
                                                cx,
                                            );
                                        }),
                                    )
                                    .into_any_element(),
                            );
                        }
                        let mut child_path = path.to_vec();
                        child_path.push(i);
                        items.push(
                            div()
                                .flex_grow(if children.len() == 2 {
                                    if i == 0 { *ratio } else { 1.0 - *ratio }
                                } else {
                                    1.0
                                })
                                .min_h(px(0.))
                                .overflow_hidden()
                                .child(Self::render_pane_tree(this, child, &child_path, window, cx))
                                .into_any_element(),
                        );
                        items
                    }))
                    .into_any_element()
            }
            PaneLayout::Vertical(children, ratio) => h_flex()
                .items_stretch()
                .size_full()
                .children(children.iter().enumerate().flat_map(|(i, child)| {
                    let mut items: Vec<gpui::AnyElement> = Vec::new();
                    if i > 0 {
                        let splitter_path = path.to_vec(); // path to the CONTAINER that has the ratio
                        items.push(
                            div()
                                .w(px(4.))
                                .h_full()
                                .flex_none()
                                .cursor_col_resize()
                                .bg(cx.theme().border)
                                .hover(|s| s.bg(cx.theme().accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        this.start_drag_split(
                                            splitter_path.clone(),
                                            i,
                                            event,
                                            window,
                                            cx,
                                        );
                                    }),
                                )
                                .into_any_element(),
                        );
                    }
                    let mut child_path = path.to_vec();
                    child_path.push(i);
                    items.push(
                        div()
                            .flex_grow(if children.len() == 2 {
                                if i == 0 { *ratio } else { 1.0 - *ratio }
                            } else {
                                1.0
                            })
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(Self::render_pane_tree(this, child, &child_path, window, cx))
                            .into_any_element(),
                    );
                    items
                }))
                .into_any_element(),
        }
    }

    fn render_terminal_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let monitoring_contents = v_flex()
            .size_full()
            .when(self.config.monitoring_position() == "Bottom", |this| {
                this.child(self.render_monitoring_panel(window.viewport_size().width, cx))
            })
            .child(self.render_sftp_panel(window, cx));

        let is_monitor_bottom = self.config.monitoring_position() == "Bottom";
        let minimized_height = if is_monitor_bottom { 104. } else { 24. };
        let min_panel_height = if is_monitor_bottom { 220. } else { 130. };
        let default_panel_height = if is_monitor_bottom { 304. } else { 154. };
        let sftp_size = if self.sftp_panel_minimized {
            px(minimized_height)
        } else {
            px(self
                .config
                .body_panels()
                .and_then(|sizes| sizes.get(1).copied())
                .unwrap_or(default_panel_height))
        };
        let body_panel = v_resizable("ashell-body")
            .lock(self.config.lock_layout())
            .with_state(&self.body_panels)
            .child(resizable_panel().child(self.render_terminal_panel(window, cx)))
            .child(
                resizable_panel()
                    .size(sftp_size)
                    .size_range(if self.sftp_panel_minimized {
                        px(minimized_height)..px(minimized_height)
                    } else {
                        px(min_panel_height)..px(1200.)
                    })
                    .child(monitoring_contents),
            )
            .into_any_element();

        if self.sidebar_collapsed {
            h_flex()
                .size_full()
                .child(self.render_activity_rail(cx))
                .child(
                    div().flex_1().h_full().min_w(px(0.)).child(
                        v_flex()
                            .size_full()
                            .relative()
                            .overflow_hidden()
                            .child(body_panel),
                    ),
                )
                .into_any_element()
        } else {
            let sidebar_area = resizable_panel()
                .size(px(self
                    .config
                    .workspace_panels()
                    .and_then(|sizes| sizes.first().copied())
                    .unwrap_or(SIDEBAR_WIDTH)))
                .size_range(px(210.)..px(360.))
                .flex_none()
                .child(self.sidebar(cx));
            let main_area = resizable_panel().child(
                v_flex()
                    .size_full()
                    .relative()
                    .overflow_hidden()
                    .child(body_panel),
            );
            h_flex()
                .size_full()
                .child(
                    div().flex_1().min_w(px(0.)).h_full().child(
                        h_resizable("ashell-workspace")
                            .lock(self.config.lock_layout())
                            .with_state(&self.workspace_panels)
                            .child(sidebar_area)
                            .child(main_area),
                    ),
                )
                .into_any_element()
        }
    }
}

impl Render for Ashell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let window_title = self.current_window_title();
        if window.window_title() != window_title {
            window.set_window_title(&window_title);
        }

        if !self.config.is_persistent() && self.startup_config_error.take().is_some() {
            let title = t!("config_load_failed_title").to_string();
            let message = t!("config_load_failed_message").to_string();
            cx.defer_in(window, move |_, window, cx| {
                window.push_notification(
                    Notification::error(message).title(title).autohide(false),
                    cx,
                );
            });
        }

        let active_document_id = self.active_document_id();
        if self
            .active_tab
            .as_ref()
            .is_some_and(|active_id| !self.tabs.iter().any(|tab| &tab.id == active_id))
        {
            self.active_tab = self.tabs.first().map(|tab| tab.id.clone());
        }
        if active_document_id.is_none() {
            self.sync_sftp_path_input(window, cx);
        }

        if self.show_transfers_dialog {
            self.show_transfers_dialog = false;
            self.show_transfers_dialog(window, cx);
        }
        if active_document_id.is_none()
            && let Some(active_id) = self.active_tab.clone()
        {
            if let Some(scrollbar) = self.terminal_scrollbars.get(&active_id)
                && let Some(new_display_offset) = scrollbar.future_display_offset.take()
                && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == active_id)
            {
                let current = tab.render_snapshot(false).display_offset;
                match new_display_offset.cmp(&current) {
                    std::cmp::Ordering::Greater => tab.scroll_up_by(new_display_offset - current),
                    std::cmp::Ordering::Less => tab.scroll_down_by(current - new_display_offset),
                    std::cmp::Ordering::Equal => {}
                }
            }
            if let Some(snapshot) = self.active_snapshot().as_ref()
                && let Some(scrollbar) = self.terminal_scrollbars.get(&active_id)
            {
                scrollbar.update(snapshot, px(self.terminal_line_height()));
            }
        }

        let workspace = self.render_terminal_workspace(window, cx);

        v_flex()
            .id("ashell-root")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(self.ui_font_family.clone())
            .on_action(cx.listener(|this, _: &crate::OpenSettings, window, cx| this.show_settings_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &crate::OpenSession, window, cx| this.show_selector_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &crate::OpenTransfers, window, cx| this.show_transfers_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &crate::NewSsh, window, cx| this.show_ssh_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &crate::OpenSearch, window, cx| this.toggle_search(window, cx)))
            .on_action(cx.listener(|this, _: &crate::ToggleSidebar, _, cx| {
                this.sidebar_collapsed = !this.sidebar_collapsed;
                this.config.set_sidebar_collapsed(this.sidebar_collapsed);
                this.save_preferences_background();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleSftpZoom, window, cx| {
                this.toggle_sftp_minimized(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::FocusPaneLeft, window, cx| this.focus_adjacent_pane("left", window, cx)))
            .on_action(cx.listener(|this, _: &crate::FocusPaneRight, window, cx| this.focus_adjacent_pane("right", window, cx)))
            .on_action(cx.listener(|this, _: &crate::FocusPaneUp, window, cx| this.focus_adjacent_pane("up", window, cx)))
            .on_action(cx.listener(|this, _: &crate::FocusPaneDown, window, cx| this.focus_adjacent_pane("down", window, cx)))
            .on_action(cx.listener(|this, _: &crate::SplitPaneLeft, _, cx| this.split_current_pane("left", cx)))
            .on_action(cx.listener(|this, _: &crate::SplitPaneRight, _, cx| this.split_current_pane("right", cx)))
            .on_action(cx.listener(|this, _: &crate::SplitPaneUp, _, cx| this.split_current_pane("up", cx)))
            .on_action(cx.listener(|this, _: &crate::SplitPaneDown, _, cx| this.split_current_pane("down", cx)))
            .on_action(cx.listener(|this, _: &crate::ClosePane, window, cx| {
                match this
                    .pane_root
                    .focused_leaf(&this.focused_pane_path)
                    .cloned()
                {
                    Some(PaneLeaf::Terminal(tab_id)) => this.close_tab(tab_id, window, cx),
                    Some(PaneLeaf::Document(document_id)) => {
                        this.request_close_document(document_id, window, cx);
                    }
                    Some(PaneLeaf::Empty) | None => {}
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Copy, window, cx| {
                if window.focused(cx) == Some(this.focus_handle.clone()) {
                    if let Some(text) = this.active_terminal_selection_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        if let Some(active_id) = &this.active_tab
                            && let Some(tab) = this.tabs.iter_mut().find(|tab| &tab.id == active_id) {
                                tab.clear_selection();
                            }
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Paste, window, cx| {
                if window.focused(cx) == Some(this.focus_handle.clone()) {
                    if let Some(clipboard) = cx.read_from_clipboard()
                        && let Some(text) = clipboard.text() {
                            this.paste_into_terminal(&text, window, cx);
                        }
                } else {
                    cx.propagate();
                }
            }))
            .child(self.render_windows_title_bar(window, cx))
            .child(
                div().flex_1().min_h_0().child(workspace),
            )
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .when_some(self.sftp_context_menu.clone(), |this, menu| {
                let label = if menu.is_dir {
                    t!("download_folder").to_string()
                } else {
                    t!("download").to_string()
                };
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.dismiss_sftp_context_menu(cx);
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.dismiss_sftp_context_menu(cx);
                            }),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(menu.position.x)
                                .top(menu.position.y)
                                .w(px(172.))
                                .p_1()
                                .rounded_md()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().popover)
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_mouse_down(MouseButton::Right, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .child(
                                    v_flex()
                                        .w_full()
                                        .child(
                                            Button::new("sftp-context-download")
                                                .ghost()
                                                .w_full()
                                                .justify_start()
                                                .label(label)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.trigger_sftp_context_download(window, cx);
                                                })),
                                        )
                                        .when(!menu.is_dir, |this| {
                                            this.child(
                                                Button::new("sftp-context-open-document")
                                                    .ghost()
                                                    .w_full()
                                                    .justify_start()
                                                    .label(t!("open_in_ashell"))
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.trigger_sftp_context_open_document(
                                                                window, cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                        })
                                        .when(
                                            !menu.is_dir
                                                && is_editable_text_file(&menu.remote_path),
                                            |this| {
                                                this.child(
                                                    Button::new("sftp-context-edit")
                                                        .ghost()
                                                        .w_full()
                                                        .justify_start()
                                                        .label(t!("edit_file"))
                                                        .tooltip(
                                                            t!("edit_file_tooltip").to_string(),
                                                        )
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.trigger_sftp_context_edit(cx);
                                                        })),
                                                )
                                            },
                                        )
                                        .child(
                                            Button::new("sftp-context-delete")
                                                .ghost()
                                                .w_full()
                                                .justify_start()
                                                .icon(IconName::Delete)
                                                .label(t!("delete"))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.trigger_sftp_context_delete(window, cx);
                                                })),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.connection_progress.clone(), |this, progress| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .bg(gpui::Hsla {
                            h: 0.0,
                            s: 0.0,
                            l: 0.0,
                            a: 0.48,
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(420.))
                                .p_5()
                                .rounded_lg()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().popover)
                                .shadow_lg()
                                .child(
                                    v_flex()
                                        .gap_4()
                                        .child(
                                            Button::new("ssh-connect-progress")
                                                .primary()
                                                .loading(!progress.failed)
                                                .label(progress.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .relative()
                                                .min_h(px(0.))
                                                .max_h(px(220.))
                                                .child(
                                                    div()
                                                        .id("connection-progress-scroll")
                                                        .max_h(px(220.))
                                                        .overflow_hidden()
                                                        .overflow_y_scroll()
                                                        .track_scroll(&self.connection_scroll_handle)
                                                        .child(
                                                            v_flex().gap_2().children(
                                                                progress.lines.iter().cloned().map(|line| {
                                                                    div()
                                                                        .text_size(rems(1.0))
                                                                        .text_color(if progress.failed {
                                                                            cx.theme().danger
                                                                        } else {
                                                                            cx.theme().muted_foreground
                                                                        })
                                                                        .child(line)
                                                                }),
                                                            ),
                                                        )
                                                )
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .top_0()
                                                        .right_0()
                                                        .bottom_0()
                                                        .w(px(16.))
                                                        .child(
                                                            Scrollbar::vertical(&self.connection_scroll_handle)
                                                                .scrollbar_show(ScrollbarShow::Scrolling)
                                                        )
                                                )
                                        )
                                        .when(progress.failed, |this| {
                                            this.child(
                                                h_flex()
                                                    .justify_end()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("ssh-connect-progress-retry")
                                                            .primary()
                                                            .label(t!("retry").to_string())
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.retry_connection_progress(
                                                                        cx,
                                                                    )
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("ssh-connect-progress-close")
                                                            .label(t!("cancel").to_string())
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.cancel_connection_progress(
                                                                        cx,
                                                                    )
                                                                },
                                                            )),
                                                    ),
                                            )
                                        }),
                                ),
                        ),
                )
            })
            .on_prepaint({
                let view = cx.entity().clone();
                move |_, window, cx| {
                    view.update(cx, |this, cx| {
                        let current_win_size = window.viewport_size();
                        let size_changed = this.last_window_size != Some(current_win_size);
                        this.last_window_size = Some(current_win_size);

                        let current_sizes = this.workspace_panels.read(cx).sizes().clone();
                        if let Some(current_first_size) = current_sizes.first().copied() {
                            if size_changed {
                                if let Some(target_width) = this.last_sidebar_width
                                    && current_first_size != target_width {
                                        this.workspace_panels.update(cx, |state, cx| {
                                            state.resize_panel(0, target_width, window, cx);
                                        });
                                    }
                            } else {
                                this.last_sidebar_width = Some(current_first_size);
                            }
                        }
                    });
                }
            })
    }
}
