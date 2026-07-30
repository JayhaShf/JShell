use gpui::{
    Anchor, Context, Focusable as _, FontWeight, InteractiveElement as _, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    input::Input,
    menu::{DropdownMenu as _, PopupMenuItem},
    progress::Progress,
    scroll::{ScrollableElement as _, Scrollbar, ScrollbarShow},
    switch::Switch,
    v_flex,
};
use rust_i18n::t;

use crate::{
    Ashell,
    session::{
        SessionProxyPolicy, can_submit_ssh_session, config::AuthMethod, parse_non_zero_u16,
        session_proxy_policy, supported_proxy_protocol,
    },
    system::format_bytes,
};

fn open_completed_download_target_with<E>(
    target: &str,
    opener: impl FnOnce(&str) -> Result<(), E>,
) -> Result<(), String>
where
    E: std::fmt::Display,
{
    opener(target).map_err(|error| error.to_string())
}

fn open_completed_download_target(target: &str) {
    if let Err(error) = open_completed_download_target_with(target, |target| open::that(target)) {
        tracing::warn!(
            target = target,
            %error,
            "failed to open completed download target"
        );
    }
}

impl Ashell {
    pub(crate) fn show_window_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let documents: Vec<(String, String)> = self
            .dirty_document_ids()
            .into_iter()
            .filter_map(|document_id| {
                self.documents.get(&document_id).map(|document| {
                    (
                        document.title().to_string(),
                        document.key.remote_path.clone(),
                    )
                })
            })
            .collect();
        if documents.is_empty() {
            self.request_application_close(window, cx);
            return;
        }

