use std::{fs, path::PathBuf, sync::OnceLock};

use anyhow::{Context, Result};
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use directories::BaseDirs;
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Password,
    Key,
    Config,
}

fn default_protocol() -> String {
    "ssh".to_string()
}

fn default_baud_rate() -> u32 {
    115200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub private_key_path: String,
    #[serde(default)]
    pub private_key_inline: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(default)]
    pub last_used: Option<String>,
    #[serde(default = "default_global_proxy_type")]
    pub proxy_type: String, // "none", "socks5", "http"
    #[serde(default)]
    pub proxy_host: String,
    #[serde(default)]
    pub proxy_port: Option<u16>,
    #[serde(default)]
    pub proxy_user: String,
    #[serde(default)]
    pub proxy_password: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub session_ids: Vec<String>,
}

impl SessionFolder {
    pub fn new(name: String, session_ids: Vec<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            session_ids,
        }
    }
}

impl Session {
    pub fn password(host: String, port: u16, user: String, password: String) -> Self {
        let name = format!("{user}@{host}");
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            host,
            port,
            user,
            auth: AuthMethod::Password,
            password,
            private_key_path: String::new(),
            private_key_inline: String::new(),
            passphrase: String::new(),
            last_used: None,
            proxy_type: "none".to_string(),
            proxy_host: String::new(),
            proxy_port: None,
            proxy_user: String::new(),
            proxy_password: String::new(),
            protocol: "ssh".to_string(),
            baud_rate: 115200,
        }
    }

    pub fn key(
        host: String,
        port: u16,
        user: String,
        private_key_path: String,
        private_key_inline: String,
        passphrase: String,
    ) -> Self {
        let name = format!("{user}@{host}");
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            host,
            port,
            user,
            auth: AuthMethod::Key,
            password: String::new(),
            private_key_path,
            private_key_inline,
            passphrase,
            last_used: None,
            proxy_type: "none".to_string(),
            proxy_host: String::new(),
            proxy_port: None,
            proxy_user: String::new(),
            proxy_password: String::new(),
            protocol: "ssh".to_string(),
            baud_rate: 115200,
        }
    }

    pub fn serial(port_name: String, baud_rate: u32) -> Self {
        let name = format!("serial://{port_name}@{baud_rate}");
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            host: port_name,
            port: 0,
            user: String::new(),
            auth: AuthMethod::Password,
            password: String::new(),
            private_key_path: String::new(),
            private_key_inline: String::new(),
            passphrase: String::new(),
            last_used: None,
            proxy_type: "none".to_string(),
            proxy_host: String::new(),
            proxy_port: None,
            proxy_user: String::new(),
            proxy_password: String::new(),
            protocol: "serial".to_string(),
            baud_rate,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SavedWindowBounds {
    Fullscreen {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Maximized {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Windowed {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CursorStyle {
    #[default]
    Default,
    Blink,
    Beam,
    BeamBlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub font_defaults_version: u8,
    #[serde(default = "default_follow_system_theme")]
    pub follow_system_theme: bool,
    #[serde(default)]
    pub theme_mode: String,
    #[serde(default)]
    pub light_theme_name: String,
    #[serde(default)]
    pub dark_theme_name: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f32,
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: f32,
    #[serde(default)]
    pub right_click_copy_paste: bool,
    #[serde(default)]
    pub keyword_highlight: bool,
    #[serde(default = "default_history_completion_plugin_enabled")]
    pub history_completion_plugin_enabled: bool,
    #[serde(default = "default_ui_font_family")]
    pub ui_font_family: String,
    #[serde(default = "default_terminal_font_family")]
    pub terminal_font_family: String,
    #[serde(default)]
    pub cursor_style: CursorStyle,
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub session_folders: Vec<SessionFolder>,
    #[serde(default)]
    pub window_bounds: Option<SavedWindowBounds>,
    #[serde(default)]
    pub workspace_panels: Option<Vec<f32>>,
    #[serde(default)]
    pub body_panels: Option<Vec<f32>>,
    #[serde(default)]
    pub transfers: Vec<crate::terminal::Transfer>,
    #[serde(default)]
    pub show_hidden_files: bool,
    #[serde(default)]
    pub lock_layout: bool,
    #[serde(default = "default_monitoring_position")]
    pub monitoring_position: String,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub sftp_panel_minimized: bool,
    #[serde(default)]
    pub key_bindings: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub sync_endpoint: String,
    #[serde(default)]
    pub sync_username: String,
    #[serde(default)]
    pub sync_etag: Option<String>,
    #[serde(default)]
    pub sync_device_id: String,
    #[serde(default)]
    pub sync_backend: String,
    #[serde(default)]
    pub sync_etag_backend: String,
    #[serde(default)]
    pub sync_s3_endpoint: String,
    #[serde(default = "default_s3_region")]
    pub sync_s3_region: String,
    #[serde(default)]
    pub sync_s3_bucket: String,
    #[serde(default = "default_s3_object_key")]
    pub sync_s3_object_key: String,
    #[serde(default)]
    pub use_proxy: bool,
    #[serde(default = "default_read_env_proxy")]
    pub read_env_proxy: bool,
    #[serde(default = "default_global_proxy_type")]
    pub global_proxy_type: String,
    #[serde(default)]
    pub global_proxy_host: String,
    #[serde(default)]
    pub global_proxy_port: Option<u16>,
    #[serde(default)]
    pub global_proxy_user: String,
    #[serde(default)]
    pub global_proxy_password: String,
}

fn default_read_env_proxy() -> bool {
    true
}

fn default_history_completion_plugin_enabled() -> bool {
    true
}

fn default_global_proxy_type() -> String {
    "socks5".to_string()
}

fn default_monitoring_position() -> String {
    "Sidebar".to_string()
}

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

fn default_s3_object_key() -> String {
    "jshell-sync.json".to_string()
}

fn default_follow_system_theme() -> bool {
    true
}

fn default_locale() -> String {
    "system".to_string()
}

fn default_terminal_font_size() -> f32 {
    16.0
}

fn default_ui_font_size() -> f32 {
    14.0
}

pub(crate) const MIN_UI_FONT_SIZE: f32 = 10.0;
pub(crate) const MAX_UI_FONT_SIZE: f32 = 18.0;

pub(crate) fn clamp_ui_font_size(ui_font_size: f32) -> f32 {
    if ui_font_size <= 0.0 {
        default_ui_font_size()
    } else {
        ui_font_size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
    }
}

const CURRENT_FONT_DEFAULTS_VERSION: u8 = 4;
pub(crate) const SYSTEM_MONOSPACE_FONT: &str = ".SystemMonospace";

pub fn default_ui_font_family() -> String {
    "Noto Sans CJK SC".to_string()
}

fn default_terminal_font_family() -> String {
    SYSTEM_MONOSPACE_FONT.to_string()
}

fn migrate_legacy_font_preferences(config: &mut ConfigFile) -> bool {
    if config.font_defaults_version >= CURRENT_FONT_DEFAULTS_VERSION {
        return false;
    }

    if config.font_defaults_version < 1 {
        if matches!(
            config.ui_font_family.as_str(),
            "Maple Mono NF CN"
                | "Noto Sans SC"
                | "Noto Sans CJK SC"
                | "Noto Sans CJK SC Black"
                | "Noto Sans Mono CJK SC"
        ) {
            config.ui_font_family = "Noto Sans CJK SC".to_string();
        }
        if matches!(
            config.terminal_font_family.as_str(),
            "Maple Mono NF CN" | "Noto Sans SC" | "Noto Sans CJK SC"
        ) {
            config.terminal_font_family = "Noto Sans Mono CJK SC".to_string();
        }
    }

    if config.font_defaults_version < 2 && (config.terminal_font_size - 18.0).abs() < f32::EPSILON {
        config.terminal_font_size = default_terminal_font_size();
    }
    if config.font_defaults_version < 3 && (config.terminal_font_size - 15.0).abs() < f32::EPSILON {
        config.terminal_font_size = default_terminal_font_size();
    }
    if config.font_defaults_version < 4 && config.terminal_font_family == "Noto Sans Mono CJK SC" {
        config.terminal_font_family = default_terminal_font_family();
    }

    config.font_defaults_version = CURRENT_FONT_DEFAULTS_VERSION;
    true
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            font_defaults_version: CURRENT_FONT_DEFAULTS_VERSION,
            follow_system_theme: default_follow_system_theme(),
            theme_mode: String::new(),
            light_theme_name: String::new(),
            dark_theme_name: String::new(),
            locale: default_locale(),
            terminal_font_size: default_terminal_font_size(),
            ui_font_size: default_ui_font_size(),
            right_click_copy_paste: false,
            keyword_highlight: false,
            history_completion_plugin_enabled: default_history_completion_plugin_enabled(),
            ui_font_family: default_ui_font_family(),
            terminal_font_family: default_terminal_font_family(),
            cursor_style: CursorStyle::default(),
            sessions: Vec::new(),
            session_folders: Vec::new(),
            window_bounds: None,
            workspace_panels: None,
            body_panels: None,
            transfers: Vec::new(),
            show_hidden_files: false,
            lock_layout: false,
            monitoring_position: default_monitoring_position(),
            sidebar_collapsed: false,
            sftp_panel_minimized: false,
            key_bindings: std::collections::HashMap::new(),
            sync_endpoint: String::new(),
            sync_username: String::new(),
            sync_etag: None,
            sync_device_id: String::new(),
            sync_backend: String::new(),
            sync_etag_backend: String::new(),
            sync_s3_endpoint: String::new(),
            sync_s3_region: default_s3_region(),
            sync_s3_bucket: String::new(),
            sync_s3_object_key: default_s3_object_key(),
            use_proxy: false,
            read_env_proxy: true,
            global_proxy_type: default_global_proxy_type(),
            global_proxy_host: String::new(),
            global_proxy_port: None,
            global_proxy_user: String::new(),
            global_proxy_password: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct ConfigStore {
    pub(crate) path: PathBuf,
    pub(crate) cache: ConfigFile,
}

impl ConfigStore {
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let legacy_path = Self::legacy_config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config dir {}", parent.display()))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mut perms) = fs::metadata(parent).map(|m| m.permissions()) {
                    perms.set_mode(0o700);
                    let _ = fs::set_permissions(parent, perms);
                }
            }

            let tmp_dir = parent.join("tmp");
            let _ = fs::remove_dir_all(&tmp_dir);
            let _ = fs::create_dir_all(&tmp_dir);
        }

        let source_path = if path.exists() {
            path.clone()
        } else if legacy_path.exists() {
            legacy_path
        } else {
            path.clone()
        };
        let migrated_config_location = source_path != path;

        let mut cache = if source_path.exists() {
            let raw_bytes = fs::read(&source_path)
                .with_context(|| format!("failed to read {}", source_path.display()))?;
            let hardware_uuid = get_hardware_uuid();
            match decrypt_config(&raw_bytes, &hardware_uuid) {
                Ok(cache) => cache,
                Err(decrypt_err) => {
                    // Fallback to plain text JSON if decryption/parsing failed
                    match serde_json::from_slice::<ConfigFile>(&raw_bytes) {
                        Ok(cache) => cache,
                        Err(json_err) => {
                            let backup_path = source_path.with_extension("json.bak");
                            if let Err(backup_err) = fs::write(&backup_path, &raw_bytes) {
                                tracing::warn!(
                                    "failed to parse config {} (decrypt err: {decrypt_err:#}, json err: {json_err:#}); backup to {} also failed: {backup_err:#}",
                                    source_path.display(),
                                    backup_path.display(),
                                );
                            } else {
                                tracing::warn!(
                                    "failed to parse config {} (decrypt err: {decrypt_err:#}, json err: {json_err:#}); backed up the original to {} and loaded defaults",
                                    source_path.display(),
                                    backup_path.display(),
                                );
                            }
                            ConfigFile::default()
                        }
                    }
                }
            }
        } else {
            ConfigFile::default()
        };

        if cache.sync_device_id.is_empty() {
            cache.sync_device_id = Uuid::new_v4().to_string();
        }
        let migrated_font_preferences = migrate_legacy_font_preferences(&mut cache);
        let store = Self { path, cache };
        if (migrated_font_preferences || migrated_config_location)
            && let Err(err) = store.save()
        {
            tracing::warn!("failed to persist migrated font preferences: {err:#}");
        }
        Ok(store)
    }

    pub fn in_memory() -> Self {
        let cache = ConfigFile {
            sync_device_id: Uuid::new_v4().to_string(),
            ..ConfigFile::default()
        };
        Self {
            path: PathBuf::new(),
            cache,
        }
    }

    fn config_path() -> Result<PathBuf> {
        let dirs = BaseDirs::new().context("could not determine user home directory")?;
        Ok(dirs
            .home_dir()
            .join(".config")
            .join("jshell")
            .join("sessions.json"))
    }

    fn legacy_config_path() -> Result<PathBuf> {
        let dirs = BaseDirs::new().context("could not determine user home directory")?;
        Ok(dirs
            .home_dir()
            .join(".config")
            .join("ashell")
            .join("sessions.json"))
    }

    pub fn sessions(&self) -> &[Session] {
        &self.cache.sessions
    }

    pub fn session_folders(&self) -> &[SessionFolder] {
        &self.cache.session_folders
    }

    pub fn session_folder_sessions(&self, folder_id: &str) -> Vec<Session> {
        let Some(folder) = self
            .cache
            .session_folders
            .iter()
            .find(|folder| folder.id == folder_id)
        else {
            return Vec::new();
        };
        let sessions_by_id: std::collections::HashMap<_, _> = self
            .cache
            .sessions
            .iter()
            .map(|session| (session.id.as_str(), session))
            .collect();
        folder
            .session_ids
            .iter()
            .filter_map(|session_id| sessions_by_id.get(session_id.as_str()))
            .map(|session| (*session).clone())
            .collect()
    }

    pub fn replace_sessions(&mut self, sessions: Vec<Session>) {
        self.cache.sessions = sessions;
    }

    pub fn sync_endpoint(&self) -> &str {
        &self.cache.sync_endpoint
    }

    pub fn sync_username(&self) -> &str {
        &self.cache.sync_username
    }

    pub fn sync_etag(&self) -> Option<&str> {
        (self.cache.sync_etag_backend == self.sync_backend())
            .then_some(self.cache.sync_etag.as_deref())
            .flatten()
    }

    pub fn sync_device_id(&self) -> &str {
        &self.cache.sync_device_id
    }

    pub fn sync_backend(&self) -> &str {
        if self.cache.sync_backend == "s3" {
            "s3"
        } else {
            "webdav"
        }
    }

    pub fn set_sync_backend(&mut self, backend: &str) {
        self.cache.sync_backend = if backend == "s3" { "s3" } else { "webdav" }.to_string();
    }

    pub fn sync_s3_endpoint(&self) -> &str {
        &self.cache.sync_s3_endpoint
    }

    pub fn sync_s3_region(&self) -> &str {
        if self.cache.sync_s3_region.is_empty() {
            "us-east-1"
        } else {
            &self.cache.sync_s3_region
        }
    }

    pub fn sync_s3_bucket(&self) -> &str {
        &self.cache.sync_s3_bucket
    }

    pub fn sync_s3_object_key(&self) -> &str {
        if self.cache.sync_s3_object_key.is_empty() {
            "jshell-sync.json"
        } else {
            &self.cache.sync_s3_object_key
        }
    }

    pub fn set_sync_connection(&mut self, endpoint: String, username: String) {
        self.cache.sync_endpoint = endpoint;
        self.cache.sync_username = username;
    }

    pub fn set_sync_s3_connection(
        &mut self,
        endpoint: String,
        region: String,
        bucket: String,
        object_key: String,
    ) {
        self.cache.sync_s3_endpoint = endpoint;
        self.cache.sync_s3_region = region;
        self.cache.sync_s3_bucket = bucket;
        self.cache.sync_s3_object_key = object_key;
    }

    pub fn set_sync_etag(&mut self, etag: Option<String>) {
        self.cache.sync_etag = etag;
        self.cache.sync_etag_backend = self.sync_backend().to_string();
    }

    pub fn tmp_dir(&self) -> Option<PathBuf> {
        self.path.parent().map(|p| p.join("tmp"))
    }

    pub fn follow_system_theme(&self) -> bool {
        self.cache.follow_system_theme
    }

    pub fn theme_mode(&self) -> &str {
        &self.cache.theme_mode
    }

    pub fn light_theme_name(&self) -> &str {
        &self.cache.light_theme_name
    }

    pub fn dark_theme_name(&self) -> &str {
        &self.cache.dark_theme_name
    }

    pub fn locale(&self) -> &str {
        if self.cache.locale.is_empty() {
            "system"
        } else {
            &self.cache.locale
        }
    }

    pub fn set_locale(&mut self, locale: &str) {
        self.cache.locale = locale.to_string();
    }

    pub fn key_bindings(&self) -> &std::collections::HashMap<String, String> {
        &self.cache.key_bindings
    }

    pub fn set_key_binding(&mut self, action_name: &str, keystroke: &str) {
        self.cache
            .key_bindings
            .insert(action_name.to_string(), keystroke.to_string());
    }

    pub fn monitoring_position(&self) -> &str {
        if self.cache.monitoring_position.is_empty() {
            "Sidebar"
        } else {
            &self.cache.monitoring_position
        }
    }

    pub fn set_monitoring_position(&mut self, pos: &str) {
        self.cache.monitoring_position = pos.to_string();
    }

    pub fn terminal_font_size(&self) -> f32 {
        if self.cache.terminal_font_size <= 0.0 {
            default_terminal_font_size()
        } else {
            self.cache.terminal_font_size
        }
    }

    pub fn set_theme_preferences(
        &mut self,
        follow_system_theme: bool,
        theme_mode: impl Into<String>,
        light_theme_name: impl Into<String>,
        dark_theme_name: impl Into<String>,
    ) {
        self.cache.follow_system_theme = follow_system_theme;
        self.cache.theme_mode = theme_mode.into();
        self.cache.light_theme_name = light_theme_name.into();
        self.cache.dark_theme_name = dark_theme_name.into();
    }

    pub fn window_bounds(&self) -> Option<&SavedWindowBounds> {
        self.cache.window_bounds.as_ref()
    }

    pub fn workspace_panels(&self) -> Option<&Vec<f32>> {
        self.cache.workspace_panels.as_ref()
    }

    #[allow(dead_code)]
    pub fn body_panels(&self) -> Option<&Vec<f32>> {
        self.cache.body_panels.as_ref()
    }

    pub fn transfers(&self) -> Vec<crate::terminal::Transfer> {
        self.cache.transfers.clone()
    }

    pub fn set_transfers(&mut self, transfers: Vec<crate::terminal::Transfer>) {
        self.cache.transfers = transfers;
        if let Err(err) = self.save() {
            tracing::error!("failed to save config: {err:#}");
        }
    }

    pub fn set_layout_state(
        &mut self,
        window_bounds: Option<SavedWindowBounds>,
        workspace_panels: Option<Vec<f32>>,
        body_panels: Option<Vec<f32>>,
    ) {
        self.cache.window_bounds = window_bounds;
        self.cache.workspace_panels = workspace_panels;
        self.cache.body_panels = body_panels;
    }

    pub fn set_terminal_font_size(&mut self, terminal_font_size: f32) {
        self.cache.terminal_font_size = terminal_font_size.max(10.0);
    }

    pub fn ui_font_size(&self) -> f32 {
        clamp_ui_font_size(self.cache.ui_font_size)
    }

    pub fn set_ui_font_size(&mut self, ui_font_size: f32) {
        self.cache.ui_font_size = clamp_ui_font_size(ui_font_size);
    }

    pub fn ui_font_family(&self) -> &str {
        if self.cache.ui_font_family.is_empty() {
            "Noto Sans CJK SC"
        } else {
            &self.cache.ui_font_family
        }
    }

    pub fn set_ui_font_family(&mut self, family: &str) {
        self.cache.ui_font_family = family.to_string();
    }

    pub fn right_click_copy_paste(&self) -> bool {
        self.cache.right_click_copy_paste
    }

    pub fn set_right_click_copy_paste(&mut self, val: bool) {
        self.cache.right_click_copy_paste = val;
    }

    pub fn keyword_highlight(&self) -> bool {
        self.cache.keyword_highlight
    }

    pub fn set_keyword_highlight(&mut self, val: bool) {
        self.cache.keyword_highlight = val;
    }

    pub fn history_completion_plugin_enabled(&self) -> bool {
        self.cache.history_completion_plugin_enabled
    }

    pub fn set_history_completion_plugin_enabled(&mut self, val: bool) {
        self.cache.history_completion_plugin_enabled = val;
    }

    pub fn terminal_font_family(&self) -> &str {
        if self.cache.terminal_font_family.is_empty() {
            SYSTEM_MONOSPACE_FONT
        } else {
            &self.cache.terminal_font_family
        }
    }

    pub fn set_terminal_font_family(&mut self, family: &str) {
        self.cache.terminal_font_family = family.to_string();
    }

    pub fn cursor_style(&self) -> CursorStyle {
        self.cache.cursor_style
    }

    pub fn set_cursor_style(&mut self, style: CursorStyle) {
        self.cache.cursor_style = style;
    }

    pub fn use_proxy(&self) -> bool {
        self.cache.use_proxy
    }
    pub fn set_use_proxy(&mut self, val: bool) {
        self.cache.use_proxy = val;
    }
    pub fn read_env_proxy(&self) -> bool {
        self.cache.read_env_proxy
    }
    pub fn set_read_env_proxy(&mut self, val: bool) {
        self.cache.read_env_proxy = val;
    }
    pub fn global_proxy_type(&self) -> &str {
        &self.cache.global_proxy_type
    }
    pub fn set_global_proxy_type(&mut self, val: String) {
        self.cache.global_proxy_type = val;
    }
    pub fn global_proxy_host(&self) -> &str {
        &self.cache.global_proxy_host
    }
    pub fn set_global_proxy_host(&mut self, val: String) {
        self.cache.global_proxy_host = val;
    }
    pub fn global_proxy_port(&self) -> Option<u16> {
        self.cache.global_proxy_port
    }
    pub fn set_global_proxy_port(&mut self, val: Option<u16>) {
        self.cache.global_proxy_port = val;
    }
    pub fn global_proxy_user(&self) -> &str {
        &self.cache.global_proxy_user
    }
    pub fn set_global_proxy_user(&mut self, val: String) {
        self.cache.global_proxy_user = val;
    }
    pub fn global_proxy_password(&self) -> &str {
        &self.cache.global_proxy_password
    }
    pub fn set_global_proxy_password(&mut self, val: String) {
        self.cache.global_proxy_password = val;
    }

    pub fn show_hidden_files(&self) -> bool {
        self.cache.show_hidden_files
    }

    pub fn set_show_hidden_files(&mut self, val: bool) {
        self.cache.show_hidden_files = val;
    }

    pub fn lock_layout(&self) -> bool {
        self.cache.lock_layout
    }

    pub fn set_lock_layout(&mut self, val: bool) {
        self.cache.lock_layout = val;
    }

    pub fn sidebar_collapsed(&self) -> bool {
        self.cache.sidebar_collapsed
    }

    pub fn set_sidebar_collapsed(&mut self, val: bool) {
        self.cache.sidebar_collapsed = val;
    }

    pub fn sftp_panel_minimized(&self) -> bool {
        self.cache.sftp_panel_minimized
    }

    pub fn set_sftp_panel_minimized(&mut self, val: bool) {
        self.cache.sftp_panel_minimized = val;
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.cache.sessions.iter().find(|s| s.id == id)
    }

    pub fn upsert(&mut self, session: Session) {
        if let Some(existing) = self.cache.sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = session;
        } else {
            self.cache.sessions.push(session);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.cache.sessions.retain(|s| s.id != id);
        for folder in &mut self.cache.session_folders {
            folder.session_ids.retain(|session_id| session_id != id);
        }
    }

    pub fn upsert_session_folder(&mut self, mut folder: SessionFolder) {
        folder.session_ids = self.normalize_folder_session_ids(folder.session_ids);
        if let Some(existing) = self
            .cache
            .session_folders
            .iter_mut()
            .find(|existing| existing.id == folder.id)
        {
            *existing = folder;
        } else {
            self.cache.session_folders.push(folder);
        }
    }

    pub fn remove_session_folder(&mut self, id: &str) {
        self.cache.session_folders.retain(|folder| folder.id != id);
    }

    pub fn add_session_to_folder(
        &mut self,
        session_id: &str,
        folder_id: &str,
        move_from_other_folders: bool,
    ) -> bool {
        if !self
            .cache
            .sessions
            .iter()
            .any(|session| session.id == session_id)
        {
            return false;
        }
        let Some(destination_index) = self
            .cache
            .session_folders
            .iter()
            .position(|folder| folder.id == folder_id)
        else {
            return false;
        };

        if move_from_other_folders {
            for folder in &mut self.cache.session_folders {
                folder.session_ids.retain(|member| member != session_id);
            }
        }
        let destination = &mut self.cache.session_folders[destination_index];
        if !destination
            .session_ids
            .iter()
            .any(|member| member == session_id)
        {
            destination.session_ids.push(session_id.to_string());
        }
        true
    }

    fn normalize_folder_session_ids(&self, session_ids: Vec<String>) -> Vec<String> {
        let known_ids: std::collections::HashSet<&str> = self
            .cache
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect();
        let mut seen = std::collections::HashSet::new();
        session_ids
            .into_iter()
            .filter(|session_id| known_ids.contains(session_id.as_str()))
            .filter(|session_id| seen.insert(session_id.clone()))
            .collect()
    }

    pub fn save(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let hardware_uuid = get_hardware_uuid();
        let encrypted_bytes = encrypt_config(&self.cache, &hardware_uuid)?;
        fs::write(&self.path, encrypted_bytes)
            .with_context(|| format!("failed to write {}", self.path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = fs::metadata(&self.path).map(|m| m.permissions()) {
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&self.path, perms);
            }
        }

        Ok(())
    }

    pub fn save_merged_preferences(&self, local_config: ConfigFile) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let hardware_uuid = get_hardware_uuid();

        let mut disk_config = if self.path.exists() {
            if let Ok(raw_bytes) = fs::read(&self.path) {
                match decrypt_config(&raw_bytes, &hardware_uuid) {
                    Ok(loaded) => loaded,
                    Err(_) => serde_json::from_slice::<ConfigFile>(&raw_bytes)
                        .unwrap_or_else(|_| self.cache.clone()),
                }
            } else {
                self.cache.clone()
            }
        } else {
            self.cache.clone()
        };

        // Merge UI preference fields
        disk_config.font_defaults_version = local_config.font_defaults_version;
        disk_config.follow_system_theme = local_config.follow_system_theme;
        disk_config.theme_mode = local_config.theme_mode;
        disk_config.light_theme_name = local_config.light_theme_name;
        disk_config.dark_theme_name = local_config.dark_theme_name;
        disk_config.locale = local_config.locale;
        disk_config.terminal_font_size = local_config.terminal_font_size;
        disk_config.ui_font_size = local_config.ui_font_size;
        disk_config.right_click_copy_paste = local_config.right_click_copy_paste;
        disk_config.keyword_highlight = local_config.keyword_highlight;
        disk_config.history_completion_plugin_enabled =
            local_config.history_completion_plugin_enabled;
        disk_config.ui_font_family = local_config.ui_font_family;
        disk_config.terminal_font_family = local_config.terminal_font_family;
        disk_config.cursor_style = local_config.cursor_style;
        disk_config.window_bounds = local_config.window_bounds;
        disk_config.workspace_panels = local_config.workspace_panels;
        disk_config.body_panels = local_config.body_panels;
        disk_config.show_hidden_files = local_config.show_hidden_files;
        disk_config.lock_layout = local_config.lock_layout;
        disk_config.monitoring_position = local_config.monitoring_position;
        disk_config.sidebar_collapsed = local_config.sidebar_collapsed;
        disk_config.sftp_panel_minimized = local_config.sftp_panel_minimized;

        let encrypted_bytes = encrypt_config(&disk_config, &hardware_uuid)?;
        fs::write(&self.path, encrypted_bytes)
            .with_context(|| format!("failed to write {}", self.path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = fs::metadata(&self.path).map(|m| m.permissions()) {
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&self.path, perms);
            }
        }

        Ok(())
    }
}

pub trait ProxyStream:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static> ProxyStream
    for T
{
}

#[derive(Debug, Clone)]
pub struct EnvProxy {
    pub proxy_type: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    pub pass: String,
}

pub static ENV_PROXY: OnceLock<Option<EnvProxy>> = OnceLock::new();

pub async fn connect_proxy(session: &Session) -> Result<Box<dyn ProxyStream>> {
    let target_host = session.host.clone();
    let target_port = session.port;
    let session = session.clone();

    let connect_fut = async move {
        let target_host = &target_host;
        let config = ConfigStore::load().unwrap_or_else(|_| ConfigStore::in_memory());
        let (proxy_type, proxy_host, proxy_port, proxy_user, proxy_password) = {
            if !session.proxy_type.is_empty() && session.proxy_type != "none" {
                (
                    session.proxy_type.clone(),
                    session.proxy_host.clone(),
                    session.proxy_port,
                    session.proxy_user.clone(),
                    session.proxy_password.clone(),
                )
            } else if config.cache.read_env_proxy
                && ENV_PROXY.get().and_then(|opt| opt.as_ref()).is_some()
            {
                let env_p = ENV_PROXY.get().and_then(|opt| opt.as_ref()).unwrap();
                (
                    env_p.proxy_type.clone(),
                    env_p.host.clone(),
                    env_p.port,
                    env_p.user.clone(),
                    env_p.pass.clone(),
                )
            } else if config.cache.use_proxy {
                (
                    config.cache.global_proxy_type.clone(),
                    config.cache.global_proxy_host.clone(),
                    config.cache.global_proxy_port,
                    config.cache.global_proxy_user.clone(),
                    config.cache.global_proxy_password.clone(),
                )
            } else {
                (
                    "none".to_string(),
                    String::new(),
                    None,
                    String::new(),
                    String::new(),
                )
            }
        };

        if proxy_type != "none" && (proxy_host.is_empty() || proxy_port.is_none()) {
            let addr = format!("{}:{}", target_host, target_port);
            let stream = tokio::net::TcpStream::connect(&addr).await?;
            return Ok(Box::new(stream) as Box<dyn ProxyStream>);
        }

        match proxy_type.as_str() {
            "socks5" | "socks5h" => {
                let proxy_port = proxy_port.unwrap_or(1080);
                let proxy_addr = format!("{}:{}", proxy_host, proxy_port);

                if !proxy_user.is_empty() {
                    let stream = tokio_socks::tcp::Socks5Stream::connect_with_password(
                        proxy_addr.as_str(),
                        (target_host.as_str(), target_port),
                        &proxy_user,
                        &proxy_password,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("SOCKS5 proxy connection failed: {}", e))?;
                    Ok(Box::new(stream) as Box<dyn ProxyStream>)
                } else {
                    let stream = tokio_socks::tcp::Socks5Stream::connect(
                        proxy_addr.as_str(),
                        (target_host.as_str(), target_port),
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("SOCKS5 proxy connection failed: {}", e))?;
                    Ok(Box::new(stream) as Box<dyn ProxyStream>)
                }
            }
            "http" => {
                let proxy_port = proxy_port.unwrap_or(8080);
                let proxy_addr = format!("{}:{}", proxy_host, proxy_port);

                use tokio::io::AsyncWriteExt;
                let mut stream = tokio::net::TcpStream::connect(&proxy_addr)
                    .await
                    .map_err(|e| anyhow::anyhow!("HTTP proxy connection failed: {}", e))?;

                let mut request = format!(
                    "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n",
                    target_host, target_port, target_host, target_port
                );
                if !proxy_user.is_empty() {
                    use base64::Engine as _;
                    let auth = format!("{}:{}", proxy_user, proxy_password);
                    let encoded = base64::engine::general_purpose::STANDARD.encode(auth);
                    request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", encoded));
                }
                request.push_str("\r\n");

                stream.write_all(request.as_bytes()).await?;

                let mut response = [0u8; 1024];
                let n = tokio::io::AsyncReadExt::read(&mut stream, &mut response).await?;
                let resp_str = String::from_utf8_lossy(&response[..n]);
                if !resp_str.contains("200") && !resp_str.contains("established") {
                    return Err(anyhow::anyhow!("HTTP proxy CONNECT failed: {}", resp_str));
                }

                Ok(Box::new(stream) as Box<dyn ProxyStream>)
            }
            _ => {
                let addr = format!("{}:{}", target_host, target_port);
                let stream = tokio::net::TcpStream::connect(&addr).await?;
                Ok(Box::new(stream) as Box<dyn ProxyStream>)
            }
        }
    };

    tokio::time::timeout(std::time::Duration::from_secs(16), connect_fut)
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out after 16 seconds"))?
}

pub fn active_proxy(session: &Session) -> Option<(String, String, Option<u16>)> {
    let config = ConfigStore::load().unwrap_or_else(|_| ConfigStore::in_memory());
    let (proxy_type, proxy_host, proxy_port, _, _) = {
        if !session.proxy_type.is_empty() && session.proxy_type != "none" {
            (
                session.proxy_type.clone(),
                session.proxy_host.clone(),
                session.proxy_port,
                session.proxy_user.clone(),
                session.proxy_password.clone(),
            )
        } else if config.cache.read_env_proxy
            && ENV_PROXY.get().and_then(|opt| opt.as_ref()).is_some()
        {
            let env_p = ENV_PROXY.get().and_then(|opt| opt.as_ref()).unwrap();
            (
                env_p.proxy_type.clone(),
                env_p.host.clone(),
                env_p.port,
                env_p.user.clone(),
                env_p.pass.clone(),
            )
        } else if config.cache.use_proxy {
            (
                config.cache.global_proxy_type.clone(),
                config.cache.global_proxy_host.clone(),
                config.cache.global_proxy_port,
                config.cache.global_proxy_user.clone(),
                config.cache.global_proxy_password.clone(),
            )
        } else {
            (
                "none".to_string(),
                String::new(),
                None,
                String::new(),
                String::new(),
            )
        }
    };

    if proxy_type != "none" && !proxy_host.is_empty() && proxy_port.is_some() {
        Some((proxy_type, proxy_host, proxy_port))
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedConfigEnvelope {
    format_version: u32,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    payload: String,
}

static HARDWARE_UUID_CACHE: OnceLock<String> = OnceLock::new();

pub fn get_hardware_uuid() -> String {
    HARDWARE_UUID_CACHE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                if let Ok(output) = std::process::Command::new("ioreg")
                    .args(&["-rd1", "-c", "IOPlatformExpertDevice"])
                    .output()
                {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        if line.contains("IOPlatformUUID") {
                            if let Some(uuid) = line.split('"').nth(3) {
                                let uuid = uuid.trim().to_string();
                                if !uuid.is_empty() {
                                    return uuid;
                                }
                            }
                        }
                    }
                }
            }

            #[cfg(target_os = "linux")]
            {
                if let Ok(uuid) = std::fs::read_to_string("/sys/class/dmi/id/product_uuid") {
                    let uuid = uuid.trim().to_string();
                    if !uuid.is_empty() {
                        return uuid;
                    }
                }
                if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
                    let id = id.trim().to_string();
                    if !id.is_empty() {
                        return id;
                    }
                }
                if let Ok(id) = std::fs::read_to_string("/var/lib/dbus/machine-id") {
                    let id = id.trim().to_string();
                    if !id.is_empty() {
                        return id;
                    }
                }
            }

            #[cfg(target_os = "windows")]
            {
                use winreg::RegKey;
                use winreg::enums::HKEY_LOCAL_MACHINE;
                let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
                if let Ok(subkey) = hklm.open_subkey("SOFTWARE\\Microsoft\\Cryptography") {
                    if let Ok(guid) = subkey.get_value::<String, _>("MachineGuid") {
                        let guid = guid.trim().to_string();
                        if !guid.is_empty() {
                            return guid;
                        }
                    }
                }
            }

            "ashell-default-hardware-uuid-fallback".to_string()
        })
        .clone()
}

