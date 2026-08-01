pub mod command_history;
pub mod config_sync;
pub mod constants;
pub mod dialogs;
pub mod keybinding_recorder;
pub mod pane_layout;
pub mod resizable;
pub mod search;
pub mod startup;
pub mod theme;
pub mod ui;
pub mod workspace_tabs;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::app::resizable::ResizableState;
use crate::app::startup::StartupConfig;
use gpui::{
    AnyWindowHandle, AppContext as _, Bounds, Context, Entity, FocusHandle, Pixels, Point,
    SharedString, Size, UniformListScrollHandle, Window, point, px, size,
};
use gpui_component::{
    Theme, ThemeMode,
    input::{InputEvent, InputState},
    scroll::ScrollbarHandle,
};
use rust_i18n::t;
use tokio::runtime::Runtime;

use crate::{
    session::config::{AuthMethod, ConfigStore},
    session::ssh_config::SshConfigEntry,
    system::{SystemSampler, SystemSnapshot},
    terminal::{self, BackendEvent, TabKind, TerminalTab},
};
pub(crate) use pane_layout::{PaneLayout, PaneLeaf};

pub(crate) fn format_window_title(
    document: Option<(&str, bool)>,
    session_fallback: Option<&str>,
) -> String {
    if let Some((path, dirty)) = document {
        let file_name = path
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(path);
        let dirty_marker = if dirty { "*" } else { "" };
        return format!("JShell - {file_name}{dirty_marker}");
    }

    session_fallback
        .filter(|title| !title.is_empty())
        .map(|title| format!("JShell - {title}"))
        .unwrap_or_else(|| "JShell".to_string())
}

struct GlobalProxyFormValues {
    proxy_type: String,
    host: String,
    port: String,
    user: String,
    password: String,
}

fn global_proxy_form_values(config: &ConfigStore) -> GlobalProxyFormValues {
    GlobalProxyFormValues {
        proxy_type: config.global_proxy_type().to_string(),
        host: config.global_proxy_host().to_string(),
        port: config
            .global_proxy_port()
            .map(|port| port.to_string())
            .unwrap_or_default(),
        user: config.global_proxy_user().to_string(),
        password: config.global_proxy_password().to_string(),
    }
}

#[derive(Clone, Default)]
pub(crate) struct ConfigWriteCoordinator {
    lock: Arc<Mutex<()>>,
    latest_preference_sequence: Arc<AtomicU64>,
}

impl ConfigWriteCoordinator {
    pub(crate) fn begin_preference_save(&self) -> u64 {
        self.latest_preference_sequence
            .fetch_add(1, Ordering::SeqCst)
            + 1
    }

    pub(crate) fn run_preference_save<R>(
        &self,
        sequence: u64,
        write: impl FnOnce() -> R,
    ) -> Option<R> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        if sequence < self.latest_preference_sequence.load(Ordering::SeqCst) {
            return None;
        }
        Some(write())
    }

    pub(crate) fn run_exclusive<R>(&self, write: impl FnOnce() -> R) -> R {
        self.latest_preference_sequence
            .fetch_add(1, Ordering::SeqCst);
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        write()
    }
}

#[derive(Clone)]
pub(crate) struct TabGroup {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) pane_root: PaneLayout,
    pub(crate) focused_pane_path: Vec<usize>,
    pub(crate) sftp: Option<crate::terminal::SftpUiState>,
}

#[cfg(test)]
mod window_title_tests {
    use super::{ConfigWriteCoordinator, format_window_title, global_proxy_form_values};
    use crate::session::config::ConfigStore;

    #[test]
    fn document_title_uses_only_the_file_name_and_dirty_marker() {
        assert_eq!(
            format_window_title(Some(("/etc/nginx/nginx.conf", true)), Some("production")),
            "JShell - nginx.conf*"
        );
    }

    #[test]
    fn session_title_is_used_when_no_document_is_focused() {
        assert_eq!(
            format_window_title(None, Some("production")),
            "JShell - production"
        );
        assert_eq!(format_window_title(None, None), "JShell");
    }

    #[test]
    fn loaded_global_proxy_values_are_ready_to_refresh_the_settings_form() {
        let mut config = ConfigStore::in_memory();
        config.set_global_proxy_type("https".to_string());
        config.set_global_proxy_host("proxy.example.test".to_string());
        config.set_global_proxy_port(Some(8443));
        config.set_global_proxy_user("proxy-user".to_string());
        config.set_global_proxy_password("proxy-password".to_string());

        let values = global_proxy_form_values(&config);

        assert_eq!(values.proxy_type, "https");
        assert_eq!(values.host, "proxy.example.test");
        assert_eq!(values.port, "8443");
        assert_eq!(values.user, "proxy-user");
        assert_eq!(values.password, "proxy-password");
    }

    #[test]
    fn exclusive_config_write_invalidates_an_older_preference_save() {
        let coordinator = ConfigWriteCoordinator::default();
        let stale_sequence = coordinator.begin_preference_save();
        let writes = std::sync::Mutex::new(Vec::new());

        coordinator.run_exclusive(|| writes.lock().unwrap().push("sync"));
        let stale_result = coordinator.run_preference_save(stale_sequence, || {
            writes.lock().unwrap().push("stale-preferences")
        });

        assert!(stale_result.is_none());
        assert_eq!(*writes.lock().unwrap(), vec!["sync"]);
    }
}

pub(crate) struct TerminalScrollbarState {
    line_height: Pixels,
    total_lines: usize,
    viewport_lines: usize,
    display_offset: usize,
}

#[derive(Clone, Default)]
pub(crate) struct TerminalScrollbarHandle {
    state: Rc<RefCell<Option<TerminalScrollbarState>>>,
    pub(crate) future_display_offset: Rc<Cell<Option<usize>>>,
}

impl TerminalScrollbarHandle {
    pub(crate) fn update(&self, snapshot: &terminal::RenderSnapshot, line_height: Pixels) {
        self.state.replace(Some(TerminalScrollbarState {
            line_height,
            total_lines: snapshot.history_size + snapshot.rows,
            viewport_lines: snapshot.rows,
            display_offset: snapshot.display_offset,
        }));
    }
}

impl ScrollbarHandle for TerminalScrollbarHandle {
    fn offset(&self) -> Point<Pixels> {
        let state_ref = self.state.borrow();
        let Some(state) = state_ref.as_ref() else {
            return point(px(0.), px(0.));
        };
        let scroll_offset = state
            .total_lines
            .saturating_sub(state.viewport_lines)
            .saturating_sub(state.display_offset);
        point(px(0.), -(scroll_offset as f32 * state.line_height))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let state_ref = self.state.borrow();
        let Some(state) = state_ref.as_ref() else {
            return;
        };
        let offset_delta = (offset.y / state.line_height).round() as i32;
        let max_offset = state.total_lines.saturating_sub(state.viewport_lines);
        let display_offset = (max_offset as i32 + offset_delta).clamp(0, max_offset as i32);
        self.future_display_offset
            .set(Some(display_offset as usize));
    }

