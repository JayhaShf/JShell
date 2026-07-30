use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Subscription,
    Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder as _, px, relative, rems,
    size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Root, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use rust_i18n::t;

use crate::{
    Ashell,
    app::{constants::DOCUMENT_KEY_CONTEXT, format_window_title},
    document::{
        DocumentConnectionState, DocumentMode, LineEnding, LoadState, ReadOnlyReason, SaveState,
    },
    system::format_bytes,
};

pub(crate) fn can_detach_document(
    load_state: &LoadState,
    save_state: &SaveState,
    mode: DocumentMode,
) -> bool {
    matches!(load_state, LoadState::Ready)
        && matches!(mode, DocumentMode::Editable)
        && !matches!(save_state, SaveState::Checking | SaveState::Saving)
}

pub(crate) fn detached_window_can_close(save_state: &SaveState) -> bool {
    !matches!(save_state, SaveState::Checking | SaveState::Saving)
}

struct DetachedDocumentSnapshot {
    editor: Entity<InputState>,
    endpoint: String,
    language: String,
    line_ending: &'static str,
    encoding: String,
    save_state: SaveState,
    connection_state: DocumentConnectionState,
    read_only_reason: Option<ReadOnlyReason>,
    dirty: bool,
    read_only: bool,
    soft_wrap: bool,
    size: u64,
    font_family: gpui::SharedString,
    font_size: f32,
    line_height: f32,
    title: String,
}

pub(crate) struct DetachedDocumentWindow {
    app: Entity<Ashell>,
    document_id: String,
    main_window: AnyWindowHandle,
    last_title: Option<String>,
    _app_subscription: Subscription,
}

impl DetachedDocumentWindow {
    fn new(
        app: Entity<Ashell>,
        document_id: String,
        main_window: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        if let Some(document) = app.read(cx).documents.get(&document_id) {
            document.editor.focus_handle(cx).focus(window, cx);
        }
        Self {
            app,
            document_id,
            main_window,
            last_title: None,
            _app_subscription: app_subscription,
        }
    }

    fn snapshot(&self, cx: &App) -> Option<DetachedDocumentSnapshot> {
        let app = self.app.read(cx);
        let document = app.documents.get(&self.document_id)?;
        let line_ending = match document.line_ending {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        };
        let encoding = match document.read_only_reason {
            Some(ReadOnlyReason::UnsupportedEncoding) => t!("document_non_utf8").to_string(),
            Some(ReadOnlyReason::Binary) => t!("document_binary_label").to_string(),
            _ => "UTF-8".to_string(),
        };
        Some(DetachedDocumentSnapshot {
            editor: document.editor.clone(),
            endpoint: format!(
                "{}@{}:{}",
                document.session.user, document.session.host, document.key.remote_path
            ),
            language: document.language.clone(),
            line_ending,
            encoding,
            save_state: document.save_state.clone(),
            connection_state: document.connection_state.clone(),
            read_only_reason: document.read_only_reason.clone(),
            dirty: document.revisions.is_dirty(),
            read_only: document.is_read_only(),
            soft_wrap: document.soft_wrap,
            size: document
                .metadata
                .as_ref()
                .map(|metadata| metadata.size)
                .unwrap_or(0),
            font_family: app.terminal_font_family.clone(),
            font_size: app.terminal_font_size,
            line_height: app.terminal_line_height(),
            title: document.title().to_string(),
        })
    }

    fn reattach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let can_close = self
            .app
            .read(cx)
            .documents
            .get(&self.document_id)
            .is_none_or(|document| detached_window_can_close(&document.save_state));
        if !can_close {
            return;
        }

        let current_window = window.window_handle();
        let document_id = self.document_id.clone();
        let app = self.app.clone();
        let main_window = self.main_window;
        app.update(cx, |this, cx| {
            if this.detached_document_windows.get(&document_id) == Some(&current_window) {
                this.detached_document_windows.remove(&document_id);
            }
            this.detaching_document_ids.remove(&document_id);
            cx.notify();
        });
        window.remove_window();
        focus_document_in_main_window(app, document_id, main_window, cx);
    }
}