        let view = cx.entity();
        window.open_dialog(cx, move |dialog: Dialog, _window, _cx| {
            let listed_documents = documents.clone();
            dialog
                .title(t!("document_close_all_title").to_string())
                .w(px(600.))
                .keyboard(false)
                .close_button(false)
                .content(move |content, _window, _cx| {
                    content.child(
                        v_flex()
                            .w_full()
                            .gap_3()
                            .child(div().child(t!("document_close_all_body").to_string()))
                            .child(
                                v_flex()
                                    .w_full()
                                    .max_h(px(260.))
                                    .overflow_y_scrollbar()
                                    .border_1()
                                    .border_color(gpui::rgba(0x0000001f))
                                    .children(listed_documents.iter().enumerate().map(
                                        |(index, (title, path))| {
                                            v_flex()
                                                .w_full()
                                                .px_3()
                                                .py_2()
                                                .when(index > 0, |row| {
                                                    row.border_t_1()
                                                        .border_color(gpui::rgba(0x0000001f))
                                                })
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(title.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(rems(0.833))
                                                        .font_family("monospace")
                                                        .child(path.clone()),
                                                )
                                        },
                                    )),
                            ),
                    )
                })
                .footer({
                    let cancel_view = view.clone();
                    let discard_view = view.clone();
                    let save_view = view.clone();
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("document-close-all-cancel")
                                .ghost()
                                .label(t!("cancel").to_string())
                                .on_click(move |_, window, cx| {
                                    cancel_view.update(cx, |this, cx| {
                                        this.cancel_application_close(cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("document-close-all-discard")
                                .danger()
                                .label(t!("document_discard").to_string())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    discard_view.update(cx, |this, cx| {
                                        this.discard_all_and_close(window, cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("document-close-all-save")
                                .primary()
                                .label(t!("document_save_all").to_string())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    save_view.update(cx, |this, cx| {
                                        this.save_all_and_close(window, cx);
                                    });
                                }),
                        )
                })
        });
    }

    pub(crate) fn show_document_close_dialog(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .documents
            .get(&document_id)
            .map(|document| document.key.remote_path.clone())
        else {
            return;
        };
        let view = cx.entity();
        window.open_dialog(cx, move |dialog: Dialog, _window, _cx| {
            let body = t!("document_close_unsaved_body", path = path.clone()).to_string();
            let content_path = path.clone();
            dialog
                .title(t!("document_close_unsaved_title").to_string())
                .w(px(560.))
                .keyboard(false)
                .content(move |content, _window, _cx| {
                    content.child(
                        v_flex()
                            .w_full()
                            .gap_3()
                            .child(div().child(body.clone()))
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(gpui::rgba(0x00000012))
                                    .font_family("monospace")
                                    .child(content_path.clone()),
                            ),
                    )
                })
                .footer({
                    let save_view = view.clone();
                    let discard_view = view.clone();
                    let save_document_id = document_id.clone();
                    let discard_document_id = document_id.clone();
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("document-close-cancel")
                                .ghost()
                                .label(t!("cancel").to_string())
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("document-close-discard")
                                .danger()
                                .label(t!("document_discard").to_string())
                                .on_click(move |_, window, cx| {
                                    discard_view.update(cx, |this, cx| {
                                        this.close_document_now(&discard_document_id, window, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("document-close-save")
                                .primary()
                                .label(t!("document_save").to_string())
                                .on_click(move |_, window, cx| {
                                    save_view.update(cx, |this, cx| {
                                        this.save_and_close_document(
                                            save_document_id.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                })
        });
    }

    pub(crate) fn show_document_conflict_dialog(
        &mut self,
        document_id: String,
        remote_deleted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .documents
            .get(&document_id)
            .map(|document| document.key.remote_path.clone())
        else {
            return;
        };
        let view = cx.entity();
        window.open_dialog(cx, move |dialog: Dialog, _window, _cx| {
            let title = if remote_deleted {
                t!("document_remote_deleted_title").to_string()
            } else {
                t!("document_conflict_title").to_string()
            };
            let body = if remote_deleted {
                t!("document_remote_deleted_body", path = path.clone()).to_string()
            } else {
                t!("document_conflict_body", path = path.clone()).to_string()
            };
            let content_path = path.clone();
            dialog
                .title(title)
                .w(px(560.))
                .keyboard(false)
                .content(move |content, _window, _cx| {
                    content.child(
                        v_flex()
                            .w_full()
                            .gap_3()
                            .child(div().child(body.clone()))
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(gpui::rgba(0x00000012))
                                    .font_family("monospace")
                                    .child(content_path.clone()),
                            ),
                    )
                })
                .footer({
                    let view = view.clone();
                    let reload_document_id = document_id.clone();
                    let force_document_id = document_id.clone();
                    let cancel_document_id = document_id.clone();
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("document-conflict-cancel")
                                .ghost()
                                .label(t!("cancel").to_string())
                                .on_click({
                                    let view = view.clone();
                                    move |_, window, cx| {
                                        view.update(cx, |this, cx| {
                                            this.cancel_document_conflict(&cancel_document_id, cx);
                                        });
                                        window.close_dialog(cx);
                                    }
                                }),
                        )
                        .when(!remote_deleted, |this| {
                            this.child(
                                Button::new("document-conflict-reload")
                                    .secondary()
                                    .label(t!("document_reload").to_string())
                                    .on_click({
                                        let view = view.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.reload_document(
                                                    reload_document_id.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                            window.close_dialog(cx);
                                        }
                                    }),
                            )
                        })
                        .child(
                            Button::new("document-conflict-overwrite")
                                .danger()
                                .label(if remote_deleted {
                                    t!("document_recreate").to_string()
                                } else {
                                    t!("document_force_overwrite").to_string()
                                })
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.force_overwrite_document(
                                            force_document_id.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                })
        });
    }

    pub(crate) fn show_ssh_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_dialog.is_some() {
            return;
        }
        self.active_dialog = Some(crate::app::DialogKind::NewSsh);

        if let Some(id) = &self.editing_session_id {
            if let Some(session) = self.config.get(id) {
                self.session_protocol = session.protocol.clone();
            }
        } else {
            self.session_protocol = "ssh".to_string();
        }

        let initial_is_serial = self.session_protocol == "serial";
        let view = cx.entity();
        let session_name_input = self.session_name_input.clone();
        let host_input = self.host_input.clone();
        let focus_host_input = host_input.clone();
        let port_input = self.port_input.clone();
        let user_input = self.user_input.clone();
        let password_input = self.password_input.clone();
        let key_path_input = self.key_path_input.clone();
        let key_inline_input = self.key_inline_input.clone();
        let passphrase_input = self.passphrase_input.clone();
        let proxy_host_input = self.proxy_host_input.clone();
        let proxy_port_input = self.proxy_port_input.clone();
        let proxy_user_input = self.proxy_user_input.clone();
        let proxy_password_input = self.proxy_password_input.clone();
        let baud_rate_input = self.baud_rate_input.clone();

        window.open_dialog(cx, move |dialog: Dialog, _window, _cx| {
            dialog
                .title(if initial_is_serial { t!("new_serial_connection") } else { t!("new_ssh_connection") })
                .w(px(520.))
                .overlay_closable(true)
                .on_close({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.active_dialog = None;
                            cx.notify();
                        });
                    }
                })
                .content({
                    let view = view.clone();
                    let session_name_input = session_name_input.clone();
                    let host_input = host_input.clone();
                    let port_input = port_input.clone();
                    let user_input = user_input.clone();
                    let password_input = password_input.clone();
                    let key_path_input = key_path_input.clone();
                    let key_inline_input = key_inline_input.clone();
                    let passphrase_input = passphrase_input.clone();
                    let proxy_host_input = proxy_host_input.clone();
                    let proxy_port_input = proxy_port_input.clone();
                    let proxy_user_input = proxy_user_input.clone();
                    let proxy_password_input = proxy_password_input.clone();
                    let baud_rate_input = baud_rate_input.clone();
                    move |content, window, cx| {
                        let auth_method = view.read(cx).ssh_auth_method;
                        let is_password = auth_method == AuthMethod::Password;
                        let is_key = auth_method == AuthMethod::Key;
                        let is_config = auth_method == AuthMethod::Config;
                        let is_editing = view.read(cx).editing_session_id.is_some();
                        let proxy_type = view.read(cx).ssh_proxy_type.clone();
                        let normalized_proxy_type = proxy_type.trim().to_ascii_lowercase();
                        let proxy_policy = session_proxy_policy(&normalized_proxy_type);
                        let show_proxy_fields = proxy_policy == SessionProxyPolicy::Custom;
                        let socks5_selected =
                            matches!(normalized_proxy_type.as_str(), "socks5" | "socks5h");
                        let ssh_config_selected = view.read(cx).ssh_config_selected.is_some();
                        let editing_session_auth = view.read(cx).editing_session_id.as_deref()
                            .and_then(|id| view.read(cx).config.get(id))
                            .map(|session| session.auth);
                        let can_submit = can_submit_ssh_session(
                            auth_method,
                            ssh_config_selected,
                            editing_session_auth,
                        );
                        let protocol = view.read(cx).session_protocol.clone();
                        let is_ssh = protocol == "ssh";
                        let is_serial = protocol == "serial";
                        content.child(
                            v_flex()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("proto-ssh")
                                                .label("SSH")
                                                .when(is_ssh, |button| button.primary())
                                                .on_click(window.listener_for(
                                                    &view,
                                                    |this, _, window, cx| {
                                                        this.set_session_protocol("ssh".to_string(), cx);
                                                        Self::set_input_value(&this.port_input, "22", window, cx);
                                                    },
                                                )),
                                        )
                                        .child(
                                            Button::new("proto-serial")
                                                .label("Serial")
                                                .when(is_serial, |button| button.primary())
                                                .on_click(window.listener_for(
                                                    &view,
                                                    |this, _, _window, cx| {
                                                        this.set_session_protocol("serial".to_string(), cx);
                                                    },
                                                )),
                                        ),
                                )
                                .when(is_serial, |this| {
                                    this.child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t!("session_name").to_string()))
                                            .child(Input::new(&session_name_input).tab_index(0))
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t!("serial_port").to_string()))
                                            .child(Input::new(&host_input).tab_index(1))
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t!("baud_rate").to_string()))
                                            .child(Input::new(&baud_rate_input).tab_index(2))
                                    )
                                })
                                .when(is_ssh, |this| {
                                    this.child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Button::new("ssh-auth-password")
                                                    .label(t!("password").to_string())
                                                    .when(is_password, |button| button.primary())
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, _, cx| {
                                                            this.set_ssh_auth_method(
                                                                AuthMethod::Password,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("ssh-auth-key")
                                                    .label(t!("key").to_string())
                                                    .when(is_key, |button| button.primary())
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, _, cx| {
                                                            this.set_ssh_auth_method(
                                                                AuthMethod::Key,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("ssh-auth-config")
                                                    .label(t!("ssh_config").to_string())
                                                    .when(is_config, |button| button.primary())
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, _, cx| {
                                                            this.set_ssh_auth_method(
                                                                AuthMethod::Config,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            ),
                                    )
                                    .when(!is_config, |this| {
                                        this.child(Input::new(&session_name_input).tab_index(0))
                                            .child(Input::new(&host_input).tab_index(1))
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(
                                                        Input::new(&port_input).w(px(96.)).tab_index(2),
                                                    )
                                                    .child(
                                                        Input::new(&user_input).flex_1().tab_index(3),
                                                    ),
                                            )
                                    })
                                    .when(is_password, |this| {
                                        this.child(
                                            Input::new(&password_input).mask_toggle().tab_index(4),
                                        )
                                    })
                                    .when(is_key, |this| {
                                        this.child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .cursor_pointer()
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            window.listener_for(
                                                                &view,
                                                                |this, _, window, cx| {
                                                                    this.pick_ssh_key_path(window, cx);
                                                                },
                                                            ),
                                                        )
                                                        .child(
                                                            Input::new(&key_path_input).tab_index(4),
                                                        ),
                                                )
                                                .child(
                                                    Button::new("clear-key-path")
                                                        .ghost()
                                                        .icon(IconName::Close)
                                                        .on_click(window.listener_for(
                                                            &view,
                                                            |this, _, window, cx| {
                                                                Self::set_input_value(
                                                                    &this.key_path_input,
                                                                    "",
                                                                    window,
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                ),
                                        )
                                        .child(Input::new(&key_inline_input).h(px(128.)).tab_index(5))
                                        .child(Input::new(&passphrase_input).mask_toggle().tab_index(6))
                                    })
                                    .when(is_config, |this| {
                                        let entries = view.read(cx).ssh_config_entries.clone();
                                        let selected = view.read(cx).ssh_config_selected;
                                        let theme = cx.theme();
                                        if entries.is_empty() {
                                            this.child(
                                                div()
                                                    .text_sm()
                                                    .text_color(theme.muted_foreground)
                                                    .child(t!("ssh_config_empty").to_string()),
                                            )
                                        } else {
                                            this.child(
                                                div()
                                                    .h(px(192.))
                                                    .id("ssh-config-list")
                                                    .track_scroll(
                                                        &view.read(cx).connection_scroll_handle,
                                                    )
                                                    .overflow_y_scroll()
                                                    .border_1()
                                                    .border_color(theme.border)
                                                    .rounded_md()
                                                    .children(entries.iter().enumerate().map(
                                                        |(i, entry)| {
                                                            let is_selected = selected == Some(i);
                                                            let label = if entry.user.is_empty() {
                                                                format!(
                                                                    "{}:{}",
                                                                    entry.hostname, entry.port
                                                                )
                                                            } else {
                                                                format!(
                                                                    "{}@{}:{}",
                                                                    entry.user,
                                                                    entry.hostname,
                                                                    entry.port
                                                                )
                                                            };
                                                            let alias_label =
                                                                if entry.host_alias == entry.hostname {
                                                                    String::new()
                                                                } else {
                                                                    format!(" ({})", entry.host_alias)
                                                                };
                                                            let view_clone = view.clone();
                                                            div()
                                                                .id(("ssh-config-entry", i))
                                                                .px_2()
                                                                .py_1()
                                                                .when(is_selected, |el| {
                                                                    el.bg(theme.selection)
                                                                })
                                                                .cursor_pointer()
                                                                .hover(|el| el.bg(theme.selection))
                                                                .text_sm()
                                                                .child(format!("{label}{alias_label}"))
                                                                .on_click(window.listener_for(
                                                                    &view_clone,
                                                                    move |this, _, window, cx| {
                                                                        this.select_ssh_config_entry(
                                                                            i, window, cx,
                                                                        );
                                                                    },
                                                                ))
                                                        },
                                                    )),
                                            )
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .child(t!("proxy").to_string()),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Button::new("proxy-inherit")
                                                    .label(t!("proxy_none").to_string())
                                                    .when(proxy_policy == SessionProxyPolicy::Inherit, |button| {
                                                        button.primary()
                                                    })
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, _, cx| {
                                                            this.set_ssh_proxy_policy(
                                                                SessionProxyPolicy::Inherit,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("proxy-direct")
                                                    .label(t!("proxy_direct").to_string())
                                                    .when(proxy_policy == SessionProxyPolicy::Direct, |button| {
                                                        button.primary()
                                                    })
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, _, cx| {
                                                            this.set_ssh_proxy_policy(
                                                                SessionProxyPolicy::Direct,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("proxy-custom")
                                                    .label(t!("proxy_use").to_string())
                                                    .when(proxy_policy == SessionProxyPolicy::Custom, |button| {
                                                        button.primary()
                                                    })
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, _, cx| {
                                                            this.set_ssh_proxy_policy(
                                                                SessionProxyPolicy::Custom,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            ),
                                    )
                                    .when(show_proxy_fields, |this| {
                                        this.child(
                                            div()
                                                .text_sm()
                                                .child(t!("proxy_protocol").to_string()),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    Button::new("proxy-type-socks5")
                                                        .label("SOCKS5")
                                                        .when(socks5_selected, |button| button.primary())
                                                        .on_click(window.listener_for(
                                                            &view,
                                                            |this, _, _, cx| {
                                                                this.set_ssh_proxy_type(
                                                                    "socks5".to_string(),
                                                                    cx,
                                                                )
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    Button::new("proxy-type-http")
                                                        .label("HTTP")
                                                        .when(normalized_proxy_type == "http", |button| button.primary())
                                                        .on_click(window.listener_for(
                                                            &view,
                                                            |this, _, _, cx| {
                                                                this.set_ssh_proxy_type(
                                                                    "http".to_string(),
                                                                    cx,
                                                                )
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    Button::new("proxy-type-https")
                                                        .label("HTTPS")
                                                        .when(normalized_proxy_type == "https", |button| button.primary())
                                                        .on_click(window.listener_for(
                                                            &view,
                                                            |this, _, _, cx| {
                                                                this.set_ssh_proxy_type(
                                                                    "https".to_string(),
                                                                    cx,
                                                                )
                                                            },
                                                        )),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(Input::new(&proxy_host_input).flex_1())
                                                .child(Input::new(&proxy_port_input).w(px(96.))),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(Input::new(&proxy_user_input).flex_1())
                                                .child(Input::new(&proxy_password_input).flex_1()),
                                        )
                                    })
                                })
                                .child(
                                    h_flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            Button::new("connect-ssh-cancel")
                                                .label(t!("cancel").to_string())
                                                .on_click(window.listener_for(
                                                    &view,
                                                    |this, _, window, cx| {
                                                        this.active_dialog = None;
                                                        window.close_dialog(cx);
                                                        cx.notify();
                                                    },
                                                )),
                                        )
                                        .when(can_submit, |this| {
                                            this.child(
                                                Button::new("connect-ssh-confirm")
                                                    .primary()
                                                    .label(if is_editing {
                                                        t!("save")
                                                    } else {
                                                        t!("connect")
                                                    })
                                                    .on_click(window.listener_for(
                                                        &view,
                                                        |this, _, window, cx| {
                                                            this.connect_ssh(window, cx)
                                                        },
                                                    )),
                                            )
                                        }),
                                ),
                        )
                    }
                })
        });
        window.defer(cx, move |window, cx| {
            window.focus(&focus_host_input.read(cx).focus_handle(cx), cx);
        });
    }

    pub(crate) fn show_session_folder_dialog(
        &mut self,
        folder_id: Option<String>,
        initial_session_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_dialog.is_some() {
            return;
        }

        let existing_folder = folder_id
            .as_deref()
            .and_then(|id| {
                self.config
                    .session_folders()
                    .iter()
                    .find(|folder| folder.id == id)
            })
            .cloned();
        self.active_dialog = Some(crate::app::DialogKind::SessionFolder);
        self.editing_session_folder_id = existing_folder.as_ref().map(|folder| folder.id.clone());
        self.editing_session_folder_members = existing_folder
            .as_ref()
            .map(|folder| folder.session_ids.iter().cloned().collect())
            .unwrap_or_default();
        if existing_folder.is_none()
            && let Some(session_id) = initial_session_id
            && self.config.get(&session_id).is_some()
        {
            self.editing_session_folder_members.insert(session_id);
        }
        Self::set_input_value(
            &self.session_folder_name_input,
            existing_folder
                .as_ref()
                .map(|folder| folder.name.clone())
                .unwrap_or_default(),
            window,
            cx,
        );

        let is_editing = existing_folder.is_some();
        let sessions = self.config.sessions().to_vec();
        let folder_name_input = self.session_folder_name_input.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog: Dialog, _window, _| {
            dialog
                .title(if is_editing {
                    t!("edit_session_folder").to_string()
                } else {
                    t!("new_session_folder").to_string()
                })
                .w(px(480.))
                .on_close({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.active_dialog = None;
                            this.editing_session_folder_id = None;
                            this.editing_session_folder_members.clear();
                            cx.notify();
                        });
                    }
                })
                .content({
                    let view = view.clone();
                    let folder_name_input = folder_name_input.clone();
                    let sessions = sessions.clone();
                    move |content, window, cx| {
                        let selected_members = view.read(cx).editing_session_folder_members.clone();
                        content
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(div().text_sm().child(t!("folder_name").to_string()))
                                    .child(Input::new(&folder_name_input).w_full().tab_index(0)),
                            )
                            .child(
                                v_flex()
                                    .mt_4()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .child(t!("sessions_in_folder").to_string()),
                                    )
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .max_h(px(260.))
                                            .overflow_y_scrollbar()
                                            .border_1()
                                            .border_color(cx.theme().border)
                                            .rounded_sm()
                                            .children(sessions.iter().enumerate().map(
                                                |(index, session)| {
                                                    let session_id = session.id.clone();
                                                    let checked = selected_members.contains(&session_id);
                                                    let detail = format!("{}@{}:{}", session.user, session.host, session.port);
                                                    h_flex()
                                                        .id(("folder-session", index))
                                                        .w_full()
                                                        .items_center()
                                                        .gap_2()
                                                        .px_3()
                                                        .py_2()
                                                        .when(index > 0, |row| {
                                                            row.border_t_1().border_color(cx.theme().border)
                                                        })
                                                        .child(
                                                            Checkbox::new(format!("folder-session-check-{}", session.id))
                                                                .checked(checked)
                                                                .on_click(window.listener_for(&view, move |this, checked, _, cx| {
                                                                    if *checked {
                                                                        this.editing_session_folder_members.insert(session_id.clone());
                                                                    } else {
                                                                        this.editing_session_folder_members.remove(&session_id);
                                                                    }
                                                                    cx.notify();
                                                                })),
                                                        )
                                                        .child(
                                                            v_flex()
                                                                .min_w(px(0.))
                                                                .gap_1()
                                                                .child(div().text_sm().child(session.name.clone()))
                                                                .child(
                                                                    div()
                                                                        .text_size(rems(0.75))
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .child(detail),
                                                                ),
                                                        )
                                                },
                                            )),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .mt_5()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("session-folder-cancel")
                                            .ghost()
                                            .label(t!("cancel").to_string())
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        Button::new("session-folder-save")
                                            .primary()
                                            .label(t!("save").to_string())
                                            .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                this.save_session_folder(window, cx);
                                            })),
                                    ),
                            )
                    }
                })
        });
    }

    pub(crate) fn show_selector_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_dialog.is_some() {
            return;
        }
        self.active_dialog = Some(crate::app::DialogKind::SessionSelector);

        let view = cx.entity();
        let selector_focus_handle = self.selector_focus_handle.clone();
        let deferred_selector_focus_handle = selector_focus_handle.clone();
        let sessions = self.config.sessions().to_vec();
        let active_session_id = self.active_session_id().map(ToOwned::to_owned);
        self.selector_selection = self.default_selector_index();
        window.open_dialog(cx, move |dialog: Dialog, _window, _| {
            dialog
                .title(t!("open_session").to_string())
                .w(px(520.))
                .on_close({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.active_dialog = None;
                            cx.notify();
                        });
                    }
                })
                .on_ok({
                    let view = view.clone();
                    move |_, window, cx| {
                        view.update(cx, |this, cx| {
                            this.activate_selector_selection(window, cx);
                        });
                        false
                    }
                })
                .content({
                    let view = view.clone();
                    let sessions = sessions.clone();
                    let _active_session_id = active_session_id.clone();
                    let selector_focus_handle = selector_focus_handle.clone();
                    move |content, window, _cx| {
                        let selected_index = view.read(_cx).selector_selection;
                        let scroll_handle = view.read(_cx).selector_scroll_handle.clone();
                        content.child(
                            v_flex()
                                .track_focus(&selector_focus_handle)
                                .on_key_down(window.listener_for(
                                    &view,
                                    |this, event, window, cx| {
                                        this.on_selector_key_down(event, window, cx)
                                    },
                                ))
                                .gap_2()
                                .child(
                                    div()
                                        .w_full()
                                        .p_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(if selected_index == 0 {
                                            _cx.theme().primary
                                        } else {
                                            _cx.theme().border
                                        })
                                        .bg(if selected_index == 0 {
                                            _cx.theme().tab_active
                                        } else {
                                            _cx.theme().muted
                                        })
                                        .cursor_pointer()
                                        .hover(|this| this.bg(_cx.theme().secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            window.listener_for(&view, |this, _, window, cx| {
                                                this.active_dialog = None;
                                                this.open_local(cx);
                                                window.close_dialog(cx);
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_size(rems(1.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(t!("local_terminal")),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(rems(0.917))
                                                        .text_color(_cx.theme().muted_foreground)
                                                        .child(t!("open_local_shell_tab")),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .p_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(if selected_index == 1 {
                                            _cx.theme().primary
                                        } else {
                                            _cx.theme().border
                                        })
                                        .bg(if selected_index == 1 {
                                            _cx.theme().tab_active
                                        } else {
                                            _cx.theme().muted
                                        })
                                        .cursor_pointer()
                                        .hover(|this| this.bg(_cx.theme().secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            window.listener_for(&view, |this, _, window, cx| {
                                                this.active_dialog = None;
                                                window.close_dialog(cx);
                                                this.open_new_ssh_dialog(window, cx);
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_size(rems(1.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(t!("new_ssh_connection")),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(rems(0.917))
                                                        .text_color(_cx.theme().muted_foreground)
                                                        .child(t!("create_or_edit_ssh_session")),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .max_h(px(320.))
                                        .size_full()
                                        .child(
                                            v_flex()
                                                .size_full()
                                                .id("selector-scroll-view")
                                                .track_scroll(&scroll_handle)
                                                .overflow_y_scroll()
                                                .gap_2()
                                                .children(
                                                    sessions.clone().into_iter().enumerate().map(
                                                        |(ix, session)| {
                                                            let connect_id = session.id.clone();
                                                            let is_selected =
                                                                selected_index == ix + 2;
                                                            let name = session.name.clone();
                                                            let detail = if session.protocol
                                                                == "serial"
                                                            {
                                                                format!(
                                                                    "Serial: {}@{}",
                                                                    session.host, session.baud_rate
                                                                )
                                                            } else {
                                                                format!(
                                                                    "{}@{}:{}",
                                                                    session.user,
                                                                    session.host,
                                                                    session.port
                                                                )
                                                            };
                                                            div()
                                                    .id(("selector-open", ix))
                                                    .w_full()
                                                    .p_2()
                                                    .rounded_md()
                                                    .border_1()
                                                    .border_color(if is_selected {
                                                        _cx.theme().primary
                                                    } else {
                                                        _cx.theme().border
                                                    })
                                                    .bg(if is_selected {
                                                        _cx.theme().tab_active
                                                    } else {
                                                        _cx.theme().muted
                                                    })
                                                    .cursor_pointer()
                                                    .hover(|this| this.bg(_cx.theme().secondary))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        window.listener_for(
                                                            &view,
                                                            move |this, _, window, cx| {
                                                                this.active_dialog = None;
                                                                this.connect_saved_session(
                                                                    connect_id.clone(),
                                                                    window,
                                                                    cx,
                                                                );
                                                                window.close_dialog(cx);
                                                                cx.notify();
                                                            },
                                                        ),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .gap_1()
                                                            .child(
                                                                div()
                                                                    .text_size(rems(1.0))
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .child(name),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(rems(0.917))
                                                                    .text_color(
                                                                        _cx.theme()
                                                                            .muted_foreground,
                                                                    )
                                                                    .child(detail),
                                                            ),
                                                    )
                                                        },
                                                    ),
                                                ),
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
                                                    &scroll_handle,
                                                )
                                                .id("selector-scrollbar")
                                                .axis(
                                                    gpui_component::scroll::ScrollbarAxis::Vertical,
                                                )
                                                .scrollbar_show(
                                                    gpui_component::scroll::ScrollbarShow::Always,
                                                ),
                                            ),
                                        ),
                                ),
                        )
                    }
                })
        });
        window.defer(cx, move |window, cx| {
            window.focus(&deferred_selector_focus_handle, cx);
        });
    }
    pub(crate) fn show_transfers_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_dialog.is_some() {
            return;
        }
        self.active_dialog = Some(crate::app::DialogKind::Transfers);

        let view = cx.entity();
        window.open_dialog(cx, move |dialog: Dialog, _window, _| {
            dialog
                .w(px(600.))
                .close_button(false)
                .on_close({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.active_dialog = None;
                            cx.notify();
                        });
                    }
                })
                .content({
                    let view = view.clone();
                    move |content, window, cx| {
                        let can_clear = view.read(cx).transfers.iter().any(|t| {
                            !matches!(
                                t.state,
                                crate::terminal::TransferState::Running
                                    | crate::terminal::TransferState::Paused
                            )
                        });

                        let clear_btn = if can_clear {
                            Some(
                                Button::new("clear_transfers_btn")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Delete)
                                    .label(t!("clear_transfers").to_string())
                                    .on_click(window.listener_for(&view, |this, _, _, cx| {
                                        this.transfers.retain(|t| {
                                            matches!(
                                                t.state,
                                                crate::terminal::TransferState::Running
                                                    | crate::terminal::TransferState::Paused
                                            )
                                        });
                                        this.config.set_transfers(this.transfers.clone());
                                        cx.notify();
                                    })),
                            )
                        } else {
                            None
                        };

                        let header = h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(t!("transfers").to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .ml_2()
                                            .child(t!("transfers_limit").to_string()),
                                    ),
                            )
                            .child(
                                h_flex().gap_2().children(clear_btn).child(
                                    Button::new("close_dialog")
                                        .small()
                                        .ghost()
                                        .icon(IconName::Close)
                                        .on_click(window.listener_for(
                                            &view,
                                            |this, _, window, cx| {
                                                this.active_dialog = None;
                                                window.close_dialog(cx);
                                                cx.notify();
                                            },
                                        )),
                                ),
                            );

                        let mut transfers = view.read(cx).transfers.clone();
                        transfers.sort_by_key(|t| match t.state {
                            crate::terminal::TransferState::Running
                            | crate::terminal::TransferState::Paused => 0,
                            _ => 1,
                        });

                        if transfers.is_empty() {
                            return content.child(
                                v_flex().gap_2().child(header).child(
                                    div()
                                        .p_4()
                                        .text_center()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t!("no_transfers_yet").to_string()),
                                ),
                            );
                        }
                        let list = v_flex().gap_2().children(transfers.into_iter().map(|t| {
                            let (icon, _color) = match t.info.kind {
                                crate::terminal::TransferType::Upload => {
                                    (IconName::ArrowUp, cx.theme().primary)
                                }
                                crate::terminal::TransferType::Download => {
                                    (IconName::ArrowDown, cx.theme().success)
                                }
                            };

                            let (status_text, actions) = match t.state {
                                crate::terminal::TransferState::Running => {
                                    let percent = t
                                        .total
                                        .map(|tot| {
                                            (t.transferred as f64 / tot as f64 * 100.0)
                                                .clamp(0.0, 100.0)
                                        })
                                        .unwrap_or(0.0);
                                    let txt = if let Some(tot) = t.total {
                                        format!(
                                            "{:.1}% ({}/{})",
                                            percent,
                                            format_bytes(t.transferred),
                                            format_bytes(tot)
                                        )
                                    } else {
                                        match t.info.kind {
                                            crate::terminal::TransferType::Upload => {
                                                format!("{}...", t!("uploading"))
                                            }
                                            crate::terminal::TransferType::Download => {
                                                format!("{}...", t!("downloading"))
                                            }
                                        }
                                    };
                                    let btn_pause = Button::new(SharedString::from(format!(
                                        "pause-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Pause)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, _| {
                                            if let Some(handle) = this.active_sftp_handle() {
                                                handle.pause_transfer(id.clone());
                                            }
                                        }
                                    }));
                                    let btn_cancel = Button::new(SharedString::from(format!(
                                        "cancel-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, _| {
                                            if let Some(handle) = this.active_sftp_handle() {
                                                handle.cancel_transfer(id.clone());
                                            }
                                        }
                                    }));
                                    (txt, h_flex().gap_1().child(btn_pause).child(btn_cancel))
                                }
                                crate::terminal::TransferState::Paused => {
                                    let txt = t!("paused").to_string();
                                    let btn_resume = Button::new(SharedString::from(format!(
                                        "resume-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Play)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, _| {
                                            if let Some(handle) = this.active_sftp_handle() {
                                                handle.resume_transfer(id.clone());
                                            }
                                        }
                                    }));
                                    let btn_cancel = Button::new(SharedString::from(format!(
                                        "cancel-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, _| {
                                            if let Some(handle) = this.active_sftp_handle() {
                                                handle.cancel_transfer(id.clone());
                                            }
                                        }
                                    }));
                                    (txt, h_flex().gap_1().child(btn_resume).child(btn_cancel))
                                }
                                crate::terminal::TransferState::Interrupted(ref reason) => {
                                    let txt = format!("{}: {}", t!("interrupted"), reason);
                                    let btn_remove = Button::new(SharedString::from(format!(
                                        "remove-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, cx| {
                                            this.remove_transfer(&id, cx);
                                        }
                                    }));
                                    (txt, h_flex().gap_1().child(btn_remove))
                                }
                                crate::terminal::TransferState::Completed => {
                                    let txt = t!("completed").to_string();
                                    let mut actions = h_flex().gap_1();
                                    if matches!(
                                        t.info.kind,
                                        crate::terminal::TransferType::Download
                                    ) {
                                        let btn_folder = Button::new(SharedString::from(format!(
                                            "folder-{}",
                                            t.info.id
                                        )))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Folder)
                                        .on_click({
                                            let target = t.info.target.clone();
                                            move |_, _, _| {
                                                open_completed_download_target(&target);
                                            }
                                        });
                                        actions = actions.child(btn_folder);
                                    }
                                    let btn_remove = Button::new(SharedString::from(format!(
                                        "remove-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, cx| {
                                            this.remove_transfer(&id, cx);
                                        }
                                    }));
                                    actions = actions.child(btn_remove);
                                    (txt, actions)
                                }
                                crate::terminal::TransferState::Failed(ref err) => {
                                    let txt = format!("{}: {}", t!("failed"), err);
                                    let btn_remove = Button::new(SharedString::from(format!(
                                        "remove-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, cx| {
                                            this.remove_transfer(&id, cx);
                                        }
                                    }));
                                    (txt, h_flex().gap_1().child(btn_remove))
                                }
                                crate::terminal::TransferState::Zombie(ref reason) => {
                                    let txt = format!("{}: {}", t!("zombie"), reason);
                                    let btn_remove = Button::new(SharedString::from(format!(
                                        "remove-{}",
                                        t.info.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(window.listener_for(&view, {
                                        let id = t.info.id.clone();
                                        move |this, _, _, cx| {
                                            this.remove_transfer(&id, cx);
                                        }
                                    }));
                                    (txt, h_flex().gap_1().child(btn_remove))
                                }
                            };

                            let percent = match t.state {
                                crate::terminal::TransferState::Completed => 100.0,
                                _ => t
                                    .total
                                    .map(|tot| t.transferred as f64 / tot as f64 * 100.0)
                                    .unwrap_or(0.0),
                            };

                            v_flex()
                                .gap_1()
                                .p_2()
                                .rounded_md()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().muted)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "icon-{}",
                                                t.info.id
                                            )))
                                            .icon(icon)
                                            .ghost()
                                            .small()
                                            .disabled(true),
                                        )
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .overflow_hidden()
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(cx.theme().foreground)
                                                        .overflow_hidden()
                                                        .child(t.info.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(cx.theme().muted_foreground)
                                                        .overflow_hidden()
                                                        .child(format!(
                                                            "{}: {}",
                                                            t!("session"),
                                                            t.tab_title
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(status_text.clone()),
                                                ),
                                        )
                                        .child(actions),
                                )
                                .when(
                                    matches!(
                                        t.state,
                                        crate::terminal::TransferState::Running
                                            | crate::terminal::TransferState::Paused
                                    ),
                                    |this| {
                                        this.child(
                                            Progress::new(format!("progress-{}", t.info.id))
                                                .with_size(px(4.))
                                                .value(percent as f32)
                                                .color(cx.theme().primary)
                                                .w_full(),
                                        )
                                    },
                                )
                        }));