    fn content_size(&self) -> Size<Pixels> {
        let state_ref = self.state.borrow();
        let Some(state) = state_ref.as_ref() else {
            return size(px(0.), px(0.));
        };
        size(
            px(0.),
            state.total_lines.max(state.viewport_lines) as f32 * state.line_height,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Settings,
    SessionSelector,
    SessionFolder,
    Transfers,
    NewSsh,
}

pub(crate) struct Ashell {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) selector_focus_handle: FocusHandle,
    pub(crate) host_input: Entity<InputState>,
    pub(crate) session_name_input: Entity<InputState>,
    pub(crate) session_folder_name_input: Entity<InputState>,
    pub(crate) port_input: Entity<InputState>,
    pub(crate) user_input: Entity<InputState>,
    pub(crate) password_input: Entity<InputState>,
    pub(crate) key_path_input: Entity<InputState>,
    pub(crate) key_inline_input: Entity<InputState>,
    pub(crate) passphrase_input: Entity<InputState>,
    pub(crate) baud_rate_input: Entity<InputState>,
    pub(crate) session_protocol: String,
    pub(crate) ssh_proxy_type: String,
    pub(crate) proxy_host_input: Entity<InputState>,
    pub(crate) proxy_port_input: Entity<InputState>,
    pub(crate) proxy_user_input: Entity<InputState>,
    pub(crate) proxy_password_input: Entity<InputState>,
    pub(crate) global_proxy_type: String,
    pub(crate) global_proxy_host_input: Entity<InputState>,
    pub(crate) global_proxy_port_input: Entity<InputState>,
    pub(crate) global_proxy_user_input: Entity<InputState>,
    pub(crate) global_proxy_password_input: Entity<InputState>,
    pub(crate) sync_endpoint_input: Entity<InputState>,
    pub(crate) sync_username_input: Entity<InputState>,
    pub(crate) sync_webdav_password_input: Entity<InputState>,
    pub(crate) sync_s3_endpoint_input: Entity<InputState>,
    pub(crate) sync_s3_region_input: Entity<InputState>,
    pub(crate) sync_s3_bucket_input: Entity<InputState>,
    pub(crate) sync_s3_object_key_input: Entity<InputState>,
    pub(crate) sync_s3_access_key_input: Entity<InputState>,
    pub(crate) sync_s3_secret_key_input: Entity<InputState>,
    pub(crate) sync_s3_session_token_input: Entity<InputState>,
    pub(crate) sync_r2_account_id_input: Entity<InputState>,
    pub(crate) sync_r2_bucket_input: Entity<InputState>,
    pub(crate) sync_r2_object_key_input: Entity<InputState>,
    pub(crate) sync_r2_access_key_id_input: Entity<InputState>,
    pub(crate) sync_r2_secret_access_key_input: Entity<InputState>,
    pub(crate) sync_encryption_password_input: Entity<InputState>,
    pub(crate) sync_provider: String,
    pub(crate) sync_remember_encryption_password: bool,
    pub(crate) sync_ui_state: crate::app::config_sync::SyncUiState,
    pub(crate) sync_in_progress: bool,
    pub(crate) sync_status: SharedString,
    pub(crate) sftp_path_input: Entity<InputState>,
    pub(crate) ssh_auth_method: AuthMethod,
    pub(crate) ssh_config_entries: Vec<SshConfigEntry>,
    pub(crate) ssh_config_selected: Option<usize>,
    pub(crate) editing_session_id: Option<String>,
    pub(crate) editing_session_folder_id: Option<String>,
    pub(crate) editing_session_folder_members: std::collections::HashSet<String>,
    pub(crate) collapsed_session_folder_ids: std::collections::HashSet<String>,
    pub(crate) follow_system_theme: bool,
    pub(crate) theme_mode: ThemeMode,
    pub(crate) light_theme_name: SharedString,
    pub(crate) dark_theme_name: SharedString,
    pub(crate) ui_font_size: f32,
    pub(crate) terminal_font_size: f32,
    pub(crate) terminal_cell_width: f32,
    pub(crate) terminal_zoom_accumulator: f32,
    pub(crate) ui_font_family: SharedString,
    pub(crate) terminal_font_family: SharedString,
    pub(crate) tabs: Vec<TerminalTab>,
    pub(crate) active_tab: Option<String>,
    pub(crate) tab_groups: Vec<TabGroup>,
    pub(crate) active_group: Option<String>,
    pub(crate) workspace_tabs: Vec<crate::document::WorkspaceTab>,
    pub(crate) active_workspace_tab: Option<String>,
    pub(crate) documents: std::collections::HashMap<String, crate::document::RemoteDocument>,
    pub(crate) detaching_document_ids: HashSet<String>,
    pub(crate) detached_document_windows: HashMap<String, AnyWindowHandle>,
    pub(crate) closing_application: bool,
    pub(crate) allow_window_close: bool,
    pub(crate) window_close_prompt_open: bool,
    pub(crate) window_close_save_queue: Vec<String>,
    pub(crate) window_close_save_current: Option<String>,
    pub(crate) selector_selection: usize,
    pub(crate) workspace_panels: Entity<ResizableState>,
    pub(crate) body_panels: Entity<ResizableState>,
    pub(crate) is_layout_reset: bool,
    pub(crate) terminal_scrollbars: HashMap<String, TerminalScrollbarHandle>,
    pub(crate) remote_files_scroll_handle: UniformListScrollHandle,
    pub(crate) disk_scroll_handle: gpui::ScrollHandle,
    pub(crate) tabs_scroll_handle: gpui::ScrollHandle,
    pub(crate) selector_scroll_handle: gpui::ScrollHandle,
    pub(crate) saved_scroll_handle: gpui::ScrollHandle,
    pub(crate) connection_scroll_handle: gpui::ScrollHandle,
    pub(crate) connection_progress: Option<ConnectionProgress>,
    pub(crate) pending_sftp_path_sync: Option<String>,
    pub(crate) sftp_context_menu: Option<SftpContextMenuState>,
    pub(crate) sftp_creating_folder: bool,
    pub(crate) sftp_new_folder_input: Entity<InputState>,
    pub(crate) sftp_delete_scroll_handle: gpui::ScrollHandle,
    pub(crate) show_hidden_files: bool,
    pub(crate) transfers: Vec<crate::terminal::Transfer>,
    pub(crate) show_transfers_dialog: bool,
    pub(crate) system_status: Option<SharedString>,
    pub(crate) pane_root: PaneLayout,
    pub(crate) focused_pane_path: Vec<usize>,
    pub(crate) terminal_panel_bounds: Option<Bounds<Pixels>>,
    pub(crate) split_container_bounds: HashMap<Vec<usize>, Bounds<Pixels>>,
    pub(crate) terminal_bounds: HashMap<String, Bounds<Pixels>>,
    pub(crate) terminal_selecting: bool,
    pub(crate) dragging_splitter: Option<(Vec<usize>, usize)>, // (parent_path, child_index)
    pub(crate) drag_split_origin: Option<gpui::Point<Pixels>>,
    pub(crate) terminal_marked_text: Option<String>,
    pub(crate) sftp_panel_minimized: bool,
    pub(crate) sidebar_collapsed: bool,
    pub(crate) prev_monitoring_size: Option<Pixels>,
    pub(crate) status: SharedString,
    pub(crate) config: ConfigStore,
    pub(crate) startup_config_error: Option<String>,
    pub(crate) cursor_style: crate::session::config::CursorStyle,
    pub(crate) system_sampler: SystemSampler,
    pub(crate) recording_action: Option<String>,
    pub(crate) active_dialog: Option<DialogKind>,
    /// Error message when a recorded keybinding conflicts with another
    pub(crate) keybind_error: Option<(String, String)>, // (action_id, error_message)
    /// Whether workspace keybindings are currently suspended (during settings)
    pub(crate) keybinds_suspended: bool,
    pub(crate) system: SystemSnapshot,
    pub(crate) cpu_history: Vec<f32>,
    pub(crate) net_rx_history: Vec<f32>,
    pub(crate) net_tx_history: Vec<f32>,
    pub(crate) last_system_sample: Instant,
    pub(crate) last_theme_sync: Instant,

    pub(crate) search_input: Entity<InputState>,
    pub(crate) search_active: bool,
    pub(crate) search_query: String,
    pub(crate) search_matches: Vec<(i32, i32)>,
    pub(crate) search_current: usize,
    pub(crate) search_target_tab: Option<String>,
    pub(crate) search_bar_bounds: Option<Bounds<Pixels>>,

    pub(crate) command_history_input: Entity<InputState>,
    pub(crate) command_bar_input: Entity<InputState>,
    pub(crate) command_bar_open: bool,
    pub(crate) command_history_panel_open: bool,
    pub(crate) command_history_target_tab: Option<String>,
    pub(crate) command_history_by_tab: HashMap<String, RemoteCommandHistoryState>,
    pub(crate) command_history_scroll_handle: gpui::ScrollHandle,

    pub(crate) system_tab_id: Option<String>,
    pub(crate) sftp_handles: std::collections::HashMap<String, crate::sftp::SftpHandle>,

    pub(crate) remote_sample_in_flight: bool,
    pub(crate) runtime: Runtime,
    pub(crate) events_rx: mpsc::Receiver<BackendEvent>,
    pub(crate) events_tx: mpsc::Sender<BackendEvent>,
    pub(crate) last_window_size: Option<gpui::Size<Pixels>>,
    pub(crate) last_sidebar_width: Option<Pixels>,
    pub(crate) hovered_url: Option<HoveredUrl>,
    pub(crate) terminal_link_ctrl_pressed: bool,
    pub(crate) _subscriptions: Vec<gpui::Subscription>,
    pub(crate) config_writes: ConfigWriteCoordinator,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RemoteCommandHistoryState {
    pub(crate) entries: Vec<String>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) input_buffer: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HoveredUrl {
    pub(crate) url: String,
    pub(crate) tab_id: String,
    pub(crate) cells: Vec<(usize, usize)>,
}

#[derive(Clone)]
pub(crate) enum SelectorEntry {
    Local,
    NewSsh,
    Saved(String),
}

#[derive(Clone)]
pub(crate) struct ConnectionProgress {
    pub(crate) tab_id: String,
    pub(crate) title: SharedString,
    pub(crate) lines: Vec<SharedString>,
    pub(crate) failed: bool,
}

#[derive(Clone)]
pub(crate) struct SftpContextMenuState {
    pub(crate) remote_path: String,
    pub(crate) is_dir: bool,
    pub(crate) position: Point<Pixels>,
}

impl Ashell {
    pub(crate) fn current_window_title(&self) -> String {
        let document = self
            .pane_root
            .focused_leaf(&self.focused_pane_path)
            .and_then(|leaf| match leaf {
                PaneLeaf::Document(document_id) => self.documents.get(document_id),
                PaneLeaf::Terminal(_) | PaneLeaf::Empty => None,
            })
            .map(|document| {
                (
                    document.key.remote_path.as_str(),
                    document.revisions.is_dirty(),
                )
            });
        let session_fallback = self
            .active_group
            .as_ref()
            .and_then(|group_id| self.tab_groups.iter().find(|group| &group.id == group_id))
            .map(|group| group.title.as_str())
            .or_else(|| {
                self.active_tab
                    .as_ref()
                    .and_then(|tab_id| self.tabs.iter().find(|tab| &tab.id == tab_id))
                    .map(|tab| tab.title.as_str())
            });
        format_window_title(document, session_fallback)
    }

    fn transfer_source_title(&self, tab_id: &str) -> String {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.title.clone())
            .or_else(|| {
                self.tab_groups
                    .iter()
                    .find(|group| group.id == tab_id)
                    .map(|group| group.title.clone())
            })
            .or_else(|| {
                self.tab_groups
                    .iter()
                    .find(|group| group.pane_root.contains(tab_id))
                    .map(|group| group.title.clone())
            })
            .unwrap_or_else(|| "Unknown".to_string())
    }

    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        startup_config: StartupConfig,
    ) -> Self {
        let StartupConfig {
            config,
            error: startup_config_error,
        } = startup_config;
        let sync_provider = config.sync_backend().to_string();
        let sync_remember_encryption_password = config.sync_remember_encryption_password();
        let mut sync_r2_secret_access_key = String::new();
        let mut sync_encryption_password = String::new();
        let mut sync_credential_load_error = None;
        if let Some(target_id) = config.sync_target_id() {
            let credential_store = crate::sync::PlatformSyncCredentialStore::new();
            if sync_provider == "r2" {
                match crate::sync::SyncCredentialStore::load_r2_secret(
                    &credential_store,
                    &target_id,
                ) {
                    Ok(Some(secret)) => {
                        sync_r2_secret_access_key = secret.expose_secret().to_string()
                    }
                    Ok(None) => {}
                    Err(error) => {
                        sync_credential_load_error =
                            Some(crate::app::config_sync::sync_error_message(&error))
                    }
                }
            }
            if sync_remember_encryption_password {
                match crate::sync::SyncCredentialStore::load_encryption_password(
                    &credential_store,
                    &target_id,
                ) {
                    Ok(Some(password)) => {
                        sync_encryption_password = password.expose_secret().to_string()
                    }
                    Ok(None) => {}
                    Err(error) => {
                        sync_credential_load_error =
                            Some(crate::app::config_sync::sync_error_message(&error))
                    }
                }
            }
        }
        let host_input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("host")));
        let session_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("name (optional)"));
        let session_folder_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let port_input = cx.new(|cx| InputState::new(window, cx).default_value("22"));
        let user_input = cx.new(|cx| InputState::new(window, cx).default_value("root"));
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("password"))
                .masked(true)
        });
        let key_path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("~/.ssh/id_ed25519"));
        let key_inline_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(5)
                .placeholder("-----BEGIN OPENSSH PRIVATE KEY-----")
        });
        let passphrase_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("SSH private key passphrase (optional)")
                .masked(true)
        });
        let baud_rate_input = cx.new(|cx| InputState::new(window, cx).default_value("115200"));
        let proxy_host_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("proxy_host").to_string()));
        let proxy_port_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("proxy_port").to_string()));
        let proxy_user_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("proxy_user").to_string()));
        let proxy_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("proxy_password").to_string())
                .masked(true)
        });
        let sftp_path_input = cx.new(|cx| InputState::new(window, cx).default_value("/"));
        let sftp_new_folder_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("new_folder").to_string()));
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("search").to_string()));
        let command_history_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("command_history_search").to_string())
        });
        let command_bar_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("command_bar_placeholder").to_string())
        });
        let global_proxy_host_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("proxy_host").to_string())
                .default_value(config.global_proxy_host())
        });
        let global_proxy_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("proxy_port").to_string())
                .default_value(
                    config
                        .global_proxy_port()
                        .map(|p| p.to_string())
                        .unwrap_or_default(),
                )
        });
        let global_proxy_user_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("proxy_user").to_string())
                .default_value(config.global_proxy_user())
        });
        let global_proxy_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("proxy_password").to_string())
                .masked(true)
                .default_value(config.global_proxy_password())
        });
        let sync_endpoint_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://dav.example.com/jshell/")
                .default_value(config.sync_endpoint())
        });
        let sync_username_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sync_username").to_string())
                .default_value(config.sync_username())
        });
        let sync_webdav_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sync_webdav_password").to_string())
                .masked(true)
        });
        let sync_s3_endpoint_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://s3.example.com")
                .default_value(config.sync_s3_endpoint())
        });
        let sync_s3_region_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("us-east-1")
                .default_value(config.sync_s3_region())
        });
        let sync_s3_bucket_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sync_s3_bucket").to_string())
                .default_value(config.sync_s3_bucket())
        });
        let sync_s3_object_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("jshell-sync.json")
                .default_value(config.sync_s3_object_key())
        });
        let sync_s3_access_key_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("sync_s3_access_key").to_string())
        });
        let sync_s3_secret_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sync_s3_secret_key").to_string())
                .masked(true)
        });
        let sync_s3_session_token_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sync_s3_session_token").to_string())
                .masked(true)
        });
        let sync_r2_account_id_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("R2 account ID")
                .default_value(config.sync_r2_account_id())
        });
        let sync_r2_bucket_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("R2 bucket")
                .default_value(config.sync_r2_bucket())
        });
        let sync_r2_object_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("jshell-sync.json")
                .default_value(config.sync_r2_object_key())
        });
        let sync_r2_access_key_id_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("R2 access key ID")
                .default_value(config.sync_r2_access_key_id())
        });
        let sync_r2_secret_access_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("R2 secret access key")
                .masked(true)
                .default_value(sync_r2_secret_access_key)
        });
        let sync_encryption_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sync_encryption_password").to_string())
                .masked(true)
                .default_value(sync_encryption_password)
        });

        let _subscriptions = vec![
            cx.subscribe_in(&host_input, window, Self::on_input_event),
            cx.subscribe_in(&session_name_input, window, Self::on_input_event),
            cx.subscribe_in(&session_folder_name_input, window, Self::on_input_event),
            cx.subscribe_in(&port_input, window, Self::on_input_event),
            cx.subscribe_in(&user_input, window, Self::on_input_event),
            cx.subscribe_in(&password_input, window, Self::on_input_event),
            cx.subscribe_in(&key_path_input, window, Self::on_input_event),
            cx.subscribe_in(&key_inline_input, window, Self::on_input_event),
            cx.subscribe_in(&passphrase_input, window, Self::on_input_event),
            cx.subscribe_in(&baud_rate_input, window, Self::on_input_event),
            cx.subscribe_in(&proxy_host_input, window, Self::on_input_event),
            cx.subscribe_in(&proxy_port_input, window, Self::on_input_event),
            cx.subscribe_in(&proxy_user_input, window, Self::on_input_event),
            cx.subscribe_in(&proxy_password_input, window, Self::on_input_event),
            cx.subscribe_in(&sftp_path_input, window, Self::on_input_event),
            cx.subscribe_in(&sftp_new_folder_input, window, Self::on_input_event),
            cx.subscribe_in(&search_input, window, Self::on_input_event),
            cx.subscribe_in(&command_history_input, window, Self::on_input_event),
            cx.subscribe_in(&command_bar_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_endpoint_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_username_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_webdav_password_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_endpoint_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_region_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_bucket_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_object_key_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_access_key_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_secret_key_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_s3_session_token_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_r2_account_id_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_r2_bucket_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_r2_object_key_input, window, Self::on_input_event),
            cx.subscribe_in(&sync_r2_access_key_id_input, window, Self::on_input_event),
            cx.subscribe_in(
                &sync_r2_secret_access_key_input,
                window,
                Self::on_input_event,
            ),
            cx.subscribe_in(
                &sync_encryption_password_input,
                window,
                Self::on_input_event,
            ),
        ];

        let (events_tx, events_rx) = mpsc::channel();
        let workspace_panels = cx.new(|_| ResizableState::default());
        let body_panels = cx.new(|_| ResizableState::default());
        let mut system_sampler = SystemSampler::new();
        let system = system_sampler.sample();
        let follow_system_theme = config.follow_system_theme();

        let theme_mode = match config.theme_mode() {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::Light,
        };
        let light_theme_name =
            crate::app::theme::validated_theme_name(config.light_theme_name(), false).into();
        let dark_theme_name =
            crate::app::theme::validated_theme_name(config.dark_theme_name(), true).into();

        let configured_locale = config.locale();
        let mut active_locale = configured_locale.to_string();
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
        let ui_font_family: SharedString = config.ui_font_family().into();
        let installed_fonts = cx.text_system().all_font_names();
        let terminal_font_family: SharedString = crate::app::theme::resolve_terminal_font_family(
            config.terminal_font_family(),
            &installed_fonts,
        )
        .into();
        let last_sidebar_width = Some(px(config
            .workspace_panels()
            .and_then(|s| s.first().copied())
            .unwrap_or(constants::SIDEBAR_WIDTH)));
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            selector_focus_handle: cx.focus_handle(),
            host_input,
            session_name_input,
            session_folder_name_input,
            port_input,
            user_input,
            password_input,
            key_path_input,
            key_inline_input,
            passphrase_input,
            baud_rate_input,
            session_protocol: "ssh".to_string(),
            ssh_proxy_type: "none".to_string(),
            proxy_host_input,
            proxy_port_input,
            proxy_user_input,
            proxy_password_input,
            global_proxy_type: config.global_proxy_type().to_string(),
            global_proxy_host_input,
            global_proxy_port_input,
            global_proxy_user_input,
            global_proxy_password_input,
            sync_endpoint_input,
            sync_username_input,
            sync_webdav_password_input,
            sync_s3_endpoint_input,
            sync_s3_region_input,
            sync_s3_bucket_input,
            sync_s3_object_key_input,
            sync_s3_access_key_input,
            sync_s3_secret_key_input,
            sync_s3_session_token_input,
            sync_r2_account_id_input,
            sync_r2_bucket_input,
            sync_r2_object_key_input,
            sync_r2_access_key_id_input,
            sync_r2_secret_access_key_input,
            sync_encryption_password_input,
            sync_provider,
            sync_remember_encryption_password,
            sync_ui_state: crate::app::config_sync::SyncUiState::default(),
            sync_in_progress: false,
            sync_status: sync_credential_load_error
                .map(|error| format!("{}: {error}", t!("sync_failed")).into())
                .unwrap_or_else(|| t!("sync_not_run").into()),
            sftp_path_input,
            ssh_auth_method: AuthMethod::Password,
            ssh_config_entries: crate::session::ssh_config::parse_ssh_config().unwrap_or_default(),
            ssh_config_selected: None,
            editing_session_id: None,
            editing_session_folder_id: None,
            editing_session_folder_members: std::collections::HashSet::new(),
            collapsed_session_folder_ids: std::collections::HashSet::new(),
            follow_system_theme,
            theme_mode,
            light_theme_name,
            dark_theme_name,
            ui_font_size: config.ui_font_size(),
            terminal_font_size: config.terminal_font_size(),
            terminal_cell_width: crate::app::constants::terminal_cell_width_from_measurement(
                f32::NAN,
                config.terminal_font_size(),
            ),
            terminal_zoom_accumulator: 0.0,
            cursor_style: config.cursor_style(),
            ui_font_family,
            terminal_font_family,
            tabs: Vec::new(),
            active_tab: None,
            tab_groups: Vec::new(),
            active_group: None,
            workspace_tabs: Vec::new(),
            active_workspace_tab: None,
            documents: std::collections::HashMap::new(),
            detaching_document_ids: HashSet::new(),
            detached_document_windows: HashMap::new(),
            closing_application: false,
            allow_window_close: false,
            window_close_prompt_open: false,
            window_close_save_queue: Vec::new(),
            window_close_save_current: None,
            pane_root: PaneLayout::empty(),
            focused_pane_path: Vec::new(),
            terminal_panel_bounds: None,
            split_container_bounds: HashMap::new(),
            selector_selection: 0,
            workspace_panels,
            body_panels,
            is_layout_reset: false,
            terminal_scrollbars: HashMap::new(),
            remote_files_scroll_handle: UniformListScrollHandle::new(),
            disk_scroll_handle: gpui::ScrollHandle::new(),
            tabs_scroll_handle: gpui::ScrollHandle::new(),
            selector_scroll_handle: gpui::ScrollHandle::new(),
            saved_scroll_handle: gpui::ScrollHandle::new(),
            connection_scroll_handle: gpui::ScrollHandle::new(),
            connection_progress: None,
            pending_sftp_path_sync: Some("/".into()),
            sftp_context_menu: None,
            sftp_creating_folder: false,
            sftp_new_folder_input,
            sftp_delete_scroll_handle: gpui::ScrollHandle::new(),
            show_hidden_files: config.show_hidden_files(),
            transfers: {
                let mut transfers = config.transfers();
                for t in transfers.iter_mut() {
                    if matches!(
                        t.state,
                        crate::terminal::TransferState::Running
                            | crate::terminal::TransferState::Paused
                    ) {
                        t.state =
                            crate::terminal::TransferState::Zombie(t!("zombie_reason").to_string());
                    }
                }
                transfers
            },
            show_transfers_dialog: false,
            system_status: None,
            terminal_bounds: HashMap::new(),
            terminal_selecting: false,
            terminal_marked_text: None,
            dragging_splitter: None,
            drag_split_origin: None,
            sftp_panel_minimized: config.sftp_panel_minimized(),
            sidebar_collapsed: config.sidebar_collapsed(),
            prev_monitoring_size: None,
            status: "ready".into(),
            config,
            startup_config_error,
            system_sampler,
            recording_action: None,
            active_dialog: None,
            keybind_error: None,
            keybinds_suspended: false,
            system,
            cpu_history: Vec::with_capacity(20),
            net_rx_history: Vec::with_capacity(20),
            net_tx_history: Vec::with_capacity(20),
            last_system_sample: Instant::now(),
            last_theme_sync: Instant::now(),

            search_input,
            search_active: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_current: 0,
            search_target_tab: None,
            search_bar_bounds: None,

            command_history_input,
            command_bar_input,
            command_bar_open: false,
            command_history_panel_open: false,
            command_history_target_tab: None,
            command_history_by_tab: HashMap::new(),
            command_history_scroll_handle: gpui::ScrollHandle::new(),

            system_tab_id: None,
            sftp_handles: std::collections::HashMap::new(),

            remote_sample_in_flight: false,
            runtime: Runtime::new().expect("create tokio runtime"),
            events_rx,
            events_tx,
            last_window_size: None,
            last_sidebar_width,
            hovered_url: None,
            terminal_link_ctrl_pressed: false,
            _subscriptions,
            config_writes: ConfigWriteCoordinator::default(),
        };

        this.apply_theme_preferences(window, cx);
        // this.open_local(cx);
        this.start_event_pump(cx);
        this
    }

    pub(crate) fn apply_loaded_config(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_system_theme = self.config.follow_system_theme();
        self.theme_mode = match self.config.theme_mode() {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::Dark,
        };
        self.light_theme_name = self.config.light_theme_name().to_string().into();
        self.dark_theme_name = self.config.dark_theme_name().to_string().into();
        self.ui_font_size = self.config.ui_font_size();
        self.terminal_font_size = self.config.terminal_font_size();
        self.cursor_style = self.config.cursor_style();
        self.ui_font_family = self.config.ui_font_family().to_string().into();
        self.terminal_font_family = self.config.terminal_font_family().to_string().into();
        self.show_hidden_files = self.config.show_hidden_files();
        self.sftp_panel_minimized = self.config.sftp_panel_minimized();
        self.sidebar_collapsed = self.config.sidebar_collapsed();
        let locale = self.config.locale().to_string();
        self.apply_runtime_display_language(&locale, window, cx);
        // Apply theme preferences
        self.apply_theme_preferences(window, cx);

        // Update inputs
        let proxy = global_proxy_form_values(&self.config);
        self.global_proxy_type = proxy.proxy_type;
        Self::set_input_value(&self.global_proxy_host_input, proxy.host, window, cx);
        Self::set_input_value(&self.global_proxy_port_input, proxy.port, window, cx);
        Self::set_input_value(&self.global_proxy_user_input, proxy.user, window, cx);
        Self::set_input_value(
            &self.global_proxy_password_input,
            proxy.password,
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_endpoint_input,
            self.config.sync_endpoint().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_username_input,
            self.config.sync_username().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_s3_endpoint_input,
            self.config.sync_s3_endpoint().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_s3_region_input,
            self.config.sync_s3_region().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_s3_bucket_input,
            self.config.sync_s3_bucket().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_s3_object_key_input,
            self.config.sync_s3_object_key().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_r2_account_id_input,
            self.config.sync_r2_account_id().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_r2_bucket_input,
            self.config.sync_r2_bucket().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_r2_object_key_input,
            self.config.sync_r2_object_key().to_string(),
            window,
            cx,
        );
        Self::set_input_value(
            &self.sync_r2_access_key_id_input,
            self.config.sync_r2_access_key_id().to_string(),
            window,
            cx,
        );
        self.sync_provider = self.config.sync_backend().to_string();
        self.sync_remember_encryption_password = self.config.sync_remember_encryption_password();

        // Notify
        cx.notify();
    }

    pub(crate) fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) && self.is_sync_input(input) {
            crate::app::config_sync::invalidate_pending_sync_actions(&mut self.sync_ui_state);
            self.sync_status = t!("sync_not_run").into();
        }
        if input == &self.sftp_path_input {
            if let InputEvent::PressEnter { .. } = event {
                let path = self
                    .sftp_path_input
                    .read(cx)
                    .text()
                    .to_string()
                    .trim()
                    .to_string();
                self.navigate_sftp(if path.is_empty() { "/".into() } else { path }, cx);
                window.prevent_default();
                cx.stop_propagation();
            }
        } else if input == &self.sftp_new_folder_input {
            match event {
                InputEvent::PressEnter { .. } => {
                    let name = self.sftp_new_folder_input.read(cx).text().to_string();
                    if !name.is_empty() {
                        let base_path = self.sftp_path_input.read(cx).text().to_string();
                        let path = crate::sftp::join_remote(&base_path, &name);
                        if let Some(handle) = self.active_sftp_handle() {
                            handle.send(crate::sftp::SftpCommand::CreateDir(path));
                        }
                    }
                    self.sftp_creating_folder = false;
                    window.prevent_default();
                    cx.stop_propagation();
                }
                InputEvent::Blur => {
                    self.sftp_creating_folder = false;
                }
                _ => {}
            }
        } else if input == &self.search_input {
            if let InputEvent::PressEnter { .. } = event {
                if self.search_query.is_empty()
                    || *self.search_input.read(cx).text() != self.search_query
                {
                    self.perform_search(window, cx);
                } else {
                    self.search_goto_next(cx);
                }
                window.prevent_default();
                cx.stop_propagation();
            }
        } else if input == &self.command_bar_input
            && let InputEvent::PressEnter { .. } = event
        {
            self.execute_command_bar_input(window, cx);
            window.prevent_default();
            cx.stop_propagation();
        }
        cx.notify();
    }

    pub(crate) fn save_preferences_background(&mut self) {
        let local_config = self.config.cache.clone();
        let config_store = self.config.clone();
        let coordinator = self.config_writes.clone();
        let current_seq = coordinator.begin_preference_save();

        self.runtime.spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                if let Some(Err(err)) = coordinator.run_preference_save(current_seq, || {
                    config_store.save_merged_preferences(local_config)
                }) {
                    tracing::error!("failed to save merged preferences in background: {err:#}");
                }
            })
            .await;
        });
    }

    pub(crate) fn save_config_now(&mut self) -> anyhow::Result<()> {
        let coordinator = self.config_writes.clone();
        coordinator.run_exclusive(|| self.config.save())
    }

    pub(crate) fn start_event_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut last_blink_time = std::time::Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let changed = this.drain_backend_events();
                        let system_sampled = this.sample_system_if_due();
                        this.sync_theme_if_due(cx);
                        let is_blinking = matches!(
                            this.cursor_style,
                            crate::session::config::CursorStyle::Blink
                                | crate::session::config::CursorStyle::BeamBlink
                        );
                        let now = std::time::Instant::now();
                        let blink_due = is_blinking
                            && now.duration_since(last_blink_time)
                                >= std::time::Duration::from_millis(600);
                        if changed || system_sampled || blink_due {
                            cx.notify();
                            if blink_due {
                                last_blink_time = now;
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn drain_backend_events(&mut self) -> bool {
        let mut changed = false;
        let mut transfers_changed = false;
        while let Ok(event) = self.events_rx.try_recv() {
            changed = true;
            match event {
                BackendEvent::Output {
                    tab_id,
                    generation,
                    bytes,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id)
                        && crate::terminal::backend_generation_matches(
                            tab.backend_generation,
                            generation,
                        )
                    {
                        tab.feed(&bytes);
                    }
                }
                BackendEvent::Status {
                    tab_id,
                    generation,
                    text,
                } => {
                    let accepted = self
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.id == tab_id)
                        .is_some_and(|tab| {
                            if crate::terminal::backend_generation_matches(
                                tab.backend_generation,
                                generation,
                            ) {
                                tab.status = text.clone();
                                true
                            } else {
                                false
                            }
                        });
                    if !accepted {
                        continue;
                    }
                    if let Some(progress) = self.connection_progress.as_mut()
                        && progress.tab_id == tab_id
                    {
                        progress.lines.push(text.clone().into());
                        let _idx = progress.lines.len().saturating_sub(1);
                        self.connection_scroll_handle
                            .set_offset(gpui::point(px(0.), px(-99999.0)));
                    }
                    self.status = text.into();
                }
                BackendEvent::Connected { tab_id, generation } => {
                    let accepted = self
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.id == tab_id)
                        .is_some_and(|tab| {
                            if !crate::terminal::backend_generation_matches(
                                tab.backend_generation,
                                generation,
                            ) {
                                return false;
                            }
                            tab.connected = true;
                            tab.disconnected_reason = None;
                            true
                        });
                    if !accepted {
                        continue;
                    }
                    self.sync_system_tab_to_active_group();
                    self.request_active_system_snapshot();
                    if self
                        .tabs
                        .iter()
                        .any(|tab| tab.id == tab_id && tab.kind == TabKind::Ssh)
                    {
                        self.start_command_history_load(&tab_id);
                    }
                    if self
                        .connection_progress
                        .as_ref()
                        .is_some_and(|progress| progress.tab_id == tab_id && !progress.failed)
                    {
                        self.connection_progress = None;
                    }
                }
                BackendEvent::SftpEntries {
                    tab_id,
                    generation,
                    path,
                    entries,
                } => {
                    if let Some(group) = self.tab_groups.iter_mut().find(|g| g.id == tab_id)
                        && let Some(sftp) = group.sftp.as_mut()
                        && sftp.accepts_generation(generation)
                        && sftp.current_path == path
                    {
                        sftp.current_path = path;
                        sftp.entries = entries;
                        self.pending_sftp_path_sync = Some(sftp.current_path.clone());
                    }
                }
                BackendEvent::SftpPreview {
                    tab_id,
                    generation,
                    preview,
                } => {
                    if let Some(group) = self.tab_groups.iter_mut().find(|g| g.id == tab_id)
                        && let Some(sftp) = group.sftp.as_mut()
                        && sftp.accepts_generation(generation)
                        && sftp.selected_path.as_deref() == Some(preview.path.as_str())
                    {
                        sftp.preview = Some(preview);
                    }
                }
                BackendEvent::SftpStatus {
                    tab_id,
                    generation,
                    text,
                } => {
                    let current_path = self
                        .tab_groups
                        .iter_mut()
                        .find(|group| group.id == tab_id)
                        .and_then(|group| group.sftp.as_mut())
                        .and_then(|sftp| {
                            if sftp.accepts_generation(generation) {
                                sftp.status = text.clone();
                                Some(sftp.current_path.clone())
                            } else {
                                None
                            }
                        });
                    let Some(current_path) = current_path else {
                        continue;
                    };
                    if text == rust_i18n::t!("sftp_connected") {
                        if let Some(handle) = self.sftp_handles.get(&tab_id) {
                            handle.list_dir(current_path);
                        }
                        for document in self
                            .documents
                            .values_mut()
                            .filter(|document| document.key.connection_id == tab_id)
                        {
                            document.connection_state =
                                crate::document::DocumentConnectionState::Online;
                        }
                    } else if text.starts_with(rust_i18n::t!("sftp_reconnecting").as_ref()) {
                        for document in self
                            .documents
                            .values_mut()
                            .filter(|document| document.key.connection_id == tab_id)
                        {
                            document.connection_state =
                                crate::document::DocumentConnectionState::Reconnecting;
                        }
                    }
                    if self.active_group.as_ref() == Some(&tab_id) {
                        self.status = text.into();
                    }
                }
                BackendEvent::SftpDeleteFinished {
                    tab_id,
                    generation,
                    paths,
                    deleted_paths,
                } => {
                    if let Some(group) = self.tab_groups.iter_mut().find(|g| g.id == tab_id)
                        && let Some(sftp) = group.sftp.as_mut()
                        && sftp.accepts_generation(generation)
                    {
                        crate::sftp::ops::apply_sftp_delete_result(
                            &mut sftp.deleting_entries,
                            &mut sftp.selected_entries,
                            &paths,
                            &deleted_paths,
                        );
                    }
                }
                BackendEvent::RemoteSystem { tab_id, snapshot } => {
                    self.remote_sample_in_flight = false;
                    if self.system_tab_id.as_deref() == Some(tab_id.as_str()) {
                        self.system_status = None;
                        self.system = snapshot.clone();
                        self.cpu_history.push(snapshot.cpu_percent);
                        if self.cpu_history.len() > 20 {
                            self.cpu_history.remove(0);
                        }
                        self.net_rx_history.push(snapshot.net_rx_rate as f32);
                        if self.net_rx_history.len() > 20 {
                            self.net_rx_history.remove(0);
                        }
                        self.net_tx_history.push(snapshot.net_tx_rate as f32);
                        if self.net_tx_history.len() > 20 {
                            self.net_tx_history.remove(0);
                        }
                    }
                }
                BackendEvent::RemoteSystemUnavailable { tab_id, reason } => {
                    self.remote_sample_in_flight = false;
                    if self.system_tab_id.as_deref() == Some(tab_id.as_str()) {
                        self.system_status = Some(reason.clone().into());
                        self.status = reason.into();
                    }
                }
                BackendEvent::CommandHistory { tab_id, entries } => {
                    let history = self.command_history_by_tab.entry(tab_id).or_default();
                    history.entries = entries;
                    history.loading = false;
                    history.error = None;
                }
                BackendEvent::CommandHistoryUnavailable { tab_id, reason } => {
                    let history = self.command_history_by_tab.entry(tab_id).or_default();
                    history.loading = false;
                    history.error = Some(reason);
                }
                BackendEvent::Closed {
                    tab_id,
                    generation,
                    reason,
                } => {
                    self.remote_sample_in_flight = false;
                    let is_current = self
                        .tabs
                        .iter()
                        .find(|t| t.id == tab_id)
                        .is_some_and(|tab| {
                            crate::terminal::backend_generation_matches(
                                tab.backend_generation,
                                generation,
                            )
                        });
                    if !is_current {
                        continue;
                    }
                    let is_graceful_exit =
                        reason == "local shell closed" || reason == "ssh session closed";
                    if is_graceful_exit {
                        self.handle_tab_close(tab_id.clone());
                        self.status = reason.into();
                        continue;
                    }
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.connected = false;
                        tab.status = reason.clone();
                        tab.disconnected_reason = Some(reason.clone());
                    }
                    if self.system_tab_id.as_deref() == Some(tab_id.as_str()) {
                        self.system_status = Some(reason.clone().into());
                    }
                    if let Some(progress) = self.connection_progress.as_mut()
                        && progress.tab_id == tab_id
                    {
                        progress.lines.push(reason.clone().into());
                        let _idx = progress.lines.len().saturating_sub(1);
                        self.connection_scroll_handle
                            .set_offset(gpui::point(px(0.), px(-99999.0)));
                        progress.title = t!("connection_failed").into();
                        progress.failed = true;
                    }
                    self.status = reason.into();
                }
                BackendEvent::TransferProgress {
                    tab_id,
                    generation,
                    id,
                    transferred,
                    total,
                    state,
                } => {
                    if let Some(t) = self.transfers.iter_mut().find(|transfer| {
                        transfer.tab_id == tab_id
                            && transfer.generation == generation
                            && transfer.info.id == id
                    }) {
                        t.transferred = transferred;
                        if let Some(total) = total {
                            t.total = Some(total);
                        }
                        t.state = state;
                        transfers_changed = true;
                    }
                }
                BackendEvent::TransferStarted {
                    tab_id,
                    generation,
                    info,
                } => {
                    let accepted = self
                        .tab_groups
                        .iter()
                        .find(|group| group.id == tab_id)
                        .and_then(|group| group.sftp.as_ref())
                        .is_some_and(|sftp| sftp.accepts_generation(generation));
                    if !accepted {
                        continue;
                    }
                    let tab_title = self.transfer_source_title(&tab_id);
                    self.transfers.insert(
                        0,
                        crate::terminal::Transfer {
                            tab_id,
                            generation,
                            tab_title,
                            info,
                            transferred: 0,
                            total: None,
                            state: crate::terminal::TransferState::Running,
                        },
                    );
                    if self.transfers.len() > 100 {
                        self.transfers.truncate(100);
                    }
                    transfers_changed = true;
                }
                BackendEvent::SftpHome {
                    tab_id,
                    generation,
                    home,
                } => {
                    if let Some(group) = self.tab_groups.iter_mut().find(|g| g.id == tab_id)
                        && let Some(sftp) = group.sftp.as_mut()
                        && sftp.accepts_generation(generation)
                    {
                        sftp.apply_home(home);
                        self.pending_sftp_path_sync = Some(sftp.current_path.clone());
                    }
                }
                BackendEvent::SftpGeneration { tab_id, generation } => {
                    let accepted = self
                        .tab_groups
                        .iter_mut()
                        .find(|group| group.id == tab_id)
                        .and_then(|group| group.sftp.as_mut())
                        .is_some_and(|sftp| sftp.begin_generation(generation));
                    if accepted {
                        for transfer in self.transfers.iter_mut().filter(|transfer| {
                            transfer.tab_id == tab_id && transfer.generation < generation
                        }) {
                            if matches!(
                                transfer.state,
                                crate::terminal::TransferState::Running
                                    | crate::terminal::TransferState::Paused
                            ) {
                                transfer.state = crate::terminal::TransferState::Interrupted(
                                    "SFTP connection restarted".to_string(),
                                );
                                transfers_changed = true;
                            }
                        }
                    }
                }
                BackendEvent::TerminalTitleChanged { tab_id, title } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.dynamic_title = title;
                    }
                }
                BackendEvent::SyncFinished(result) => {
                    self.handle_sync_finished(*result);
                }
            }
        }
        if transfers_changed {
            self.config.set_transfers(self.transfers.clone());
            if let Err(err) = self.save_config_now() {
                tracing::error!("failed to save transfers: {err:#}");
            }
        }
        changed
    }

    pub(crate) fn sample_system_if_due(&mut self) -> bool {
        if self.last_system_sample.elapsed() >= SystemSampler::interval() {
            self.last_system_sample = Instant::now();
            // Use system_tab_id (not active_tab) to decide remote vs local sampling
            if let Some(ref tab_id) = self.system_tab_id.clone()
                && self
                    .tabs
                    .iter()
                    .any(|t| t.id == *tab_id && t.kind == TabKind::Ssh && t.connected)
                && self.system_status.is_none()
            {
                self.request_active_system_snapshot();
                return false;
            }
            let snapshot = self.system_sampler.sample();
            let cpu_usage = snapshot.cpu_percent;
            self.cpu_history.push(cpu_usage);
            if self.cpu_history.len() > 20 {
                self.cpu_history.remove(0);
            }
            self.net_rx_history.push(snapshot.net_rx_rate as f32);
            if self.net_rx_history.len() > 20 {
                self.net_rx_history.remove(0);
            }
            self.net_tx_history.push(snapshot.net_tx_rate as f32);
            if self.net_tx_history.len() > 20 {
                self.net_tx_history.remove(0);
            }
            self.system = snapshot;
            return true;
        }
        false
    }

    pub(crate) fn sync_theme_if_due(&mut self, cx: &mut Context<Self>) {
        if self.follow_system_theme && self.last_theme_sync.elapsed() >= Duration::from_secs(1) {
            self.last_theme_sync = Instant::now();
            Theme::sync_system_appearance(None, cx);
            cx.refresh_windows();
        }
    }

    pub(crate) fn request_active_system_snapshot(&mut self) {
        let Some(ref tab_id) = self.system_tab_id.clone() else {
            return;
        };
        let Some(backend) = (|| {
            let tab = self.tabs.iter().find(|t| t.id == *tab_id)?;
            if !tab.connected {
                return None;
            }
            Some(tab.backend.clone())
        })() else {
            return;
        };
        if self.remote_sample_in_flight {
            return;
        }
        self.remote_sample_in_flight = true;
        if let Ok(backend) = backend.lock() {
            backend.send(crate::terminal::BackendCommand::SampleMetrics);
        }
    }

    pub(crate) fn terminal_ime_bounds_for_range(
        &self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        cell_width: f32,
        line_height: f32,
    ) -> Option<Bounds<Pixels>> {
        let snapshot = self.active_snapshot()?;
        let cursor = snapshot.cursor?;
        let x = element_bounds.origin.x
            + px(cell_width) * cursor.col as f32
            + px(cell_width) * range_utf16.start as f32;
        let y = element_bounds.origin.y + px(line_height) * cursor.row as f32;
        Some(Bounds::new(
            point(x, y),
            size(px(cell_width), px(line_height)),
        ))
    }

    pub(crate) fn remove_transfer(&mut self, transfer_id: &str, cx: &mut Context<Self>) {
        self.transfers.retain(|t| t.info.id != transfer_id);
        self.config.set_transfers(self.transfers.clone());
        if let Err(err) = self.save_config_now() {
            tracing::error!("failed to save transfers: {err:#}");
        }
        cx.notify();
    }

    pub(crate) fn retry_connection_progress(&mut self, cx: &mut Context<Self>) {
        let Some(progress) = self.connection_progress.clone() else {
            return;
        };
        self.connection_progress = None;
        let mut retry_tabs = Vec::new();
        for (ix, tab) in self.tabs.iter().enumerate() {
            if !tab.connected && tab.session.is_some() && tab.id == progress.tab_id {
                retry_tabs.push((ix, tab.id.clone(), tab.session.clone().unwrap(), tab.kind));
            }
        }

        if retry_tabs.is_empty() {
            cx.notify();
            return;
        }

        for (ix, tab_id, session, tab_kind) in retry_tabs {
            let proxy_config = self.config.connection_proxy_config();
            let new_generation = self.tabs[ix].backend_generation.saturating_add(1);
            // Close old backend
            self.tabs[ix].send_backend(crate::terminal::BackendCommand::Close);

            // Spawn new backend
            let backend = match tab_kind {
                crate::terminal::TabKind::Serial => {
                    let b = crate::backend::serial::spawn_serial_client(
                        self.runtime.handle(),
                        tab_id.clone(),
                        new_generation,
                        session.clone(),
                        self.events_tx.clone(),
                    );
                    crate::terminal::BackendTx::Serial(b)
                }
                crate::terminal::TabKind::Ssh => crate::backend::ssh::spawn_ssh_terminal(
                    self.runtime.handle(),
                    tab_id.clone(),
                    new_generation,
                    session.clone(),
                    proxy_config.clone(),
                    self.tabs[ix].cols,
                    self.tabs[ix].rows,
                    self.events_tx.clone(),
                ),
                _ => continue,
            };

            // Replace tab state
            self.tabs[ix].set_backend(backend);
            self.tabs[ix].connected = false;
            self.tabs[ix].status = "connecting".into();
            self.tabs[ix].disconnected_reason = None;
            self.tabs[ix].backend_generation = new_generation;

            // Restart SFTP for the group containing this tab
            if let Some(group) = self
                .tab_groups
                .iter()
                .find(|g| g.pane_root.contains(&tab_id))
            {
                let group_id = group.id.clone();
                if let Some(handle) = self.sftp_handles.get(&group_id) {
                    handle.reconnect_now();
                    if let Some(group) = self.tab_groups.iter_mut().find(|g| g.id == group_id)
                        && let Some(sftp) = group.sftp.as_mut()
                    {
                        sftp.status = rust_i18n::t!("sftp_reconnecting").to_string();
                    }
                }
            }
        }

        self.connection_progress = Some(ConnectionProgress {
            tab_id: progress.tab_id.clone(),
            title: t!("connecting").into(),
            lines: vec![t!("starting_connection").into()],
            failed: false,
        });
        self.status = "ssh tabs retrying".into();
        cx.notify();
    }

    pub(crate) fn cancel_connection_progress(&mut self, cx: &mut Context<Self>) {
        if let Some(progress) = &self.connection_progress {
            let tab_id = progress.tab_id.clone();
            self.connection_progress = None;
            self.handle_tab_close(tab_id);
        }
        cx.notify();
    }

    pub(crate) fn sync_cwd_from_terminal(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut Context<Self>,
    ) {
        let active_id = self.active_tab.clone();
        let Some(active_id) = active_id else {
            return;
        };

        if let Some(tab) = self.tabs.iter().find(|t| t.id == active_id) {
            let home_dir = if let Some(group) = self
                .tab_groups
                .iter()
                .find(|g| g.pane_root.contains(&tab.id))
            {
                group
                    .sftp
                    .as_ref()
                    .map(|s| s.home_dir.as_str())
                    .unwrap_or("/")
            } else {
                "/"
            };

            let parsed = Self::parse_path_from_title(&tab.dynamic_title, home_dir);

            if let Some(path) = parsed
                && let Some(group) = self
                    .tab_groups
                    .iter_mut()
                    .find(|g| g.pane_root.contains(&active_id))
                && let Some(sftp) = group.sftp.as_mut()
            {
                sftp.current_path = path.clone();
                self.pending_sftp_path_sync = Some(path.clone());
                if let Some(handle) = self.sftp_handles.get(&group.id) {
                    handle.send(crate::sftp::SftpCommand::ListDir(path));
                }
            }
        }
    }

    fn parse_path_from_title(title: &str, home_dir: &str) -> Option<String> {
        let title = title.strip_prefix("ASHELL_CWD:").unwrap_or(title);
        let path_part = if let Some(pos) = title.find(':') {
            title[pos + 1..].trim()
        } else {
            title.trim()
        };

        if path_part.starts_with('/') {
            Some(path_part.to_string())
        } else if path_part == "~" {
            Some(home_dir.to_string())
        } else if let Some(rest) = path_part.strip_prefix("~/") {
            let home = home_dir.trim_end_matches('/');
            Some(format!("{}/{}", home, rest))
        } else {
            None
        }
    }

    pub(crate) fn save_layout_state(&self, window: &mut gpui::Window, cx: &gpui::App) {
        if self.is_layout_reset {
            tracing::info!("[ui] layout was reset, skipping save layout state.");
            return;
        }
        let current_bounds = window.window_bounds();
        let bounds = match current_bounds {
            gpui::WindowBounds::Fullscreen(b) => b,
            gpui::WindowBounds::Maximized(b) => b,
            gpui::WindowBounds::Windowed(b) => b,
        };
        let size = bounds.size;
        if size.width.as_f32() > 400.0 && size.height.as_f32() > 300.0 {
            let mut config = self.config.clone();
            let saved_bounds = match current_bounds {
                gpui::WindowBounds::Fullscreen(b) => {
                    crate::session::config::SavedWindowBounds::Fullscreen {
                        x: b.origin.x.into(),
                        y: b.origin.y.into(),
                        width: b.size.width.into(),
                        height: b.size.height.into(),
                    }
                }
                gpui::WindowBounds::Maximized(b) => {
                    let mut restore_bounds = (
                        b.origin.x.into(),
                        b.origin.y.into(),
                        b.size.width.into(),
                        b.size.height.into(),
                    );
                    if let Some(existing_bounds) = config.window_bounds() {
                        match existing_bounds {
                            crate::session::config::SavedWindowBounds::Windowed {
                                x,
                                y,
                                width,
                                height,
                            } => {
                                restore_bounds = (*x, *y, *width, *height);
                            }
                            crate::session::config::SavedWindowBounds::Maximized {
                                x,
                                y,
                                width,
                                height,
                            } => {
                                restore_bounds = (*x, *y, *width, *height);
                            }
                            _ => {}
                        }
                    }
                    crate::session::config::SavedWindowBounds::Maximized {
                        x: restore_bounds.0,
                        y: restore_bounds.1,
                        width: restore_bounds.2,
                        height: restore_bounds.3,
                    }
                }
                gpui::WindowBounds::Windowed(b) => {
                    crate::session::config::SavedWindowBounds::Windowed {
                        x: b.origin.x.into(),
                        y: b.origin.y.into(),
                        width: b.size.width.into(),
                        height: b.size.height.into(),
                    }
                }
            };
            let workspace_sizes: Vec<f32> = self
                .workspace_panels
                .read(cx)
                .sizes()
                .iter()
                .map(|s| s.into())
                .collect();
            let mut body_sizes: Vec<f32> = self
                .body_panels
                .read(cx)
                .sizes()
                .iter()
                .map(|s| s.into())
                .collect();

            if self.sftp_panel_minimized
                && let Some(prev) = self.prev_monitoring_size
                && body_sizes.len() > 1
            {
                body_sizes[1] = prev.into();
            }

            config.set_layout_state(Some(saved_bounds), Some(workspace_sizes), Some(body_sizes));
            config.set_sidebar_collapsed(self.sidebar_collapsed);
            config.set_sftp_panel_minimized(self.sftp_panel_minimized);
            let coordinator = self.config_writes.clone();
            let _ = coordinator.run_exclusive(|| config.save());
        } else {
            tracing::warn!(
                "[ui] window size is too small ({:?}), skipping save layout state to prevent corrupting saved bounds.",
                size
            );
        }
    }
}