impl Render for DetachedDocumentWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(snapshot) = self.snapshot(cx) else {
            window.set_window_title("JShell");
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .child(t!("document_not_found").to_string())
                .into_any_element();
        };

        let window_title = format_window_title(Some((&snapshot.title, snapshot.dirty)), None);
        if self.last_title.as_deref() != Some(window_title.as_str()) {
            window.set_window_title(&window_title);
            self.last_title = Some(window_title);
        }

        let cursor = snapshot.editor.read(cx).cursor_position();
        let cursor_label = format!(
            "{} {}, {} {}",
            t!("document_line"),
            cursor.line + 1,
            t!("document_column"),
            cursor.character + 1
        );
        let save_label = match &snapshot.save_state {
            SaveState::Idle if snapshot.dirty => t!("document_unsaved").to_string(),
            SaveState::Idle | SaveState::Saved => t!("document_saved").to_string(),
            SaveState::Checking => t!("document_checking").to_string(),
            SaveState::Saving => t!("document_saving").to_string(),
            SaveState::Conflict => t!("document_conflict_state").to_string(),
            SaveState::Failed(error) => {
                t!("document_save_failed", error = error.clone()).to_string()
            }
            SaveState::OutcomeUnknown(error) => {
                t!("document_save_outcome_unknown", error = error.clone()).to_string()
            }
        };
        let status_color = match &snapshot.save_state {
            SaveState::Failed(_) | SaveState::OutcomeUnknown(_) | SaveState::Conflict => {
                cx.theme().danger
            }
            SaveState::Saved => cx.theme().success,
            _ if snapshot.dirty => cx.theme().warning,
            _ => cx.theme().muted_foreground,
        };
        let is_online = matches!(snapshot.connection_state, DocumentConnectionState::Online);
        let save_in_progress =
            matches!(snapshot.save_state, SaveState::Checking | SaveState::Saving);

        let app_for_save_action = self.app.clone();
        let document_for_save_action = self.document_id.clone();
        let app_for_wrap = self.app.clone();
        let document_for_wrap = self.document_id.clone();
        let app_for_download = self.app.clone();
        let document_for_download = self.document_id.clone();
        let app_for_save = self.app.clone();
        let document_for_save = self.document_id.clone();
        let app_for_reconnect = self.app.clone();
        let document_for_reconnect = self.document_id.clone();

        let editor_body: AnyElement = v_flex()
            .size_full()
            .when_some(snapshot.read_only_reason.clone(), |this, reason| {
                let message = match reason {
                    ReadOnlyReason::UnsupportedEncoding => {
                        t!("document_unsupported_encoding").to_string()
                    }
                    ReadOnlyReason::LargeFile => t!("document_large_file_read_only").to_string(),
                    ReadOnlyReason::Binary => t!("document_binary_file").to_string(),
                    ReadOnlyReason::MetadataOnly => t!("document_metadata_only").to_string(),
                };
                this.child(
                    div()
                        .flex_none()
                        .w_full()
                        .px_3()
                        .py_2()
                        .bg(cx.theme().warning.opacity(0.12))
                        .text_color(cx.theme().warning)
                        .child(message),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .px_2()
                    .font_family(snapshot.font_family.clone())
                    .text_size(px(snapshot.font_size))
                    .line_height(relative(snapshot.line_height / snapshot.font_size.max(1.0)))
                    .child(
                        Input::new(&snapshot.editor)
                            .h_full()
                            .appearance(false)
                            .disabled(snapshot.read_only),
                    ),
            )
            .into_any_element();

        v_flex()
            .id(format!("detached-document-{}", self.document_id))
            .key_context(DOCUMENT_KEY_CONTEXT)
            .size_full()
            .bg(cx.theme().background)
            .on_action(move |_: &crate::SaveDocument, window, cx| {
                app_for_save_action.update(cx, |this, cx| {
                    this.save_document(document_for_save_action.clone(), window, cx);
                });
            })
            .child(
                h_flex()
                    .flex_none()
                    .h(px(42.))
                    .w_full()
                    .px_3()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .font_family(snapshot.font_family.clone())
                            .text_size(rems(0.917))
                            .child(snapshot.endpoint),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(cx.theme().muted)
                            .text_size(rems(0.833))
                            .child(snapshot.language),
                    )
                    .child(div().text_size(rems(0.833)).child(snapshot.encoding))
                    .child(div().text_size(rems(0.833)).child(snapshot.line_ending))
                    .child(
                        Button::new(format!("detached-document-wrap-{}", self.document_id))
                            .small()
                            .when(snapshot.soft_wrap, |button| button.primary())
                            .when(!snapshot.soft_wrap, |button| button.secondary())
                            .label(t!("document_wrap").to_string())
                            .on_click(move |_, window, cx| {
                                app_for_wrap.update(cx, |this, cx| {
                                    this.toggle_document_soft_wrap(
                                        document_for_wrap.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                    .when(snapshot.read_only, |toolbar| {
                        toolbar.child(
                            Button::new(format!("detached-document-download-{}", self.document_id))
                                .secondary()
                                .small()
                                .label(t!("document_download").to_string())
                                .on_click(move |_, window, cx| {
                                    app_for_download.update(cx, |this, cx| {
                                        this.download_document(
                                            document_for_download.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                    })
                    .when(!snapshot.read_only, |toolbar| {
                        toolbar.child(
                            Button::new(format!("detached-document-save-{}", self.document_id))
                                .primary()
                                .small()
                                .disabled(!snapshot.dirty || !is_online || save_in_progress)
                                .label(match snapshot.save_state {
                                    SaveState::Checking => t!("document_checking").to_string(),
                                    SaveState::Saving => t!("document_saving").to_string(),
                                    _ => t!("document_save").to_string(),
                                })
                                .on_click(move |_, window, cx| {
                                    app_for_save.update(cx, |this, cx| {
                                        this.save_document(document_for_save.clone(), window, cx);
                                    });
                                }),
                        )
                    })
                    .child(
                        Button::new(format!("document-reattach-{}", self.document_id))
                            .secondary()
                            .small()
                            .icon(IconName::PanelRightClose)
                            .tooltip(t!("document_reattach").to_string())
                            .disabled(save_in_progress)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reattach(window, cx);
                            })),
                    ),
            )
            .when(!is_online, |root| {
                root.child(
                    h_flex()
                        .flex_none()
                        .min_h(px(38.))
                        .w_full()
                        .px_3()
                        .gap_3()
                        .items_center()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().danger.opacity(0.1))
                        .text_color(cx.theme().danger)
                        .child(div().flex_1().child(match snapshot.connection_state {
                            DocumentConnectionState::Offline(_) => {
                                t!("document_offline").to_string()
                            }
                            DocumentConnectionState::Reconnecting => {
                                t!("document_reconnecting").to_string()
                            }
                            DocumentConnectionState::Online => String::new(),
                        }))
                        .child(
                            Button::new(format!(
                                "detached-document-reconnect-{}",
                                self.document_id
                            ))
                            .secondary()
                            .small()
                            .loading(matches!(
                                snapshot.connection_state,
                                DocumentConnectionState::Reconnecting
                            ))
                            .disabled(matches!(
                                snapshot.connection_state,
                                DocumentConnectionState::Reconnecting
                            ))
                            .label(t!("document_reconnect_save").to_string())
                            .on_click(move |_, window, cx| {
                                app_for_reconnect.update(cx, |this, cx| {
                                    this.reconnect_document(
                                        document_for_reconnect.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }),
                        ),
                )
            })
            .child(div().flex_1().min_h_0().child(editor_body))
            .child(
                h_flex()
                    .flex_none()
                    .h(px(28.))
                    .w_full()
                    .px_3()
                    .items_center()
                    .gap_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .text_size(rems(0.833))
                    .text_color(cx.theme().muted_foreground)
                    .child(cursor_label)
                    .child(format_bytes(snapshot.size))
                    .child(div().flex_1())
                    .child(div().text_color(status_color).child(save_label)),
            )
            .into_any_element()
    }
}

fn focus_document_in_main_window(
    app: Entity<Ashell>,
    document_id: String,
    main_window: AnyWindowHandle,
    cx: &mut App,
) {
    cx.defer(move |cx| {
        let app_for_focus = app.clone();
        let _ = main_window.update(cx, move |_, window, cx| {
            window.activate_window();
            app_for_focus.update(cx, |this, cx| {
                this.focus_document_workspace(&document_id, window, cx);
            });
        });
    });
}

fn open_detached_document_window(
    app: Entity<Ashell>,
    document_id: String,
    main_window: AnyWindowHandle,
    cx: &mut App,
) -> anyhow::Result<AnyWindowHandle> {
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(980.), px(720.)),
            cx,
        ))),
        ..WindowOptions::default()
    };
    #[cfg(not(target_os = "macos"))]
    if let Ok(image) = image::load_from_memory(include_bytes!("../../assets/icons/jshell.png")) {
        options.icon = Some(std::sync::Arc::new(image.into_rgba8()));
    }

    let app_for_window = app.clone();
    let document_for_window = document_id.clone();
    let handle = cx.open_window(options, move |window, cx| {
        let title = app_for_window
            .read(cx)
            .documents
            .get(&document_for_window)
            .map(|document| {
                format_window_title(
                    Some((document.title(), document.revisions.is_dirty())),
                    None,
                )
            })
            .unwrap_or_else(|| "JShell".to_string());
        window.set_window_title(&title);

        let close_app = app_for_window.clone();
        let close_document = document_for_window.clone();
        window.on_window_should_close(cx, move |window, cx| {
            let current_window = window.window_handle();
            let (can_close, should_reattach) = close_app.update(cx, |this, cx| {
                let can_close = this.closing_application
                    || this
                        .documents
                        .get(&close_document)
                        .is_none_or(|document| detached_window_can_close(&document.save_state));
                if !can_close {
                    return (false, false);
                }
                if this.detached_document_windows.get(&close_document) == Some(&current_window) {
                    this.detached_document_windows.remove(&close_document);
                }
                this.detaching_document_ids.remove(&close_document);
                let should_reattach =
                    !this.closing_application && this.documents.contains_key(&close_document);
                cx.notify();
                (true, should_reattach)
            });
            if !can_close {
                return false;
            }
            if should_reattach {
                focus_document_in_main_window(
                    close_app.clone(),
                    close_document.clone(),
                    main_window,
                    cx,
                );
            }
            true
        });

        let host = cx.new(|cx| {
            DetachedDocumentWindow::new(
                app_for_window,
                document_for_window,
                main_window,
                window,
                cx,
            )
        });
        cx.new(|cx| Root::new(host, window, cx))
    })?;
    Ok(handle.into())
}

impl Ashell {
    pub(crate) fn is_document_detached(&self, document_id: &str) -> bool {
        self.detaching_document_ids.contains(document_id)
            || self.detached_document_windows.contains_key(document_id)
    }

    pub(crate) fn activate_detached_document(
        &self,
        document_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.detached_document_windows.get(document_id).copied() else {
            return self.detaching_document_ids.contains(document_id);
        };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        });
        true
    }

    pub(crate) fn detach_document_window(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.activate_detached_document(&document_id, cx) {
            return;
        }
        let Some(document) = self.documents.get(&document_id) else {
            return;
        };
        if !can_detach_document(&document.load_state, &document.save_state, document.mode) {
            self.status = t!("document_detach_unavailable").into();
            cx.notify();
            return;
        }

        self.detaching_document_ids.insert(document_id.clone());
        cx.notify();
        let app = cx.entity();
        let main_window = window.window_handle();
        cx.defer(move |cx| {
            let result =
                open_detached_document_window(app.clone(), document_id.clone(), main_window, cx);
            match result {
                Ok(handle) => {
                    let keep_window = app.update(cx, |this, cx| {
                        this.detaching_document_ids.remove(&document_id);
                        if this.closing_application || !this.documents.contains_key(&document_id) {
                            return false;
                        }
                        this.detached_document_windows
                            .insert(document_id.clone(), handle);
                        cx.notify();
                        true
                    });
                    if !keep_window {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
                Err(error) => {
                    app.update(cx, |this, cx| {
                        this.detaching_document_ids.remove(&document_id);
                        this.status =
                            t!("document_detach_failed", error = error.to_string()).into();
                        cx.notify();
                    });
                }
            }
        });
    }

    pub(crate) fn close_detached_document_window(
        &mut self,
        document_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.detaching_document_ids.remove(document_id);
        let Some(handle) = self.detached_document_windows.remove(document_id) else {
            return;
        };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    }

    pub(crate) fn close_detached_windows_for_shutdown(&mut self, cx: &mut Context<Self>) {
        self.closing_application = true;
        self.detaching_document_ids.clear();
        let handles = self
            .detached_document_windows
            .drain()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();
        cx.defer(move |cx| {
            for handle in handles {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{can_detach_document, detached_window_can_close};
    use crate::document::{DocumentMode, LoadState, SaveState};

    #[test]
    fn detach_requires_a_ready_regular_document_without_an_active_save() {
        assert!(can_detach_document(
            &LoadState::Ready,
            &SaveState::Idle,
            DocumentMode::Editable,
        ));
        assert!(!can_detach_document(
            &LoadState::Loading,
            &SaveState::Idle,
            DocumentMode::Editable,
        ));
        assert!(!can_detach_document(
            &LoadState::Ready,
            &SaveState::Checking,
            DocumentMode::Editable,
        ));
        assert!(!can_detach_document(
            &LoadState::Ready,
            &SaveState::Saving,
            DocumentMode::Editable,
        ));
        assert!(!can_detach_document(
            &LoadState::Ready,
            &SaveState::Idle,
            DocumentMode::PagedReadOnly,
        ));
        assert!(!can_detach_document(
            &LoadState::Ready,
            &SaveState::Idle,
            DocumentMode::MetadataOnly,
        ));
    }

    #[test]
    fn detached_window_waits_for_an_active_save_before_closing() {
        assert!(detached_window_can_close(&SaveState::Idle));
        assert!(detached_window_can_close(&SaveState::Saved));
        assert!(detached_window_can_close(&SaveState::Conflict));
        assert!(!detached_window_can_close(&SaveState::Checking));
        assert!(!detached_window_can_close(&SaveState::Saving));
    }
}