                        let scroll_handle = window
                            .use_keyed_state("transfers-scroll", cx, |_, _| {
                                gpui::ScrollHandle::default()
                            })
                            .read(cx)
                            .clone();

                        content.child(
                            v_flex().gap_2().child(header).child(
                                div()
                                    .w_full()
                                    .relative()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_h(px(400.))
                                            .flex_col()
                                            .id("transfers-scroll-view")
                                            .track_scroll(&scroll_handle)
                                            .overflow_y_scroll()
                                            .pr(px(14.))
                                            .child(list),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .right_0()
                                            .bottom_0()
                                            .w(px(16.))
                                            .child(
                                                Scrollbar::vertical(&scroll_handle)
                                                    .scrollbar_show(ScrollbarShow::Always),
                                            ),
                                    ),
                            ),
                        )
                    }
                })
        });
    }
    pub(crate) fn show_delete_confirm_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        let selected_entries = self
            .active_sftp()
            .map(|s| s.selected_entries.clone())
            .unwrap_or_default();
        if selected_entries.is_empty() {
            return;
        }

        let has_system_path = selected_entries.iter().any(|path| {
            let p = path.as_str();
            p.starts_with("/bin/")
                || p == "/bin"
                || p.starts_with("/etc/")
                || p == "/etc"
                || p.starts_with("/usr/")
                || p == "/usr"
                || p.starts_with("/var/")
                || p == "/var"
                || p.starts_with("/sys/")
                || p == "/sys"
                || p.starts_with("/dev/")
                || p == "/dev"
                || p.starts_with("/boot/")
                || p == "/boot"
                || p.starts_with("/lib/")
                || p == "/lib"
                || p.starts_with("/opt/")
                || p == "/opt"
                || p.starts_with("/run/")
                || p == "/run"
                || p.starts_with("/sbin/")
                || p == "/sbin"
        });

        window.open_dialog(cx, move |dialog: Dialog, _window, _| {
            dialog
                .title(t!("confirm_delete").to_string())
                .w(px(500.))
                .keyboard(false)
                .on_ok({
                    let view = view.clone();
                    let paths_to_delete: Vec<String> =
                        selected_entries.clone().into_iter().collect();
                    move |_, window, cx| {
                        view.update(cx, |this, cx| {
                            this.delete_sftp_paths(paths_to_delete.clone(), cx);
                        });
                        window.close_dialog(cx);
                        true
                    }
                })
                .content({
                    let view = view.clone();
                    move |content, _window, cx| {
                        let scroll_handle = view.read(cx).sftp_delete_scroll_handle.clone();
                        let selected_paths: Vec<String> = view
                            .read(cx)
                            .active_sftp()
                            .map(|s| s.selected_entries.clone().into_iter().collect())
                            .unwrap_or_default();

                        let warning_block = if has_system_path {
                            Some(
                                div()
                                    .w_full()
                                    .p_3()
                                    .mb_3()
                                    .rounded_md()
                                    .bg(gpui::rgba(0xff00001a))
                                    .border_1()
                                    .border_color(gpui::rgba(0xff000080))
                                    .child(
                                        div()
                                            .text_color(gpui::rgba(0xff0000ff))
                                            .font_weight(FontWeight::BOLD)
                                            .child(t!("system_path_warning").to_string()),
                                    ),
                            )
                        } else {
                            None
                        };

                        let paths_list = div()
                            .relative()
                            .max_h(px(200.))
                            .w_full()
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().background)
                            .rounded_md()
                            .child(
                                v_flex()
                                    .id("delete-scroll-view")
                                    .size_full()
                                    .track_scroll(&scroll_handle)
                                    .overflow_y_scroll()
                                    .p_2()
                                    .gap_1()
                                    .children(selected_paths.into_iter().map(|path| {
                                        div()
                                            .text_size(rems(0.917))
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(path)
                                    })),
                            )
                            .child(
                                div().absolute().top_0().bottom_0().right_0().child(
                                    gpui_component::scroll::Scrollbar::vertical(&scroll_handle)
                                        .scrollbar_show(
                                            gpui_component::scroll::ScrollbarShow::Always,
                                        ),
                                ),
                            );

                        content.child(
                            v_flex()
                                .w_full()
                                .gap_2()
                                .children(warning_block)
                                .child(
                                    div().text_size(rems(1.0)).mb_2().child(
                                        t!(
                                            "confirm_delete_desc",
                                            count = view
                                                .read(cx)
                                                .active_sftp()
                                                .map(|s| s.selected_entries.len())
                                                .unwrap_or(0)
                                        )
                                        .to_string(),
                                    ),
                                )
                                .child(paths_list),
                        )
                    }
                })
                .footer({
                    let view = view.clone();
                    let paths_to_delete: Vec<String> =
                        selected_entries.clone().into_iter().collect();
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel")
                                .ghost()
                                .label(t!("cancel").to_string())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("confirm")
                                .danger()
                                .label(t!("confirm").to_string())
                                .on_click({
                                    let view = view.clone();
                                    move |_, window, cx| {
                                        view.update(cx, |this, cx| {
                                            this.delete_sftp_paths(paths_to_delete.clone(), cx);
                                        });
                                        window.close_dialog(cx);
                                    }
                                }),
                        )
                })
        });
    }
    pub(crate) fn show_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_dialog.is_some() {
            return;
        }
        self.active_dialog = Some(crate::app::DialogKind::Settings);

        let view = cx.entity();

        // Unbind all workspace keys so they don't interfere with keybinding recording
        crate::app::keybinding_recorder::unbind_all_workspace_keys(cx, &self.config);
        self.keybinds_suspended = true;

        window.open_dialog(cx, move |dialog: Dialog, _window, _| {
            dialog
                .title(t!("settings").to_string())
                .w(px(840.))
                .h(px(560.))
                .on_close({
                    let view = view.clone();
                    move |_, _window, cx| {
                        // Re-register all workspace keys when closing settings
                        view.update(cx, |this, cx| {
                            this.active_dialog = None;
                            this.keybinds_suspended = false;
                            this.recording_action = None;
                            this.keybind_error = None;
                            crate::app::keybinding_recorder::bind_workspace_keys_from_config(
                                cx,
                                &this.config,
                            );
                            cx.notify();
                        });
                    }
                })
                .content({
                    let view = view.clone();
                    move |content, _window, cx| {
                        use gpui_component::setting::{Settings, SettingPage, SettingGroup, SettingItem, SettingField};
                        use gpui::IntoElement;
                        let view_clone_for_general = view.clone();
                        let sync_endpoint_input = view.read(cx).sync_endpoint_input.clone();
                        let sync_username_input = view.read(cx).sync_username_input.clone();
                        let sync_webdav_password_input = view.read(cx).sync_webdav_password_input.clone();
                        let sync_s3_endpoint_input = view.read(cx).sync_s3_endpoint_input.clone();
                        let sync_s3_region_input = view.read(cx).sync_s3_region_input.clone();
                        let sync_s3_bucket_input = view.read(cx).sync_s3_bucket_input.clone();
                        let sync_s3_object_key_input = view.read(cx).sync_s3_object_key_input.clone();
                        let sync_s3_access_key_input = view.read(cx).sync_s3_access_key_input.clone();
                        let sync_s3_secret_key_input = view.read(cx).sync_s3_secret_key_input.clone();
                        let sync_s3_session_token_input = view.read(cx).sync_s3_session_token_input.clone();
                        let sync_encryption_password_input = view.read(cx).sync_encryption_password_input.clone();

                        let focus_handle = view.read(cx).focus_handle.clone();

                        content.child(
                            div()
                                .flex()
                                .flex_col()
                                .size_full()
                                .track_focus(&focus_handle)
                                .on_key_down({
                                    let view = view.clone();
                                    move |ev: &gpui::KeyDownEvent, window, cx| {
                                        view.update(cx, |this, cx| {
                                            let Some(action) = this.recording_action.clone() else {
                                                return;
                                            };

                                            window.prevent_default();
                                            cx.stop_propagation();

                                            if ev.keystroke.key == "escape" {
                                                this.recording_action = None;
                                                cx.notify();
                                                return;
                                            }

                                            if ev.keystroke.key == "backspace"
                                                && !ev.keystroke.modifiers.control
                                                && !ev.keystroke.modifiers.alt
                                                && !ev.keystroke.modifiers.shift
                                                && !ev.keystroke.modifiers.platform
                                                && !ev.keystroke.modifiers.function
                                            {
                                                this.recording_action = None;
                                                this.keybind_error = None;
                                                this.config.set_key_binding(&action, "none");
                                                this.save_preferences_background();
                                                cx.notify();
                                                return;
                                            }

                                            let Some(new_key) = crate::app::keybinding_recorder::normalize_recorded_keystroke(ev) else {
                                                return;
                                            };

                                            // Check for conflicts with other actions
                                            if let Some((_conflict_id, conflict_label)) =
                                                crate::app::keybinding_recorder::find_conflict(
                                                    &this.config,
                                                    &action,
                                                    &new_key,
                                                )
                                            {
                                                let formatted = crate::app::keybinding_recorder::format_keystroke(&new_key);
                                                this.recording_action = None;
                                                this.keybind_error = Some((
                                                    action.clone(),
                                                    t!("keybind_conflict", key = formatted, action = conflict_label).to_string(),
                                                ));
                                                cx.notify();
                                                return;
                                            }

                                            this.recording_action = None;
                                            this.keybind_error = None;
                                            this.config.set_key_binding(&action, &new_key);
                                            this.save_preferences_background();
                                            cx.notify();
                                        });
                                    }
                                })
                                .on_mouse_down_out({
                                    let view = view.clone();
                                    move |_, _window, cx| {
                                        view.update(cx, |this, cx| {
                                            if this.recording_action.is_some() {
                                                this.recording_action = None;
                                                cx.notify();
                                            }
                                        });
                                    }
                                })
                                .child(
                                    Settings::new("settings")
                                        .sidebar_width(px(180.))
                                        .sidebar_style(div().bg(cx.theme().background).style())
                                .page(
                                    SettingPage::new(t!("settings_general").to_string())
                                        .icon(IconName::Settings)
                                        .default_open(true)
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_group_appearance").to_string())
                                                .item(
                                                    SettingItem::new(
                                                        t!("theme_mode").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                let (follow_system, is_dark_mode) = {
                                                                    let state = view.read(cx);
                                                                    (state.follow_system_theme, state.theme_mode.is_dark())
                                                                };
                                                                Button::new("theme-mode-dropdown")
                                                                    .small()
                                                                    .icon(if follow_system { IconName::Sun } else if is_dark_mode { IconName::Moon } else { IconName::Sun })
                                                                    .label(if follow_system { t!("follow_system").to_string() } else if is_dark_mode { t!("use_dark_mode").to_string() } else { t!("use_light_mode").to_string() })
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let (follow_system, is_dark_mode) = {
                                                                                let state = view.read(cx);
                                                                                (state.follow_system_theme, state.theme_mode.is_dark())
                                                                            };
                                                                            menu = menu.min_w(160.)
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("follow_system").to_string())
                                                                                        .checked(follow_system)
                                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                                            this.set_follow_system_theme(true, window, cx)
                                                                                        }))
                                                                                )
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("use_light_mode").to_string())
                                                                                        .checked(!follow_system && !is_dark_mode)
                                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                                            this.switch_theme_mode(crate::app::ThemeMode::Light, window, cx)
                                                                                        }))
                                                                                )
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("use_dark_mode").to_string())
                                                                                        .checked(!follow_system && is_dark_mode)
                                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                                            this.switch_theme_mode(crate::app::ThemeMode::Dark, window, cx)
                                                                                        }))
                                                                                );
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("light_theme").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                let current_theme = view.read(cx).light_theme_name.to_string();
                                                                Button::new("light-theme-dropdown")
                                                                    .small()
                                                                    .icon(IconName::Sun)
                                                                    .label(current_theme.clone())
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let current_theme = view.read(cx).light_theme_name.to_string();
                                                                            let themes = gpui_component::ThemeRegistry::global(cx).sorted_themes();
                                                                            let light_themes: Vec<_> = themes.into_iter().filter(|t| {
                                                                                !t.mode.is_dark()
                                                                                    && crate::app::theme::allowed_theme_names().contains(&t.name.as_ref())
                                                                            }).map(|t| t.name.clone()).collect();
                                                                            menu = menu.min_w(160.).max_h(px(320.)).scrollable(true);
                                                                            for theme_name in light_themes {
                                                                                let checked = theme_name == current_theme;
                                                                                menu = menu.item(
                                                                                    PopupMenuItem::new(theme_name.clone())
                                                                                        .checked(checked)
                                                                                        .on_click(window.listener_for(&view, move |this, _, window, cx| {
                                                                                            this.apply_theme(theme_name.clone(), window, cx)
                                                                                        }))
                                                                                );
                                                                            }
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("dark_theme").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                let current_theme = view.read(cx).dark_theme_name.to_string();
                                                                Button::new("dark-theme-dropdown")
                                                                    .small()
                                                                    .icon(IconName::Moon)
                                                                    .label(current_theme.clone())
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let current_theme = view.read(cx).dark_theme_name.to_string();
                                                                            let themes = gpui_component::ThemeRegistry::global(cx).sorted_themes();
                                                                            let dark_themes: Vec<_> = themes.into_iter().filter(|t| {
                                                                                t.mode.is_dark()
                                                                                    && crate::app::theme::allowed_theme_names().contains(&t.name.as_ref())
                                                                            }).map(|t| t.name.clone()).collect();
                                                                            menu = menu.min_w(160.).max_h(px(320.)).scrollable(true);
                                                                            for theme_name in dark_themes {
                                                                                let checked = theme_name == current_theme;
                                                                                menu = menu.item(
                                                                                    PopupMenuItem::new(theme_name.clone())
                                                                                        .checked(checked)
                                                                                        .on_click(window.listener_for(&view, move |this, _, window, cx| {
                                                                                            this.apply_theme(theme_name.clone(), window, cx)
                                                                                        }))
                                                                                );
                                                                            }
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                        )
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_group_font").to_string())
                                                .item(
                                                    SettingItem::new(
                                                        t!("ui_font_size").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, cx| {
                                                                h_flex()
                                                                    .items_center()
                                                                    .gap_3()
                                                                    .child(Button::new("ui-font-size-down").small().label("-").on_click(window.listener_for(&view, |this, _, _, cx| this.change_ui_font_size(-1.0, cx))))
                                                                    .child(div().min_w(px(64.)).text_center().child(format!("{:.0}px", view.read(cx).ui_font_size)))
                                                                    .child(Button::new("ui-font-size-up").small().label("+").on_click(window.listener_for(&view, |this, _, _, cx| this.change_ui_font_size(1.0, cx))))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("terminal_font_size").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, cx| {
                                                                h_flex()
                                                                    .items_center()
                                                                    .gap_3()
                                                                    .child(Button::new("terminal-font-size-down").small().label("-").on_click(window.listener_for(&view, |this, _, _, cx| this.change_terminal_font_size(-1.0, cx))))
                                                                    .child(div().min_w(px(64.)).text_center().child(format!("{:.0}px", view.read(cx).terminal_font_size)))
                                                                    .child(Button::new("terminal-font-size-up").small().label("+").on_click(window.listener_for(&view, |this, _, _, cx| this.change_terminal_font_size(1.0, cx))))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("ui_font_family").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                Button::new("ui-font-dropdown")
                                                                    .small()
                                                                    .icon(IconName::ChevronsUpDown)
                                                                    .label({
                                                                        let current = view.read(cx).ui_font_family.to_string();
                                                                        if current == *".SystemUIFont" || current.is_empty() {
                                                                            t!("system_default").to_string()
                                                                        } else {
                                                                            current
                                                                        }
                                                                    })
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let current = view.read(cx).ui_font_family.to_string();
                                                                            let names = cx.text_system().all_font_names();
                                                                            menu = menu.min_w(200.).max_h(px(320.)).scrollable(true);
                                                                            menu = menu.item(
                                                                                PopupMenuItem::new(t!("system_default").to_string())
                                                                                    .checked(current == *".SystemUIFont" || current.is_empty())
                                                                                    .on_click(window.listener_for(&view, move |this, _, window, cx| {
                                                                                        this.change_ui_font_family(".SystemUIFont", window, cx);
                                                                                    }))
                                                                            );
                                                                            for name in names {
                                                                                let checked = name == current;
                                                                                menu = menu.item(
                                                                                    PopupMenuItem::new(name.clone())
                                                                                        .checked(checked)
                                                                                        .on_click(window.listener_for(&view, move |this, _, window, cx| {
                                                                                            this.change_ui_font_family(&name, window, cx);
                                                                                        }))
                                                                                );
                                                                            }
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("terminal_font_family").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                Button::new("terminal-font-dropdown")
                                                                    .small()
                                                                    .icon(IconName::ChevronsUpDown)
                                                                    .label({
                                                                        let state = view.read(cx);
                                                                        let configured = state.config.terminal_font_family();
                                                                        if configured == crate::session::config::SYSTEM_MONOSPACE_FONT || configured.is_empty() {
                                                                            format!(
                                                                                "{} ({})",
                                                                                t!("system_default"),
                                                                                state.terminal_font_family,
                                                                            )
                                                                        } else {
                                                                            configured.to_string()
                                                                        }
                                                                    })
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let current = view.read(cx).config.terminal_font_family().to_string();
                                                                            let names = cx.text_system().all_font_names();
                                                                            menu = menu.min_w(200.).max_h(px(320.)).scrollable(true);
                                                                            menu = menu.item(
                                                                                PopupMenuItem::new(t!("system_default").to_string())
                                                                                    .checked(current == crate::session::config::SYSTEM_MONOSPACE_FONT || current.is_empty())
                                                                                    .on_click(window.listener_for(&view, move |this, _, _window, cx| {
                                                                                        this.change_terminal_font_family(crate::session::config::SYSTEM_MONOSPACE_FONT, cx);
                                                                                    }))
                                                                            );
                                                                            for name in names {
                                                                                let checked = name == current;
                                                                                menu = menu.item(
                                                                                    PopupMenuItem::new(name.clone())
                                                                                        .checked(checked)
                                                                                        .on_click(window.listener_for(&view, move |this, _, _window, cx| {
                                                                                            this.change_terminal_font_family(&name, cx);
                                                                                        }))
                                                                                );
                                                                            }
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("cursor_style").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                use crate::session::config::CursorStyle;
                                                                let current = view.read(cx).cursor_style;
                                                                Button::new("cursor-style-dropdown")
                                                                    .small()
                                                                    .icon(IconName::ChevronsUpDown)
                                                                    .label(match current {
                                                                        CursorStyle::Default => t!("cursor_style_default").to_string(),
                                                                        CursorStyle::Blink => t!("cursor_style_blink").to_string(),
                                                                        CursorStyle::Beam => t!("cursor_style_beam").to_string(),
                                                                        CursorStyle::BeamBlink => t!("cursor_style_beam_blink").to_string(),
                                                                    })
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            use crate::session::config::CursorStyle;
                                                                            let current = view.read(cx).cursor_style;
                                                                            menu = menu.min_w(160.).max_h(px(320.)).scrollable(true);
                                                                            for style in [
                                                                                CursorStyle::Default,
                                                                                CursorStyle::Blink,
                                                                                CursorStyle::Beam,
                                                                                CursorStyle::BeamBlink,
                                                                            ] {
                                                                                let checked = style == current;
                                                                                let label = match style {
                                                                                    CursorStyle::Default => t!("cursor_style_default").to_string(),
                                                                                    CursorStyle::Blink => t!("cursor_style_blink").to_string(),
                                                                                    CursorStyle::Beam => t!("cursor_style_beam").to_string(),
                                                                                    CursorStyle::BeamBlink => t!("cursor_style_beam_blink").to_string(),
                                                                                };
                                                                                menu = menu.item(
                                                                                    PopupMenuItem::new(label)
                                                                                        .checked(checked)
                                                                                        .on_click(window.listener_for(&view, move |this, _, _window, cx| {
                                                                                            this.change_cursor_style(style, cx);
                                                                                        }))
                                                                                );
                                                                            }
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                        )
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_group_other").to_string())
                                                .item(
                                                    SettingItem::new(
                                                        t!("right_click_copy_paste").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, cx| {
                                                                Switch::new("right-click-copy-paste")
                                                                    .small()
                                                                    .checked(view.read(cx).config.right_click_copy_paste())
                                                                    .on_click(window.listener_for(&view, |this, checked, _, cx| {
                                                                        this.config.set_right_click_copy_paste(*checked);
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    ).description(t!("copy_paste_hint").to_string())
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("keyword_highlight").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, cx| {
                                                                Switch::new("keyword-highlight")
                                                                    .small()
                                                                    .checked(view.read(cx).config.keyword_highlight())
                                                                    .on_click(window.listener_for(&view, |this, checked, _, cx| {
                                                                        this.config.set_keyword_highlight(*checked);
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("lock_layout").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, cx| {
                                                                Switch::new("lock-layout")
                                                                    .small()
                                                                    .checked(view.read(cx).config.lock_layout())
                                                                    .on_click(window.listener_for(&view, |this, checked, _, cx| {
                                                                        this.config.set_lock_layout(*checked);
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    ).description(t!("lock_layout_hint").to_string())
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("monitoring_position").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                Button::new("monitoring-position-dropdown")
                                                                    .small()
                                                                    .icon(IconName::PanelLeftOpen)
                                                                    .label({
                                                                        let pos = view.read(cx).config.monitoring_position().to_string();
                                                                        if pos == "Sidebar" {
                                                                            t!("position_sidebar").to_string()
                                                                        } else if pos == "Hidden" {
                                                                            t!("position_hidden").to_string()
                                                                        } else {
                                                                            t!("position_bottom").to_string()
                                                                        }
                                                                    })
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let pos = view.read(cx).config.monitoring_position().to_string();
                                                                            menu = menu.min_w(160.)
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("position_bottom").to_string())
                                                                                        .checked(pos == "Bottom")
                                                                                        .on_click(window.listener_for(&view, |this, _, _window, cx| {
                                                                                            this.config.set_monitoring_position("Bottom");
                                                                                            this.save_preferences_background();
                                                                                            cx.notify();
                                                                                        }))
                                                                                )
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("position_sidebar").to_string())
                                                                                        .checked(pos == "Sidebar")
                                                                                        .on_click(window.listener_for(&view, |this, _, _window, cx| {
                                                                                            this.config.set_monitoring_position("Sidebar");
                                                                                            this.save_preferences_background();
                                                                                            cx.notify();
                                                                                        }))
                                                                                )
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("position_hidden").to_string())
                                                                                        .checked(pos == "Hidden")
                                                                                        .on_click(window.listener_for(&view, |this, _, _window, cx| {
                                                                                            this.config.set_monitoring_position("Hidden");
                                                                                            this.save_preferences_background();
                                                                                            cx.notify();
                                                                                        }))
                                                                                );
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("language").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, _window, cx| {
                                                                Button::new("language-dropdown")
                                                                    .small()
                                                                    .icon(IconName::Globe)
                                                                    .label({
                                                                        let current_locale = view.read(cx).config.locale().to_string();
                                                                        if current_locale == "en" {
                                                                            t!("english").to_string()
                                                                        } else if current_locale == "zh-CN" {
                                                                            t!("chinese").to_string()
                                                                        } else {
                                                                            t!("follow_system").to_string()
                                                                        }
                                                                    })
                                                                    .dropdown_menu_with_anchor(Anchor::BottomRight, {
                                                                        let view = view.clone();
                                                                        move |mut menu, window, cx| {
                                                                            let current_locale = view.read(cx).config.locale().to_string();
                                                                            menu = menu.min_w(160.)
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("follow_system").to_string())
                                                                                        .checked(current_locale == "system")
                                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                                            this.set_display_language("system", window, cx)
                                                                                        }))
                                                                                )
                                                                                .separator()
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("english").to_string())
                                                                                        .checked(current_locale == "en")
                                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                                            this.set_display_language("en", window, cx)
                                                                                        }))
                                                                                )
                                                                                .item(
                                                                                    PopupMenuItem::new(t!("chinese").to_string())
                                                                                        .checked(current_locale == "zh-CN")
                                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                                            this.set_display_language("zh-CN", window, cx)
                                                                                        }))
                                                                                );
                                                                            menu
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("reset_layout").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, _cx| {
                                                                Button::new("reset-layout")
                                                                    .small()
                                                                    .label(t!("reset").to_string())
                                                                    .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                        this.reset_layout(window, cx);
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    ).description(t!("reset_layout_hint").to_string())
                                                )
                                        )
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_group_plugins").to_string())
                                                .item(
                                                    SettingItem::new(
                                                        t!("plugin_history_completion").to_string(),
                                                        SettingField::render({
                                                            let view = view_clone_for_general.clone();
                                                            move |_, window, cx| {
                                                                Switch::new("history-completion-plugin")
                                                                    .small()
                                                                    .checked(view.read(cx).config.history_completion_plugin_enabled())
                                                                    .on_click(window.listener_for(&view, |this, checked, _, cx| {
                                                                        this.config.set_history_completion_plugin_enabled(*checked);
                                                                        if !*checked {
                                                                            this.clear_terminal_completion_inputs();
                                                                        }
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                        )
                                )
                                .page(
                                    SettingPage::new(t!("settings_config_file").to_string())
                                        .icon(IconName::File)
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_config_file").to_string())
                                                .item(SettingItem::new(
                                                    t!("settings_backup_local_desc").to_string(),
                                                    SettingField::render({
                                                        let view = view.clone();
                                                        move |_, window, _cx| {
                                                            h_flex()
                                                                .gap_2()
                                                                .child(
                                                                    Button::new("backup-export")
                                                                        .small()
                                                                        .label(t!("backup_export").to_string())
                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                            this.export_local_config(window, cx);
                                                                        }))
                                                                )
                                                                .child(
                                                                    Button::new("backup-import")
                                                                        .small()
                                                                        .label(t!("backup_import").to_string())
                                                                        .on_click(window.listener_for(&view, |this, _, window, cx| {
                                                                            this.import_local_config(window, cx);
                                                                        }))
                                                                )
                                                                .into_any_element()
                                                        }
                                                    })
                                                ))
                                        )
                                )
                                .page(
                                    SettingPage::new(t!("settings_sync").to_string())
                                        .icon(IconName::Globe)
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_sync").to_string())
                                                .item(SettingItem::render({
                                                    let view = view.clone();
                                                    let endpoint = sync_endpoint_input.clone();
                                                    let username = sync_username_input.clone();
                                                    let webdav_password = sync_webdav_password_input.clone();
                                                    let s3_endpoint = sync_s3_endpoint_input.clone();
                                                    let s3_region = sync_s3_region_input.clone();
                                                    let s3_bucket = sync_s3_bucket_input.clone();
                                                    let s3_object_key = sync_s3_object_key_input.clone();
                                                    let s3_access_key = sync_s3_access_key_input.clone();
                                                    let s3_secret_key = sync_s3_secret_key_input.clone();
                                                    let s3_session_token = sync_s3_session_token_input.clone();
                                                    let encryption_password = sync_encryption_password_input.clone();
                                                    move |_, window, cx| {
                                                        let in_progress = view.read(cx).sync_in_progress;
                                                        let status = view.read(cx).sync_status.clone();
                                                        let is_s3 = view.read(cx).config.sync_backend() == "s3";
                                                        v_flex()
                                                            .w_full()
                                                            .gap_3()
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .child(
                                                                        Button::new("sync-backend-webdav")
                                                                            .small()
                                                                            .label("WebDAV")
                                                                            .when(!is_s3, |button| button.primary())
                                                                            .on_click(window.listener_for(&view, |this, _, _, cx| this.set_sync_backend("webdav", cx)))
                                                                    )
                                                                    .child(
                                                                        Button::new("sync-backend-s3")
                                                                            .small()
                                                                            .label("S3")
                                                                            .when(is_s3, |button| button.primary())
                                                                            .on_click(window.listener_for(&view, |this, _, _, cx| this.set_sync_backend("s3", cx)))
                                                                    )
                                                            )
                                                            .when(!is_s3, |this| this
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_endpoint").to_string())).child(Input::new(&endpoint).w_full()))
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_username").to_string())).child(Input::new(&username).w_full()))
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_webdav_password").to_string())).child(Input::new(&webdav_password).w_full())))
                                                            .when(is_s3, |this| this
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_s3_endpoint").to_string())).child(Input::new(&s3_endpoint).w_full()))
                                                                .child(h_flex().gap_2()
                                                                    .child(v_flex().flex_1().gap_1().child(div().text_sm().child(t!("sync_s3_region").to_string())).child(Input::new(&s3_region).w_full()))
                                                                    .child(v_flex().flex_1().gap_1().child(div().text_sm().child(t!("sync_s3_bucket").to_string())).child(Input::new(&s3_bucket).w_full())))
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_s3_object_key").to_string())).child(Input::new(&s3_object_key).w_full()))
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_s3_access_key").to_string())).child(Input::new(&s3_access_key).w_full()))
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_s3_secret_key").to_string())).child(Input::new(&s3_secret_key).w_full()))
                                                                .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_s3_session_token").to_string())).child(Input::new(&s3_session_token).w_full())))
                                                            .child(v_flex().gap_1().child(div().text_sm().child(t!("sync_encryption_password").to_string())).child(Input::new(&encryption_password).w_full()))
                                                            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t!("sync_security_hint").to_string()))
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .child(Button::new("sync-download").small().disabled(in_progress).label(t!("sync_download").to_string()).on_click(window.listener_for(&view, |this, _, _, cx| this.download_sync_config(cx))))
                                                                    .child(Button::new("sync-upload").small().disabled(in_progress).label(t!("sync_upload").to_string()).on_click(window.listener_for(&view, |this, _, _, cx| this.upload_sync_config(cx)))),
                                                            )
                                                            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(status))
                                                    }
                                                }))
                                        )
                                )
                                .page(
                                    SettingPage::new(t!("settings_proxy").to_string())
                                        .icon(IconName::Network)
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("settings_proxy").to_string())
                                                .item(
                                                    SettingItem::new(
                                                        t!("enable_proxy").to_string(),
                                                        SettingField::render({
                                                            let view = view.clone();
                                                            move |_, window, cx| {
                                                                Switch::new("use-proxy")
                                                                    .small()
                                                                    .checked(view.read(cx).config.use_proxy())
                                                                    .on_click(window.listener_for(&view, |this, checked, _, cx| {
                                                                        this.config.set_use_proxy(*checked);
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    )
                                                )
                                                .item(
                                                    SettingItem::new(
                                                        t!("read_env_proxy").to_string(),
                                                        SettingField::render({
                                                            let view = view.clone();
                                                            move |_, window, cx| {
                                                                Switch::new("read-env-proxy")
                                                                    .small()
                                                                    .checked(view.read(cx).config.read_env_proxy())
                                                                    .on_click(window.listener_for(&view, |this, checked, _, cx| {
                                                                        this.config.set_read_env_proxy(*checked);
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                                    .into_any_element()
                                                            }
                                                        })
                                                    ).description(t!("read_env_proxy_desc").to_string())
                                                )
                                                .item(SettingItem::render({
                                                    let view = view.clone();
                                                    let global_proxy_host_input = view.read(cx).global_proxy_host_input.clone();
                                                    let global_proxy_port_input = view.read(cx).global_proxy_port_input.clone();
                                                    let global_proxy_user_input = view.read(cx).global_proxy_user_input.clone();
                                                    let global_proxy_password_input = view.read(cx).global_proxy_password_input.clone();
                                                    move |_, window, cx| {
                                                        let proxy_type = view.read(cx).global_proxy_type.clone();
                                                        let normalized_proxy_type = proxy_type.trim().to_ascii_lowercase();
                                                        v_flex()
                                                            .w_full()
                                                            .gap_3()
                                                            .child(div().text_sm().font_weight(FontWeight::BOLD).child(t!("global_proxy_settings").to_string()))
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .child(
                                                                        Button::new("global-proxy-type-socks5")
                                                                            .small()
                                                                            .label("SOCKS5")
                                                                            .when(normalized_proxy_type == "socks5", |b| b.primary())
                                                                            .on_click(window.listener_for(&view, |this, _, _, cx| {
                                                                                this.global_proxy_type = "socks5".to_string();
                                                                                cx.notify();
                                                                            }))
                                                                    )
                                                                    .child(
                                                                        Button::new("global-proxy-type-http")
                                                                            .small()
                                                                            .label("HTTP")
                                                                            .when(normalized_proxy_type == "http", |b| b.primary())
                                                                            .on_click(window.listener_for(&view, |this, _, _, cx| {
                                                                                this.global_proxy_type = "http".to_string();
                                                                                cx.notify();
                                                                            }))
                                                                    )
                                                                    .child(
                                                                        Button::new("global-proxy-type-https")
                                                                            .small()
                                                                            .label("HTTPS")
                                                                            .when(normalized_proxy_type == "https", |b| b.primary())
                                                                            .on_click(window.listener_for(&view, |this, _, _, cx| {
                                                                                this.global_proxy_type = "https".to_string();
                                                                                cx.notify();
                                                                            }))
                                                                    )
                                                            )
                                                            .child(v_flex().gap_1().child(div().text_sm().child(t!("global_proxy_host").to_string())).child(Input::new(&global_proxy_host_input).w_full()))
                                                            .child(v_flex().gap_1().child(div().text_sm().child(t!("global_proxy_port").to_string())).child(Input::new(&global_proxy_port_input).w_full()))
                                                            .child(v_flex().gap_1().child(div().text_sm().child(t!("global_proxy_user").to_string())).child(Input::new(&global_proxy_user_input).w_full()))
                                                            .child(v_flex().gap_1().child(div().text_sm().child(t!("global_proxy_password").to_string())).child(Input::new(&global_proxy_password_input).w_full()))
                                                            .child(
                                                                Button::new("save-global-proxy")
                                                                    .small()
                                                                    .primary()
                                                                    .label(t!("save_proxy").to_string())
                                                                    .on_click(window.listener_for(&view, |this, _, _, cx| {
                                                                        let host = this.global_proxy_host_input.read(cx).value().trim().to_string();
                                                                        let port = parse_non_zero_u16(&this.global_proxy_port_input.read(cx).value());
                                                                        let user = this.global_proxy_user_input.read(cx).value().trim().to_string();
                                                                        let password = this.global_proxy_password_input.read(cx).value().to_string();
                                                                        let Some(proxy_type) = supported_proxy_protocol(&this.global_proxy_type) else {
                                                                            this.status = t!("proxy_protocol_unsupported").into();
                                                                            cx.notify();
                                                                            return;
                                                                        };

                                                                        if host.is_empty() || port.is_none() {
                                                                            this.status = t!("proxy_host_port_required").into();
                                                                            cx.notify();
                                                                            return;
                                                                        }

                                                                        this.config.set_global_proxy_type(proxy_type);
                                                                        this.config.set_global_proxy_host(host);
                                                                        this.config.set_global_proxy_port(port);
                                                                        this.config.set_global_proxy_user(user);
                                                                        this.config.set_global_proxy_password(password);
                                                                        this.save_preferences_background();
                                                                        cx.notify();
                                                                    }))
                                                            )
                                                    }
                                                }))
                                        )
                                )
                                .page({
                                    let mut page = SettingPage::new(t!("settings_key_bindings").to_string())
                                        .icon(IconName::SquareTerminal)
                                        .default_open(true);
                                    for group in crate::app::keybinding_recorder::KeybindingsPage::render_groups(&view, cx) {
                                        page = page.group(group);
                                    }
                                    page
                                })
                                .page(
                                    SettingPage::new(t!("settings_help").to_string())
                                        .icon(IconName::BookOpen)
                                )
                                .page(
                                    SettingPage::new(t!("settings_about").to_string())
                                        .icon(IconName::Info)
                                        .group(
                                            SettingGroup::new()
                                                .item(SettingItem::render(move |_, _window, cx| {
                                                    v_flex()
                                                        .gap_1()
                                                        .child(div().text_size(rems(1.35)).font_weight(FontWeight::BOLD).child("JShell"))
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.833))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_edition")),
                                                        )
                                                        .child(
                                                            div()
                                                                .mt_2()
                                                                .text_size(rems(0.9))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_summary")),
                                                        )
                                                }))
                                        )
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("about_compared_to_upstream").to_string())
                                                .item(SettingItem::render(move |_, _window, cx| {
                                                    v_flex()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.833))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_comparison_features")),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.833))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_comparison_interface")),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.833))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_upstream_foundation")),
                                                        )
                                                }))
                                        )
                                        .group(
                                            SettingGroup::new()
                                                .title(t!("about_project").to_string())
                                                .item(SettingItem::render(move |_, _window, cx| {
                                                    v_flex()
                                                        .gap_3()
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.833))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_acknowledgement")),
                                                        )
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .child(
                                                                    Button::new("github-project-link")
                                                                        .small()
                                                                        .label(t!("about_project_home").to_string())
                                                                        .on_click(|_, _window, _cx| {
                                                                            let _ = open::that("https://github.com/JayhaShf/JShell");
                                                                        }),
                                                                )
                                                                .child(
                                                                    Button::new("github-upstream-link")
                                                                        .ghost()
                                                                        .small()
                                                                        .label(t!("about_upstream_project").to_string())
                                                                        .on_click(|_, _window, _cx| {
                                                                            let _ = open::that("https://github.com/rust-kotlin/ashell");
                                                                        }),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(rems(0.75))
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(t!("about_feedback_hint")),
                                                        )
                                                }))
                                        )
                                )
                                )
                        )
                    }
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_download_target_is_forwarded_to_platform_opener() {
        let mut opened = None;
        let result = open_completed_download_target_with("C:/Downloads/archive.zip", |target| {
            opened = Some(target.to_string());
            Ok::<(), &'static str>(())
        });

        assert_eq!(result, Ok(()));
        assert_eq!(opened.as_deref(), Some("C:/Downloads/archive.zip"));
    }

    #[test]
    fn completed_download_open_error_is_preserved_for_logging() {
        let result = open_completed_download_target_with("C:/Downloads/archive.zip", |_| {
            Err::<(), _>("cannot open")
        });

        assert_eq!(result, Err("cannot open".to_string()));
    }
}