fn encrypt_config(config: &ConfigFile, password: &str) -> Result<Vec<u8>> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|err| anyhow::anyhow!("derive encryption key: {err}"))?;

    let plaintext = serde_json::to_vec(config).context("serialize config")?;
    let ciphertext = XChaCha20Poly1305::new((&key).into())
        .encrypt(XNonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| anyhow::anyhow!("encrypt config payload"))?;

    serde_json::to_vec_pretty(&EncryptedConfigEnvelope {
        format_version: 1,
        kdf: "argon2id".to_string(),
        cipher: "xchacha20poly1305".to_string(),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        payload: STANDARD.encode(ciphertext),
    })
    .context("serialize encrypted config envelope")
}

fn decrypt_config(raw: &[u8], password: &str) -> Result<ConfigFile> {
    let envelope: EncryptedConfigEnvelope =
        serde_json::from_slice(raw).context("parse encrypted config envelope")?;
    if envelope.format_version != 1
        || envelope.kdf != "argon2id"
        || envelope.cipher != "xchacha20poly1305"
    {
        return Err(anyhow::anyhow!("unsupported encrypted config format"));
    }
    let salt = STANDARD
        .decode(envelope.salt)
        .context("decode config salt")?;
    let nonce = STANDARD
        .decode(envelope.nonce)
        .context("decode config nonce")?;
    if nonce.len() != 24 {
        return Err(anyhow::anyhow!("invalid config nonce"));
    }
    let ciphertext = STANDARD
        .decode(envelope.payload)
        .context("decode encrypted config payload")?;

    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|err| anyhow::anyhow!("derive encryption key: {err}"))?;

    let plaintext = XChaCha20Poly1305::new((&key).into())
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| {
            anyhow::anyhow!("cannot decrypt config; hardware UUID mismatch or corrupted data")
        })?;

    serde_json::from_slice(&plaintext).context("parse decrypted config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_terminal_font_follows_the_system_monospace_font() {
        let config = ConfigFile::default();
        assert_eq!(config.ui_font_family, "Noto Sans CJK SC");
        assert_eq!(config.terminal_font_family, ".SystemMonospace");
        assert_eq!(config.terminal_font_size, 16.0);
        assert_eq!(config.font_defaults_version, 4);
    }

    #[test]
    fn history_completion_plugin_is_enabled_by_default_and_can_be_disabled() {
        let mut store = ConfigStore::in_memory();
        assert!(store.history_completion_plugin_enabled());

        store.set_history_completion_plugin_enabled(false);
        assert!(!store.history_completion_plugin_enabled());

        let legacy_config: ConfigFile = serde_json::from_str("{}").unwrap();
        assert!(legacy_config.history_completion_plugin_enabled);
    }

    #[test]
    fn legacy_font_preferences_migrate_to_system_monospace() {
        let mut config = ConfigFile {
            ui_font_family: "Noto Sans Mono CJK SC".to_string(),
            terminal_font_family: "Noto Sans CJK SC".to_string(),
            font_defaults_version: 0,
            ..ConfigFile::default()
        };

        assert!(migrate_legacy_font_preferences(&mut config));
        assert_eq!(config.ui_font_family, "Noto Sans CJK SC");
        assert_eq!(config.terminal_font_family, ".SystemMonospace");
        assert_eq!(config.terminal_font_size, 16.0);
        assert_eq!(config.font_defaults_version, 4);

        config.ui_font_family = "Noto Sans Mono CJK SC".to_string();
        assert!(!migrate_legacy_font_preferences(&mut config));
        assert_eq!(config.ui_font_family, "Noto Sans Mono CJK SC");
    }

    #[test]
    fn system_font_migration_preserves_an_explicit_custom_terminal_font() {
        let mut config = ConfigFile {
            terminal_font_family: "JetBrains Mono".to_string(),
            font_defaults_version: 3,
            ..ConfigFile::default()
        };

        assert!(migrate_legacy_font_preferences(&mut config));
        assert_eq!(config.terminal_font_family, "JetBrains Mono");
        assert_eq!(config.font_defaults_version, 4);
    }

    #[test]
    fn balanced_font_migration_preserves_a_custom_terminal_size() {
        let mut config = ConfigFile {
            terminal_font_size: 16.0,
            font_defaults_version: 1,
            ..ConfigFile::default()
        };

        assert!(migrate_legacy_font_preferences(&mut config));
        assert_eq!(config.terminal_font_size, 16.0);
        assert_eq!(config.font_defaults_version, 4);
    }

    #[test]
    fn balanced_font_migration_upgrades_the_compact_default() {
        let mut config = ConfigFile {
            terminal_font_size: 15.0,
            font_defaults_version: 2,
            ..ConfigFile::default()
        };

        assert!(migrate_legacy_font_preferences(&mut config));
        assert_eq!(config.terminal_font_size, 16.0);
        assert_eq!(config.font_defaults_version, 4);
    }

    #[test]
    fn configuration_does_not_persist_a_title_bar_style() {
        let config = ConfigFile::default();
        assert!(
            !serde_json::to_string(&config)
                .unwrap()
                .contains("title_bar_style")
        );
    }

    #[test]
    fn test_get_hardware_uuid() {
        let uuid = get_hardware_uuid();
        assert!(!uuid.is_empty());
    }

    #[test]
    fn jshell_config_path_uses_the_new_directory_and_retains_a_legacy_source_path() {
        let config_path = ConfigStore::config_path().unwrap();
        let legacy_path = ConfigStore::legacy_config_path().unwrap();

        assert!(
            config_path.ends_with(
                std::path::Path::new(".config")
                    .join("jshell")
                    .join("sessions.json")
            )
        );
        assert!(
            legacy_path.ends_with(
                std::path::Path::new(".config")
                    .join("ashell")
                    .join("sessions.json")
            )
        );
    }

    #[test]
    fn test_config_encryption_roundtrip() {
        let config = ConfigFile::default();
        let password = "test-password-123";
        let encrypted = encrypt_config(&config, password).unwrap();

        // Ensure it doesn't contain plain text fields of default config
        let encrypted_str = String::from_utf8_lossy(&encrypted);
        assert!(!encrypted_str.contains("Noto Sans CJK SC"));
        assert!(encrypted_str.contains("argon2id"));

        let decrypted = decrypt_config(&encrypted, password).unwrap();
        assert_eq!(decrypted.terminal_font_family, config.terminal_font_family);

        // Decrypt with wrong password should fail
        assert!(decrypt_config(&encrypted, "wrong-password").is_err());
    }

    #[test]
    fn test_save_merged_preferences() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("ashell-test-config-{}.json", Uuid::new_v4()));
        let mut store = ConfigStore {
            path: path.clone(),
            cache: ConfigFile::default(),
        };

        let session = Session {
            id: "test-session-id".to_string(),
            name: "Test Session".to_string(),
            host: "1.2.3.4".to_string(),
            port: 22,
            user: "root".to_string(),
            auth: AuthMethod::Password,
            password: "pwd".to_string(),
            private_key_path: String::new(),
            private_key_inline: String::new(),
            passphrase: String::new(),
            last_used: None,
            proxy_type: String::new(),
            proxy_host: String::new(),
            proxy_port: None,
            proxy_user: String::new(),
            proxy_password: String::new(),
            protocol: "ssh".to_string(),
            baud_rate: 115200,
        };
        store.cache.sessions.push(session.clone());
        store.save().unwrap();

        let mut local_config = ConfigFile::default();
        local_config.ui_font_size = 18.0;
        local_config.terminal_font_size = 20.0;
        local_config.show_hidden_files = true;

        store.save_merged_preferences(local_config).unwrap();

        let loaded_bytes = fs::read(&path).unwrap();
        let decrypted = decrypt_config(&loaded_bytes, &get_hardware_uuid()).unwrap();

        assert_eq!(decrypted.ui_font_size, 18.0);
        assert_eq!(decrypted.terminal_font_size, 20.0);
        assert!(decrypted.show_hidden_files);

        assert_eq!(decrypted.sessions.len(), 1);
        assert_eq!(decrypted.sessions[0].name, "Test Session");
        assert_eq!(decrypted.sessions[0].host, "1.2.3.4");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_key_binding_unbinding_none() {
        let mut store = ConfigStore::in_memory();
        store.set_key_binding("OpenSettings", "none");
        assert_eq!(store.key_bindings().get("OpenSettings").unwrap(), "none");
    }

    #[test]
    fn ui_font_size_is_clamped_when_saved() {
        let mut store = ConfigStore::in_memory();

        store.set_ui_font_size(99.0);
        assert_eq!(store.ui_font_size(), 18.0);

        store.set_ui_font_size(1.0);
        assert_eq!(store.ui_font_size(), 10.0);
    }

    #[test]
    fn ui_font_size_is_clamped_when_loaded_from_legacy_config() {
        let mut store = ConfigStore::in_memory();
        store.cache.ui_font_size = 99.0;
        assert_eq!(store.ui_font_size(), 18.0);

        store.cache.ui_font_size = 1.0;
        assert_eq!(store.ui_font_size(), 10.0);
    }

    #[test]
    fn removing_a_session_prunes_it_from_session_folders() {
        let mut store = ConfigStore::in_memory();
        let session = Session::password(
            "example.com".to_string(),
            22,
            "root".to_string(),
            "password".to_string(),
        );
        let session_id = session.id.clone();
        store.upsert(session);
        store.upsert_session_folder(SessionFolder::new(
            "Production".to_string(),
            vec![
                session_id.clone(),
                session_id.clone(),
                "missing".to_string(),
            ],
        ));

        store.remove(&session_id);

        assert!(store.sessions().is_empty());
        assert_eq!(store.session_folders().len(), 1);
        assert!(store.session_folders()[0].session_ids.is_empty());
    }

    #[test]
    fn updating_a_folder_keeps_only_existing_unique_session_members() {
        let mut store = ConfigStore::in_memory();
        let session = Session::password(
            "example.com".to_string(),
            22,
            "root".to_string(),
            "password".to_string(),
        );
        let session_id = session.id.clone();
        store.upsert(session);
        let mut folder = SessionFolder::new("Production".to_string(), Vec::new());
        let folder_id = folder.id.clone();
        folder.session_ids = vec![
            session_id.clone(),
            session_id.clone(),
            "missing".to_string(),
        ];

        store.upsert_session_folder(folder);

        assert_eq!(store.session_folders()[0].id, folder_id);
        assert_eq!(store.session_folders()[0].session_ids, vec![session_id]);
    }

    #[test]
    fn folder_sessions_follow_the_folder_member_order() {
        let mut store = ConfigStore::in_memory();
        let first = Session::password(
            "one.example.com".to_string(),
            22,
            "root".to_string(),
            "password".to_string(),
        );
        let second = Session::password(
            "two.example.com".to_string(),
            22,
            "root".to_string(),
            "password".to_string(),
        );
        let first_id = first.id.clone();
        let second_id = second.id.clone();
        store.upsert(first);
        store.upsert(second);
        let folder = SessionFolder::new("Production".to_string(), vec![second_id, first_id]);
        let folder_id = folder.id.clone();
        store.upsert_session_folder(folder);

        let names: Vec<_> = store
            .session_folder_sessions(&folder_id)
            .into_iter()
            .map(|session| session.host)
            .collect();

        assert_eq!(names, vec!["two.example.com", "one.example.com"]);
    }

    #[test]
    fn copying_and_moving_a_session_between_folders_preserves_the_expected_membership() {
        let mut store = ConfigStore::in_memory();
        let session = Session::password(
            "example.com".to_string(),
            22,
            "root".to_string(),
            "password".to_string(),
        );
        let session_id = session.id.clone();
        store.upsert(session);

        let source = SessionFolder::new("Source".to_string(), vec![session_id.clone()]);
        let source_id = source.id.clone();
        let destination = SessionFolder::new("Destination".to_string(), Vec::new());
        let destination_id = destination.id.clone();
        store.upsert_session_folder(source);
        store.upsert_session_folder(destination);

        assert!(store.add_session_to_folder(&session_id, &destination_id, false));
        assert_eq!(store.session_folder_sessions(&source_id).len(), 1);
        assert_eq!(store.session_folder_sessions(&destination_id).len(), 1);

        assert!(store.add_session_to_folder(&session_id, &destination_id, true));
        assert!(store.session_folder_sessions(&source_id).is_empty());
        assert_eq!(store.session_folder_sessions(&destination_id).len(), 1);
    }
}
