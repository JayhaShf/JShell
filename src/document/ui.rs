use gpui::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px, relative, rems,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    v_flex,
};
use rust_i18n::t;

use crate::{
    Ashell,
    app::constants::DOCUMENT_KEY_CONTEXT,
    document::{
        DocumentConnectionState, DocumentMode, LineEnding, LoadState, ReadOnlyReason, SaveState,
    },
    system::format_bytes,
};

pub(crate) fn render_document(
    this: &mut Ashell,
    document_id: &str,
    _window: &mut Window,
    cx: &mut Context<Ashell>,
) -> gpui::AnyElement {
    let Some(document) = this.documents.get(document_id) else {
        return v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(t!("document_not_found").to_string())
            .into_any_element();
    };

    let editor = document.editor.clone();
    let large_file_viewer = document.large_file_viewer.clone();
    let large_file_search = document.large_file_search.clone();
    let path = document.key.remote_path.clone();
    let endpoint = format!(
        "{}@{}:{}",
        document.session.user, document.session.host, path
    );
    let language = document.language.clone();
    let line_ending = match document.line_ending {
        LineEnding::Lf => "LF",
        LineEnding::CrLf => "CRLF",
    };
    let load_state = document.load_state.clone();
    let save_state = document.save_state.clone();
    let connection_state = document.connection_state.clone();
    let is_online = matches!(&connection_state, DocumentConnectionState::Online);
    let dirty = document.revisions.is_dirty();
    let read_only = document.is_read_only();
    let read_only_reason = document.read_only_reason.clone();
    let mode = document.mode;
    let large_file = document.large_file.clone();
    let soft_wrap = document.soft_wrap;
    let can_detach = crate::document::window::can_detach_document(&load_state, &save_state, mode);
    let size = document
        .metadata
        .as_ref()
        .map(|metadata| metadata.size)
        .unwrap_or(0);
    let cursor = if mode == DocumentMode::PagedReadOnly {
        large_file_viewer.read(cx).cursor_position()
    } else {
        editor.read(cx).cursor_position()
    };
    let encoding_label = match &read_only_reason {
        Some(ReadOnlyReason::UnsupportedEncoding) => t!("document_non_utf8").to_string(),
        Some(ReadOnlyReason::Binary) => t!("document_binary_label").to_string(),
        _ => "UTF-8".to_string(),
    };
    let cursor_label = format!(
        "{} {}, {} {}",
        t!("document_line"),
        cursor.line + 1,
        t!("document_column"),
        cursor.character + 1
    );
    let save_label = match &save_state {
        SaveState::Idle if dirty => t!("document_unsaved").to_string(),
        SaveState::Idle => t!("document_saved").to_string(),
        SaveState::Checking => t!("document_checking").to_string(),
        SaveState::Saving => t!("document_saving").to_string(),
        SaveState::Saved => t!("document_saved").to_string(),
        SaveState::Conflict => t!("document_conflict_state").to_string(),
        SaveState::Failed(error) => t!("document_save_failed", error = error.clone()).to_string(),
        SaveState::OutcomeUnknown(error) => {
            t!("document_save_outcome_unknown", error = error.clone()).to_string()
        }
    };
    let status_color = match &save_state {
        SaveState::Failed(_) | SaveState::OutcomeUnknown(_) | SaveState::Conflict => {
            cx.theme().danger
        }
        SaveState::Saved => cx.theme().success,
        _ if dirty => cx.theme().warning,
        _ => cx.theme().muted_foreground,
    };

    let body = match &load_state {
        LoadState::Loading => v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Button::new(format!("document-loading-{document_id}"))
                    .primary()
                    .loading(true)
                    .label(t!("document_loading").to_string()),
            )
            .into_any_element(),
        LoadState::Failed(error) => v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .max_w(px(720.))
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            )
            .child(
                Button::new(format!("document-retry-{document_id}"))
                    .secondary()
                    .label(t!("document_retry").to_string())
                    .on_click({
                        let document_id = document_id.to_string();
                        cx.listener(move |this, _, window, cx| {
                            this.reload_document(document_id.clone(), window, cx);
                        })
                    }),
            )
            .into_any_element(),
        LoadState::Ready => {
            if matches!(mode, DocumentMode::MetadataOnly)
                || matches!(read_only_reason, Some(ReadOnlyReason::Binary))
            {
                let message = match read_only_reason {
                    Some(ReadOnlyReason::Binary) => t!("document_binary_file").to_string(),
                    _ => t!("document_metadata_only").to_string(),
                };
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(rems(1.25))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(message),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(format_bytes(size)),
                    )
                    .into_any_element()
            } else if matches!(mode, DocumentMode::PagedReadOnly) {
                let state = large_file.clone().unwrap_or_default();
                let can_load_previous = state.offset > 0 && !state.loading;
                let can_load_next = state.end_offset < size && !state.loading;
                let range_label = t!(
                    "document_byte_range",
                    start = state.start_offset,
                    end = state.end_offset,
                    total = size
                )
                .to_string();
                let page_text = state.text.clone();
                let search_query = large_file_search.read(cx).value().to_string();
                let match_count =
                    crate::document::large_file::count_matches(&state.text, search_query.as_str());
                v_flex()
                    .size_full()
                    .child(
                        div()
                            .flex_none()
                            .w_full()
                            .px_3()
                            .py_2()
                            .bg(cx.theme().warning.opacity(0.12))
                            .text_color(cx.theme().warning)
                            .child(t!("document_large_file_read_only").to_string()),
                    )
                    .when_some(state.error.clone(), |this, error| {
                        this.child(
                            div()
                                .flex_none()
                                .w_full()
                                .px_3()
                                .py_2()
                                .bg(cx.theme().danger.opacity(0.12))
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .px_2()
                            .font_family(this.terminal_font_family.clone())
                            .text_size(px(this.terminal_font_size))
                            .line_height(relative(1.45))
                            .child(
                                Input::new(&large_file_viewer)
                                    .h_full()
                                    .appearance(false)
                                    .disabled(true),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(38.))
                            .w_full()
                            .px_3()
                            .items_center()
                            .gap_2()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new(format!("document-page-previous-{document_id}"))
                                    .secondary()
                                    .small()
                                    .icon(IconName::ChevronLeft)
                                    .tooltip(t!("document_previous_page").to_string())
                                    .disabled(!can_load_previous)
                                    .on_click({
                                        let document_id = document_id.to_string();
                                        cx.listener(move |this, _, window, cx| {
                                            this.load_previous_document_page(
                                                document_id.clone(),
                                                window,
                                                cx,
                                            );
                                        })
                                    }),
                            )
                            .child(
                                Button::new(format!("document-page-next-{document_id}"))
                                    .secondary()
                                    .small()
                                    .icon(IconName::ChevronRight)
                                    .tooltip(t!("document_next_page").to_string())
                                    .disabled(!can_load_next)
                                    .on_click({
                                        let document_id = document_id.to_string();
                                        cx.listener(move |this, _, window, cx| {
                                            this.load_next_document_page(
                                                document_id.clone(),
                                                window,
                                                cx,
                                            );
                                        })
                                    }),
                            )
                            .child(div().flex_1().text_size(rems(0.833)).child(range_label))
                            .child(
                                Input::new(&large_file_search)
                                    .small()
                                    .w(px(180.))
                                    .appearance(false),
                            )
                            .child(
                                div().w(px(74.)).text_size(rems(0.833)).child(
                                    t!("document_match_count", count = match_count).to_string(),
                                ),
                            )
                            .child(
                                Button::new(format!("document-page-copy-{document_id}"))
                                    .secondary()
                                    .small()
                                    .icon(IconName::Copy)
                                    .tooltip(t!("document_copy_chunk").to_string())
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            page_text.clone(),
                                        ));
                                    }),
                            ),
                    )
                    .into_any_element()
            } else {
                v_flex()
                    .size_full()
                    .when_some(read_only_reason.clone(), |this, reason| {
                        let message = match reason {
                            ReadOnlyReason::UnsupportedEncoding => {
                                t!("document_unsupported_encoding").to_string()
                            }
                            ReadOnlyReason::LargeFile => {
                                t!("document_large_file_read_only").to_string()
                            }
                            ReadOnlyReason::Binary => t!("document_binary_file").to_string(),
                            ReadOnlyReason::MetadataOnly => {
                                t!("document_metadata_only").to_string()
                            }
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
                            .font_family(this.terminal_font_family.clone())
                            .text_size(px(this.terminal_font_size))
                            .line_height(relative(1.45))
                            .child(
                                Input::new(&editor)
                                    .h_full()
                                    .appearance(false)
                                    .disabled(read_only),
                            ),
                    )
                    .into_any_element()
            }
        }
    };

    v_flex()
        .id(format!("document-workspace-{document_id}"))
        .key_context(DOCUMENT_KEY_CONTEXT)
        .size_full()
        .bg(cx.theme().background)
        .on_action({
            let document_id = document_id.to_string();
            cx.listener(move |this, _: &crate::SaveDocument, window, cx| {
                this.save_document(document_id.clone(), window, cx);
            })
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
                        .font_family(this.terminal_font_family.clone())
                        .text_size(rems(0.917))
                        .child(endpoint),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(cx.theme().muted)
                        .text_size(rems(0.833))
                        .child(language),
                )
                .child(div().text_size(rems(0.833)).child(encoding_label))
                .child(div().text_size(rems(0.833)).child(line_ending))
                .child(
                    Button::new(format!("document-detach-{document_id}"))
                        .secondary()
                        .small()
                        .icon(IconName::ExternalLink)
                        .tooltip(t!("document_detach").to_string())
                        .disabled(!can_detach)
                        .on_click({
                            let document_id = document_id.to_string();
                            cx.listener(move |this, _, window, cx| {
                                this.detach_document_window(document_id.clone(), window, cx);
                            })
                        }),
                )
                .child(
                    Button::new(format!("document-wrap-{document_id}"))
                        .small()
                        .when(soft_wrap, |button| button.primary())
                        .when(!soft_wrap, |button| button.secondary())
                        .label(t!("document_wrap").to_string())
                        .on_click({
                            let document_id = document_id.to_string();
                            cx.listener(move |this, _, window, cx| {
                                this.toggle_document_soft_wrap(document_id.clone(), window, cx);
                            })
                        }),
                )
                .when(read_only, |toolbar| {
                    toolbar.child(
                        Button::new(format!("document-download-{document_id}"))
                            .secondary()
                            .small()
                            .label(t!("document_download").to_string())
                            .on_click({
                                let document_id = document_id.to_string();
                                cx.listener(move |this, _, window, cx| {
                                    this.download_document(document_id.clone(), window, cx);
                                })
                            }),
                    )
                })
                .when(!read_only, |toolbar| {
                    let save_in_progress =
                        matches!(save_state, SaveState::Checking | SaveState::Saving);
                    let save_button_label = match save_state {
                        SaveState::Checking => t!("document_checking").to_string(),
                        SaveState::Saving => t!("document_saving").to_string(),
                        _ => t!("document_save").to_string(),
                    };
                    toolbar.child(
                        Button::new(format!("document-save-{document_id}"))
                            .primary()
                            .small()
                            .disabled(
                                !dirty
                                    || !is_online
                                    || !matches!(load_state, LoadState::Ready)
                                    || save_in_progress,
                            )
                            .label(save_button_label)
                            .on_click({
                                let document_id = document_id.to_string();
                                cx.listener(move |this, _, window, cx| {
                                    this.save_document(document_id.clone(), window, cx);
                                })
                            }),
                    )
                }),
        )
        .when(
            !matches!(&connection_state, DocumentConnectionState::Online),
            |root| {
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
                        .child(div().flex_1().child(match &connection_state {
                            DocumentConnectionState::Offline(_) => {
                                t!("document_offline").to_string()
                            }
                            DocumentConnectionState::Reconnecting => {
                                t!("document_reconnecting").to_string()
                            }
                            DocumentConnectionState::Online => String::new(),
                        }))
                        .child(
                            Button::new(format!("document-reconnect-{document_id}"))
                                .secondary()
                                .small()
                                .loading(matches!(
                                    &connection_state,
                                    DocumentConnectionState::Reconnecting
                                ))
                                .disabled(matches!(
                                    &connection_state,
                                    DocumentConnectionState::Reconnecting
                                ))
                                .label(t!("document_reconnect_save").to_string())
                                .on_click({
                                    let document_id = document_id.to_string();
                                    cx.listener(move |this, _, window, cx| {
                                        this.reconnect_document(document_id.clone(), window, cx);
                                    })
                                }),
                        ),
                )
            },
        )
        .child(div().flex_1().min_h_0().child(body))
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
                .child(format_bytes(size))
                .child(div().flex_1())
                .child(div().text_color(status_color).child(save_label)),
        )
        .into_any_element()
}
