use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result, bail};
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

use super::config_key::{ConfigKeyProvider, MasterKey, PlatformKeyProvider};
use crate::sync::{DecodedSyncPayload, PortableConfigV2, SyncTargetId};

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

fn default_session_proxy_type() -> String {
    "none".to_string()
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
    #[serde(default = "default_session_proxy_type")]
    pub proxy_type: String, // "none", "direct", "socks5", "socks5h", "http", "https"
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SftpCwdSyncMode {
    Off,
    Realtime,
    #[default]
    #[serde(other)]
    OnTerminalSwitch,
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
    #[serde(default)]
    pub editor_soft_wrap: bool,
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
    pub sftp_cwd_sync_mode: SftpCwdSyncMode,
    #[serde(default)]
    pub key_bindings: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub sync_endpoint: String,
    #[serde(default)]
    pub sync_username: String,
    #[serde(default)]
    pub sync_etag: Option<String>,
    #[serde(default)]
    pub sync_etag_target: String,
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
    pub sync_r2_account_id: String,
    #[serde(default)]
    pub sync_r2_bucket: String,
    #[serde(default = "default_s3_object_key")]
    pub sync_r2_object_key: String,
    #[serde(default)]
    pub sync_r2_access_key_id: String,
    #[serde(default)]
    pub sync_remember_encryption_password: bool,
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
            editor_soft_wrap: false,
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
            sftp_cwd_sync_mode: SftpCwdSyncMode::default(),
            key_bindings: std::collections::HashMap::new(),
            sync_endpoint: String::new(),
            sync_username: String::new(),
            sync_etag: None,
            sync_etag_target: String::new(),
            sync_device_id: String::new(),
            sync_backend: String::new(),
            sync_etag_backend: String::new(),
            sync_s3_endpoint: String::new(),
            sync_s3_region: default_s3_region(),
            sync_s3_bucket: String::new(),
            sync_s3_object_key: default_s3_object_key(),
            sync_r2_account_id: String::new(),
            sync_r2_bucket: String::new(),
            sync_r2_object_key: default_s3_object_key(),
            sync_r2_access_key_id: String::new(),
            sync_remember_encryption_password: false,
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

fn atomic_write_config(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_config_with(path, bytes, |staged, target| {
        staged
            .persist(target)
            .map_err(|err| anyhow::Error::new(err.error))
    })
}

fn atomic_write_config_with(
    path: &Path,
    bytes: &[u8],
    persist: impl FnOnce(tempfile::NamedTempFile, &Path) -> Result<File>,
) -> Result<()> {
    atomic_write_config_with_post_commit(path, bytes, persist, sync_committed_config)
}

fn atomic_write_config_with_post_commit(
    path: &Path,
    bytes: &[u8],
    persist: impl FnOnce(tempfile::NamedTempFile, &Path) -> Result<File>,
    post_commit: impl FnOnce(&File, &Path) -> Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("configuration path has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create staged configuration in {}",
            parent.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| {
                format!(
                    "failed to set staged configuration permissions for {}",
                    path.display()
                )
            })?;
    }

    staged.as_file_mut().write_all(bytes).with_context(|| {
        format!(
            "failed to write staged configuration for {}",
            path.display()
        )
    })?;
    staged.as_file_mut().flush().with_context(|| {
        format!(
            "failed to flush staged configuration for {}",
            path.display()
        )
    })?;
    staged
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to sync staged configuration for {}", path.display()))?;

    let persisted = persist(staged, path)
        .with_context(|| format!("failed to atomically replace {}", path.display()))?;
    if post_commit(&persisted, parent).is_err() {
        tracing::warn!("configuration committed but post-commit durability sync failed");
    }

    Ok(())
}

fn sync_committed_config(persisted: &File, _parent: &Path) -> Result<()> {
    persisted
        .sync_all()
        .context("failed to sync persisted configuration")?;

    #[cfg(unix)]
    File::open(_parent)
        .and_then(|directory| directory.sync_all())
        .context("failed to sync configuration directory")?;

    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
pub enum SyncConnectionProvider {
    WebDav {
        endpoint: String,
        username: String,
    },
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        object_key: String,
    },
    R2 {
        account_id: String,
        bucket: String,
        object_key: String,
        access_key_id: String,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncConnectionSnapshot {
    provider: SyncConnectionProvider,
    remember_encryption_password: bool,
}

impl SyncConnectionSnapshot {
    pub fn webdav(endpoint: String, username: String, remember_encryption_password: bool) -> Self {
        Self {
            provider: SyncConnectionProvider::WebDav { endpoint, username },
            remember_encryption_password,
        }
    }

    pub fn s3(
        endpoint: String,
        region: String,
        bucket: String,
        object_key: String,
        remember_encryption_password: bool,
    ) -> Self {
        Self {
            provider: SyncConnectionProvider::S3 {
                endpoint,
                region,
                bucket,
                object_key,
            },
            remember_encryption_password,
        }
    }

    pub fn r2(
        account_id: String,
        bucket: String,
        object_key: String,
        access_key_id: String,
        remember_encryption_password: bool,
    ) -> Self {
        Self {
            provider: SyncConnectionProvider::R2 {
                account_id,
                bucket,
                object_key,
                access_key_id,
            },
            remember_encryption_password,
        }
    }

    pub fn remember_encryption_password(&self) -> bool {
        self.remember_encryption_password
    }

    #[cfg(test)]
    pub fn with_remember_encryption_password(&self, remember: bool) -> Self {
        let mut connection = self.clone();
        connection.remember_encryption_password = remember;
        connection
    }

    pub fn matches_remote(&self, other: &Self) -> bool {
        self.provider == other.provider
    }

    pub fn target_id(&self) -> SyncTargetId {
        match &self.provider {
            SyncConnectionProvider::WebDav { endpoint, username } => {
                SyncTargetId::for_webdav(endpoint, username)
            }
            SyncConnectionProvider::S3 {
                endpoint,
                region,
                bucket,
                object_key,
            } => SyncTargetId::for_s3(endpoint, region, bucket, object_key),
            SyncConnectionProvider::R2 {
                account_id,
                bucket,
                object_key,
                ..
            } => SyncTargetId::for_r2(account_id, bucket, object_key),
        }
    }

    fn apply_to(&self, candidate: &mut ConfigFile) {
        match &self.provider {
            SyncConnectionProvider::WebDav { endpoint, username } => {
                candidate.sync_backend = "webdav".to_string();
                candidate.sync_endpoint.clone_from(endpoint);
                candidate.sync_username.clone_from(username);
            }
            SyncConnectionProvider::S3 {
                endpoint,
                region,
                bucket,
                object_key,
            } => {
                candidate.sync_backend = "s3".to_string();
                candidate.sync_s3_endpoint.clone_from(endpoint);
                candidate.sync_s3_region.clone_from(region);
                candidate.sync_s3_bucket.clone_from(bucket);
                candidate.sync_s3_object_key.clone_from(object_key);
            }
            SyncConnectionProvider::R2 {
                account_id,
                bucket,
                object_key,
                access_key_id,
            } => {
                candidate.sync_backend = "r2".to_string();
                candidate.sync_r2_account_id.clone_from(account_id);
                candidate.sync_r2_bucket.clone_from(bucket);
                candidate.sync_r2_object_key.clone_from(object_key);
                candidate.sync_r2_access_key_id.clone_from(access_key_id);
            }
        }
        candidate.sync_remember_encryption_password = self.remember_encryption_password;
    }
}

#[derive(Clone)]
pub struct ConfigStore {
    pub(crate) path: PathBuf,
    pub(crate) cache: ConfigFile,
    master_key: MasterKey,
}

impl ConfigStore {
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let legacy_path = Self::legacy_config_path()?;
        Self::load_with_key_provider(
            path,
            legacy_path,
            &PlatformKeyProvider,
            &get_hardware_uuid(),
        )
    }

    fn load_with_key_provider(
        path: PathBuf,
        legacy_path: PathBuf,
        key_provider: &dyn ConfigKeyProvider,
        legacy_hardware_id: &str,
    ) -> Result<Self> {
        Self::load_with_key_provider_and_persist(
            path,
            legacy_path,
            key_provider,
            legacy_hardware_id,
            Self::save,
        )
    }

    fn load_with_key_provider_and_persist<F>(
        path: PathBuf,
        legacy_path: PathBuf,
        key_provider: &dyn ConfigKeyProvider,
        legacy_hardware_id: &str,
        persist_migration: F,
    ) -> Result<Self>
    where
        F: Fn(&Self) -> Result<()>,
    {
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
            legacy_path.clone()
        } else {
            path.clone()
        };
        let migrated_config_location = source_path != path;

        let (mut cache, master_key, migrated_encryption) = if source_path.exists() {
            let raw_bytes = fs::read(&source_path)
                .with_context(|| format!("failed to read {}", source_path.display()))?;
            match config_format_version(&raw_bytes) {
                Ok(2) => {
                    let key = key_provider
                        .load_existing()
                        .context("load existing version 2 configuration master key")?;
                    let cache = decrypt_config_v2(&raw_bytes, &key)
                        .with_context(|| format!("failed to decrypt {}", source_path.display()))?;
                    (cache, key, false)
                }
                Ok(1) => {
                    let cache =
                        decrypt_config_v1(&raw_bytes, legacy_hardware_id).with_context(|| {
                            format!("failed to decrypt legacy config {}", source_path.display())
                        })?;
                    let key = key_provider
                        .load_or_create()
                        .context("initialize configuration master key for version 1 migration")?;
                    (cache, key, true)
                }
                Ok(version) => {
                    return Err(anyhow::anyhow!(
                        "unsupported encrypted config version {version} in {}",
                        source_path.display()
                    ));
                }
                Err(header_error) => {
                    let cache = parse_legacy_plaintext_config(
                        &raw_bytes,
                        source_path == legacy_path,
                    )
                    .with_context(|| {
                        format!(
                            "failed to recognize config {} (encrypted header error: {header_error:#})",
                            source_path.display()
                        )
                    })?;
                    let key = key_provider
                        .load_or_create()
                        .context("initialize configuration master key for plaintext migration")?;
                    (cache, key, true)
                }
            }
        } else {
            (
                ConfigFile::default(),
                key_provider
                    .load_or_create()
                    .context("initialize configuration master key")?,
                false,
            )
        };

        if cache.sync_device_id.is_empty() {
            cache.sync_device_id = Uuid::new_v4().to_string();
        }
        let migrated_font_preferences = migrate_legacy_font_preferences(&mut cache);
        let store = Self {
            path,
            cache,
            master_key,
        };
        if migrated_encryption || migrated_config_location {
            persist_migration(&store)
                .context("persist security-sensitive configuration migration")?;
            if migrated_config_location {
                fs::remove_file(&source_path).with_context(|| {
                    format!(
                        "remove migrated legacy configuration {}",
                        source_path.display()
                    )
                })?;
            }
        } else if migrated_font_preferences && let Err(err) = persist_migration(&store) {
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
            master_key: MasterKey::random(),
        }
    }

    pub fn is_persistent(&self) -> bool {
        !self.path.as_os_str().is_empty()
    }

    pub fn default_tmp_dir() -> Result<PathBuf> {
        let config_path = Self::config_path()?;
        let config_dir = config_path
            .parent()
            .context("configuration path has no parent directory")?;
        Ok(config_dir.join("tmp"))
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

    pub fn portable_config(&self) -> PortableConfigV2 {
        PortableConfigV2::from(&self.cache)
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

    #[cfg(test)]
    pub fn replace_sessions(&mut self, sessions: Vec<Session>) {
        self.cache.sessions = sessions;
    }

    pub fn sync_endpoint(&self) -> &str {
        &self.cache.sync_endpoint
    }

    pub fn sync_username(&self) -> &str {
        &self.cache.sync_username
    }

    #[cfg(test)]
    pub fn sync_etag(&self) -> Option<&str> {
        let target = self.current_sync_target()?;
        self.sync_etag_for_target(&target)
    }

    pub fn sync_etag_for_target(&self, target: &SyncTargetId) -> Option<&str> {
        (self.cache.sync_etag_target == target.as_str())
            .then_some(self.cache.sync_etag.as_deref())
            .flatten()
    }

    pub fn sync_backend(&self) -> &str {
        match self.cache.sync_backend.as_str() {
            "s3" => "s3",
            "r2" => "r2",
            _ => "webdav",
        }
    }

    #[cfg(test)]
    pub fn set_sync_backend(&mut self, backend: &str) {
        self.cache.sync_backend = match backend {
            "s3" => "s3",
            "r2" => "r2",
            _ => "webdav",
        }
        .to_string();
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

    pub fn sync_r2_account_id(&self) -> &str {
        &self.cache.sync_r2_account_id
    }

    pub fn sync_r2_bucket(&self) -> &str {
        &self.cache.sync_r2_bucket
    }

    pub fn sync_r2_object_key(&self) -> &str {
        if self.cache.sync_r2_object_key.is_empty() {
            "jshell-sync.json"
        } else {
            &self.cache.sync_r2_object_key
        }
    }

    pub fn sync_r2_access_key_id(&self) -> &str {
        &self.cache.sync_r2_access_key_id
    }

    pub fn sync_remember_encryption_password(&self) -> bool {
        self.cache.sync_remember_encryption_password
    }

    pub fn set_sync_remember_encryption_password(&mut self, remember: bool) {
        self.cache.sync_remember_encryption_password = remember;
    }

    pub fn sync_target_id(&self) -> Option<SyncTargetId> {
        self.current_sync_target()
    }

    #[cfg(test)]
    pub fn set_sync_connection(&mut self, endpoint: String, username: String) {
        self.cache.sync_endpoint = endpoint;
        self.cache.sync_username = username;
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub fn set_sync_etag(&mut self, etag: Option<String>) {
        if let Some(target) = self.current_sync_target() {
            self.cache.sync_etag = etag;
            self.cache.sync_etag_target = target.to_string();
        } else {
            self.cache.sync_etag = None;
            self.cache.sync_etag_target.clear();
        }
    }

    pub fn persist_sync_connection(
        &mut self,
        connection: &SyncConnectionSnapshot,
        etag: Option<String>,
    ) -> Result<()> {
        self.persist_sync_connection_with_persist(connection, etag, atomic_write_config)
    }

    fn persist_sync_connection_with_persist<F>(
        &mut self,
        connection: &SyncConnectionSnapshot,
        etag: Option<String>,
        persist: F,
    ) -> Result<()>
    where
        F: FnOnce(&Path, &[u8]) -> Result<()>,
    {
        let mut candidate = self.cache.clone();
        connection.apply_to(&mut candidate);
        candidate.sync_etag = etag;
        candidate.sync_etag_target = connection.target_id().to_string();
        candidate.sync_etag_backend = candidate.sync_backend.clone();

        let encrypted_bytes = encrypt_config_v2(&candidate, &self.master_key)?;
        if !self.path.as_os_str().is_empty() {
            persist(&self.path, &encrypted_bytes)
                .context("persist synchronized connection candidate")?;
        }
        self.cache = candidate;
        Ok(())
    }

    fn current_sync_target(&self) -> Option<SyncTargetId> {
        match self.sync_backend() {
            "webdav" if !self.cache.sync_endpoint.trim().is_empty() => Some(
                SyncTargetId::for_webdav(&self.cache.sync_endpoint, &self.cache.sync_username),
            ),
            "s3" if !self.sync_s3_region().trim().is_empty()
                && !self.cache.sync_s3_bucket.trim().is_empty() =>
            {
                Some(SyncTargetId::for_s3(
                    &self.cache.sync_s3_endpoint,
                    self.sync_s3_region(),
                    &self.cache.sync_s3_bucket,
                    self.sync_s3_object_key(),
                ))
            }
            "r2" if !self.cache.sync_r2_account_id.trim().is_empty()
                && !self.cache.sync_r2_bucket.trim().is_empty() =>
            {
                Some(SyncTargetId::for_r2(
                    &self.cache.sync_r2_account_id,
                    &self.cache.sync_r2_bucket,
                    &self.cache.sync_r2_object_key,
                ))
            }
            _ => None,
        }
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

    pub fn body_panels(&self) -> Option<&Vec<f32>> {
        self.cache.body_panels.as_ref()
    }

    pub fn transfers(&self) -> Vec<crate::terminal::Transfer> {
        self.cache.transfers.clone()
    }

    pub fn set_transfers(&mut self, transfers: Vec<crate::terminal::Transfer>) {
        self.cache.transfers = transfers;
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

    pub fn editor_soft_wrap(&self) -> bool {
        self.cache.editor_soft_wrap
    }

    pub fn set_editor_soft_wrap(&mut self, val: bool) {
        self.cache.editor_soft_wrap = val;
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

    pub fn connection_proxy_config(&self) -> ConnectionProxyConfig {
        ConnectionProxyConfig {
            read_env_proxy: self.cache.read_env_proxy,
            use_global_proxy: self.cache.use_proxy,
            global_proxy: ProxyEndpoint {
                proxy_type: self.cache.global_proxy_type.clone(),
                host: self.cache.global_proxy_host.clone(),
                port: self.cache.global_proxy_port,
                user: self.cache.global_proxy_user.clone(),
                password: self.cache.global_proxy_password.clone(),
            },
            env_proxy: ENV_PROXY.get_or_init(read_proxy_from_env).clone(),
            allow_direct: self.is_persistent(),
        }
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

    pub fn sftp_cwd_sync_mode(&self) -> SftpCwdSyncMode {
        self.cache.sftp_cwd_sync_mode
    }

    pub fn set_sftp_cwd_sync_mode(&mut self, mode: SftpCwdSyncMode) {
        self.cache.sftp_cwd_sync_mode = mode;
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

    #[cfg(test)]
    pub fn apply_decoded_sync_payload(
        &mut self,
        payload: &DecodedSyncPayload,
        target: &SyncTargetId,
        etag: Option<String>,
    ) -> Result<()> {
        self.apply_decoded_sync_payload_with_persist(payload, target, etag, atomic_write_config)
    }

    #[cfg(test)]
    fn apply_decoded_sync_payload_with_persist<F>(
        &mut self,
        payload: &DecodedSyncPayload,
        target: &SyncTargetId,
        etag: Option<String>,
        persist: F,
    ) -> Result<()>
    where
        F: FnOnce(&Path, &[u8]) -> Result<()>,
    {
        let mut candidate = self.cache.clone();
        Self::apply_decoded_sync_payload_to_candidate(&mut candidate, payload);
        candidate.sync_etag = etag;
        candidate.sync_etag_target = target.to_string();

        self.persist_sync_candidate(candidate, persist)
    }

    pub fn apply_decoded_sync_payload_with_connection(
        &mut self,
        payload: &DecodedSyncPayload,
        connection: &SyncConnectionSnapshot,
        etag: Option<String>,
    ) -> Result<()> {
        self.apply_decoded_sync_payload_with_connection_with_persist(
            payload,
            connection,
            etag,
            atomic_write_config,
        )
    }

    fn apply_decoded_sync_payload_with_connection_with_persist<F>(
        &mut self,
        payload: &DecodedSyncPayload,
        connection: &SyncConnectionSnapshot,
        etag: Option<String>,
        persist: F,
    ) -> Result<()>
    where
        F: FnOnce(&Path, &[u8]) -> Result<()>,
    {
        let mut candidate = self.cache.clone();
        Self::apply_decoded_sync_payload_to_candidate(&mut candidate, payload);
        connection.apply_to(&mut candidate);
        candidate.sync_etag = etag;
        candidate.sync_etag_target = connection.target_id().to_string();
        candidate.sync_etag_backend = candidate.sync_backend.clone();

        self.persist_sync_candidate(candidate, persist)
    }

    fn apply_decoded_sync_payload_to_candidate(
        candidate: &mut ConfigFile,
        payload: &DecodedSyncPayload,
    ) {
        match payload {
            DecodedSyncPayload::LegacyV1(payload) => {
                candidate.sessions = payload.sessions.clone();
                Self::normalize_candidate_session_folders(candidate);
            }
            DecodedSyncPayload::V2(payload) => {
                Self::apply_portable_config(candidate, &payload.portable_config);
            }
        }
    }

    fn persist_sync_candidate<F>(&mut self, candidate: ConfigFile, persist: F) -> Result<()>
    where
        F: FnOnce(&Path, &[u8]) -> Result<()>,
    {
        let encrypted_bytes = encrypt_config_v2(&candidate, &self.master_key)?;
        if !self.path.as_os_str().is_empty() {
            persist(&self.path, &encrypted_bytes)
                .context("persist synchronized configuration candidate")?;
        }
        self.cache = candidate;
        Ok(())
    }

    fn apply_portable_config(candidate: &mut ConfigFile, portable: &PortableConfigV2) {
        candidate.sessions = portable.sessions.clone();
        candidate.session_folders = portable.session_folders.clone();

        candidate.follow_system_theme = portable.preferences.follow_system_theme;
        candidate.theme_mode = portable.preferences.theme_mode.clone();
        candidate.light_theme_name = portable.preferences.light_theme_name.clone();
        candidate.dark_theme_name = portable.preferences.dark_theme_name.clone();
        candidate.locale = portable.preferences.locale.clone();
        candidate.terminal_font_size = portable.preferences.terminal_font_size;
        candidate.ui_font_size = portable.preferences.ui_font_size;
        candidate.right_click_copy_paste = portable.preferences.right_click_copy_paste;
        candidate.keyword_highlight = portable.preferences.keyword_highlight;
        candidate.editor_soft_wrap = portable.preferences.editor_soft_wrap;
        candidate.history_completion_plugin_enabled =
            portable.preferences.history_completion_plugin_enabled;
        candidate.ui_font_family = portable.preferences.ui_font_family.clone();
        candidate.terminal_font_family = portable.preferences.terminal_font_family.clone();
        candidate.cursor_style = portable.preferences.cursor_style;
        candidate.show_hidden_files = portable.preferences.show_hidden_files;
        candidate.lock_layout = portable.preferences.lock_layout;
        candidate.monitoring_position = portable.preferences.monitoring_position.clone();

        candidate.key_bindings = portable.key_bindings.clone();

        candidate.use_proxy = portable.proxy.use_proxy;
        candidate.read_env_proxy = portable.proxy.read_env_proxy;
        candidate.global_proxy_type = portable.proxy.global_proxy_type.clone();
        candidate.global_proxy_host = portable.proxy.global_proxy_host.clone();
        candidate.global_proxy_port = portable.proxy.global_proxy_port;
        candidate.global_proxy_user = portable.proxy.global_proxy_user.clone();
        candidate.global_proxy_password = portable.proxy.global_proxy_password.clone();
    }

    fn normalize_candidate_session_folders(candidate: &mut ConfigFile) {
        let known_ids: std::collections::HashSet<String> = candidate
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect();
        for folder in &mut candidate.session_folders {
            let mut seen = std::collections::HashSet::new();
            folder.session_ids.retain(|session_id| {
                known_ids.contains(session_id) && seen.insert(session_id.clone())
            });
        }
    }

    pub fn save(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let encrypted_bytes = encrypt_config_v2(&self.cache, &self.master_key)?;
        atomic_write_config(&self.path, &encrypted_bytes)
    }

    pub fn save_merged_preferences(&self, local_config: ConfigFile) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let mut disk_config = if self.path.exists() {
            let raw_bytes = fs::read(&self.path)
                .with_context(|| format!("failed to read {}", self.path.display()))?;
            decrypt_config_v2(&raw_bytes, &self.master_key)
                .with_context(|| format!("failed to decrypt {}", self.path.display()))?
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
        disk_config.editor_soft_wrap = local_config.editor_soft_wrap;
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
        disk_config.sftp_cwd_sync_mode = local_config.sftp_cwd_sync_mode;
        disk_config.key_bindings = local_config.key_bindings;
        disk_config.use_proxy = local_config.use_proxy;
        disk_config.read_env_proxy = local_config.read_env_proxy;
        disk_config.global_proxy_type = local_config.global_proxy_type;
        disk_config.global_proxy_host = local_config.global_proxy_host;
        disk_config.global_proxy_port = local_config.global_proxy_port;
        disk_config.global_proxy_user = local_config.global_proxy_user;
        disk_config.global_proxy_password = local_config.global_proxy_password;
        disk_config.sync_remember_encryption_password =
            local_config.sync_remember_encryption_password;

        let encrypted_bytes = encrypt_config_v2(&disk_config, &self.master_key)?;
        atomic_write_config(&self.path, &encrypted_bytes)
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyEndpoint {
    proxy_type: String,
    host: String,
    port: Option<u16>,
    user: String,
    password: String,
}

#[derive(Debug, Clone)]
pub struct ConnectionProxyConfig {
    read_env_proxy: bool,
    use_global_proxy: bool,
    global_proxy: ProxyEndpoint,
    env_proxy: std::result::Result<Option<ProxyEndpoint>, String>,
    allow_direct: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyKind {
    Socks5,
    Http,
    Https,
}

impl ProxyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Socks5 => "socks5",
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedProxy {
    kind: ProxyKind,
    host: String,
    port: u16,
    user: String,
    password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProxyRoute {
    Direct,
    Proxy(ResolvedProxy),
}

static ENV_PROXY: OnceLock<std::result::Result<Option<ProxyEndpoint>, String>> = OnceLock::new();

pub fn initialize_env_proxy() {
    match ENV_PROXY.get_or_init(read_proxy_from_env) {
        Ok(Some(proxy)) => tracing::info!(
            "[proxy] loaded proxy from environment: type={}, host={}, port={:?}, user={}",
            proxy.proxy_type,
            proxy.host,
            proxy.port,
            proxy.user
        ),
        Ok(None) => tracing::debug!("[proxy] no proxy configured in environment"),
        Err(error) => tracing::warn!("[proxy] invalid environment proxy: {error}"),
    }
}

fn read_proxy_from_env() -> std::result::Result<Option<ProxyEndpoint>, String> {
    const PROXY_VARIABLES: &[&str] = &[
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];

    for variable in PROXY_VARIABLES {
        let Ok(value) = std::env::var(variable) else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        return parse_env_proxy(variable, &value).map(Some);
    }

    Ok(None)
}

fn parse_env_proxy(variable: &str, value: &str) -> std::result::Result<ProxyEndpoint, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| format!("{variable} is not a valid proxy URL: {error}"))?;
    let proxy_type = match url.scheme() {
        "socks5" | "socks5h" => url.scheme().to_string(),
        "http" | "https" => url.scheme().to_string(),
        scheme => {
            return Err(format!(
                "{variable} uses unsupported proxy scheme '{scheme}'"
            ));
        }
    };
    let host = url
        .host_str()
        .filter(|host| !host.trim().is_empty())
        .ok_or_else(|| format!("{variable} does not contain a proxy host"))?
        .to_string();
    let port = url
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("{variable} does not contain a valid proxy port"))?;

    Ok(ProxyEndpoint {
        proxy_type,
        host,
        port: Some(port),
        user: url.username().to_string(),
        password: url.password().unwrap_or_default().to_string(),
    })
}

fn validate_proxy(endpoint: ProxyEndpoint, source: &str) -> Result<ResolvedProxy> {
    let kind = match endpoint.proxy_type.trim().to_ascii_lowercase().as_str() {
        "socks5" | "socks5h" => ProxyKind::Socks5,
        "http" => ProxyKind::Http,
        "https" => ProxyKind::Https,
        proxy_type => bail!("{source} proxy type '{proxy_type}' is unsupported"),
    };
    let host = endpoint.host.trim();
    if host.is_empty() {
        bail!("{source} proxy is enabled but its host is empty");
    }
    let Some(port) = endpoint.port.filter(|port| *port != 0) else {
        bail!("{source} proxy is enabled but its port is missing or invalid");
    };

    Ok(ResolvedProxy {
        kind,
        host: host.to_string(),
        port,
        user: endpoint.user,
        password: endpoint.password,
    })
}

fn resolve_proxy(session: &Session, config: &ConnectionProxyConfig) -> Result<ProxyRoute> {
    let session_proxy_type = session.proxy_type.trim();
    if session_proxy_type.eq_ignore_ascii_case("direct") {
        return Ok(ProxyRoute::Direct);
    }
    if !session_proxy_type.is_empty() && !session_proxy_type.eq_ignore_ascii_case("none") {
        return validate_proxy(
            ProxyEndpoint {
                proxy_type: session.proxy_type.clone(),
                host: session.proxy_host.clone(),
                port: session.proxy_port,
                user: session.proxy_user.clone(),
                password: session.proxy_password.clone(),
            },
            "session",
        )
        .map(ProxyRoute::Proxy);
    }

    if config.read_env_proxy {
        match &config.env_proxy {
            Ok(Some(proxy)) => {
                return validate_proxy(proxy.clone(), "environment").map(ProxyRoute::Proxy);
            }
            Ok(None) => {}
            Err(error) => bail!("environment proxy configuration is invalid: {error}"),
        }
    }

    if config.use_global_proxy {
        return validate_proxy(config.global_proxy.clone(), "global").map(ProxyRoute::Proxy);
    }

    if config.allow_direct {
        Ok(ProxyRoute::Direct)
    } else {
        bail!("persistent configuration is unavailable; refusing an unconfirmed direct connection")
    }
}

fn format_authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

const MAX_HTTP_CONNECT_RESPONSE_BYTES: usize = 16 * 1024;

fn validate_http_connect_response(response: &[u8]) -> Result<()> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut headers);
    match parsed
        .parse(response)
        .context("parse HTTP proxy CONNECT response")?
    {
        httparse::Status::Complete(_) => {}
        httparse::Status::Partial => bail!("HTTP proxy returned an incomplete CONNECT response"),
    }
    let status = parsed
        .code
        .context("HTTP proxy CONNECT response has no status code")?;
    if !(200..300).contains(&status) {
        bail!("HTTP proxy CONNECT failed with status {status}");
    }
    Ok(())
}

async fn establish_http_connect<S>(
    stream: S,
    proxy: &ResolvedProxy,
    target_host: &str,
    target_port: u16,
) -> Result<tokio::io::BufStream<S>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let mut stream = tokio::io::BufStream::new(stream);
    let authority = format_authority(target_host, target_port);
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if !proxy.user.is_empty() {
        let auth = format!("{}:{}", proxy.user, proxy.password);
        request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(auth)
        ));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .context("write HTTP proxy CONNECT request")?;
    stream
        .flush()
        .await
        .context("flush HTTP proxy CONNECT request")?;

    let mut response = Vec::with_capacity(512);
    loop {
        response.push(
            stream
                .read_u8()
                .await
                .context("read HTTP proxy CONNECT response")?,
        );
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
        if response.len() >= MAX_HTTP_CONNECT_RESPONSE_BYTES {
            bail!("HTTP proxy CONNECT response headers exceed 16 KiB");
        }
    }
    validate_http_connect_response(&response)?;
    Ok(stream)
}

fn native_https_client_config() -> Result<Arc<rustls::ClientConfig>> {
    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        tracing::warn!(%error, "[proxy] failed to load one system root certificate");
    }
    let mut roots = rustls::RootCertStore::empty();
    let (accepted, rejected) = roots.add_parsable_certificates(native.certs);
    if rejected > 0 {
        tracing::warn!(rejected, "[proxy] ignored invalid system root certificates");
    }
    if accepted == 0 {
        bail!("load HTTPS proxy system root certificates failed: no usable roots");
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

async fn connect_https_proxy_with_config(
    proxy: &ResolvedProxy,
    target_host: &str,
    target_port: u16,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<Box<dyn ProxyStream>> {
    let tcp = tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .context("connect TCP socket to HTTPS proxy")?;
    let server_name = rustls::pki_types::ServerName::try_from(proxy.host.clone())
        .context("HTTPS proxy host is not a valid TLS server name")?;
    let tls = tokio_rustls::TlsConnector::from(tls_config)
        .connect(server_name, tcp)
        .await
        .context("perform HTTPS proxy TLS handshake and certificate validation")?;
    let tunnel = establish_http_connect(tls, proxy, target_host, target_port)
        .await
        .context("establish CONNECT tunnel through HTTPS proxy")?;
    Ok(Box::new(tunnel))
}

async fn connect_proxy_with_tls_config(
    session: &Session,
    config: &ConnectionProxyConfig,
    test_tls_config: Option<Arc<rustls::ClientConfig>>,
) -> Result<Box<dyn ProxyStream>> {
    let route = resolve_proxy(session, config)?;
    let target_host = session.host.clone();
    let target_port = session.port;
    let connect_fut = async move {
        match route {
            ProxyRoute::Direct => {
                let stream = tokio::net::TcpStream::connect((target_host.as_str(), target_port))
                    .await
                    .with_context(|| {
                        format!("direct connection to {target_host}:{target_port} failed")
                    })?;
                Ok(Box::new(stream) as Box<dyn ProxyStream>)
            }
            ProxyRoute::Proxy(
                proxy @ ResolvedProxy {
                    kind: ProxyKind::Socks5,
                    ..
                },
            ) => {
                let proxy_address = (proxy.host.as_str(), proxy.port);
                if proxy.user.is_empty() {
                    let stream = tokio_socks::tcp::Socks5Stream::connect(
                        proxy_address,
                        (target_host.as_str(), target_port),
                    )
                    .await
                    .map_err(|error| anyhow::anyhow!("SOCKS5 proxy connection failed: {error}"))?;
                    Ok(Box::new(stream) as Box<dyn ProxyStream>)
                } else {
                    let stream = tokio_socks::tcp::Socks5Stream::connect_with_password(
                        proxy_address,
                        (target_host.as_str(), target_port),
                        &proxy.user,
                        &proxy.password,
                    )
                    .await
                    .map_err(|error| anyhow::anyhow!("SOCKS5 proxy connection failed: {error}"))?;
                    Ok(Box::new(stream) as Box<dyn ProxyStream>)
                }
            }
            ProxyRoute::Proxy(
                proxy @ ResolvedProxy {
                    kind: ProxyKind::Http,
                    ..
                },
            ) => {
                let stream = tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
                    .await
                    .map_err(|error| anyhow::anyhow!("HTTP proxy connection failed: {error}"))?;
                let tunnel =
                    establish_http_connect(stream, &proxy, &target_host, target_port).await?;
                Ok(Box::new(tunnel) as Box<dyn ProxyStream>)
            }
            ProxyRoute::Proxy(
                proxy @ ResolvedProxy {
                    kind: ProxyKind::Https,
                    ..
                },
            ) => {
                let tls_config = match test_tls_config {
                    Some(config) => config,
                    None => native_https_client_config()?,
                };
                connect_https_proxy_with_config(&proxy, &target_host, target_port, tls_config).await
            }
        }
    };

    tokio::time::timeout(std::time::Duration::from_secs(16), connect_fut)
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out after 16 seconds"))?
}

pub async fn connect_proxy(
    session: &Session,
    config: &ConnectionProxyConfig,
) -> Result<Box<dyn ProxyStream>> {
    connect_proxy_with_tls_config(session, config, None).await
}

pub fn active_proxy(
    session: &Session,
    config: &ConnectionProxyConfig,
) -> Result<Option<(String, String, u16)>> {
    match resolve_proxy(session, config)? {
        ProxyRoute::Direct => Ok(None),
        ProxyRoute::Proxy(proxy) => Ok(Some((
            proxy.kind.as_str().to_string(),
            proxy.host,
            proxy.port,
        ))),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedConfigEnvelopeV2 {
    format_version: u32,
    kdf: String,
    cipher: String,
    nonce: String,
    payload: String,
}

#[derive(Deserialize)]
struct EncryptedConfigHeader {
    format_version: u32,
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
                if let Ok(subkey) = hklm.open_subkey("SOFTWARE\\Microsoft\\Cryptography")
                    && let Ok(guid) = subkey.get_value::<String, _>("MachineGuid")
                {
                    let guid = guid.trim().to_string();
                    if !guid.is_empty() {
                        return guid;
                    }
                }
            }

            "ashell-default-hardware-uuid-fallback".to_string()
        })
        .clone()
}

fn config_format_version(raw: &[u8]) -> Result<u32> {
    serde_json::from_slice::<EncryptedConfigHeader>(raw)
        .map(|header| header.format_version)
        .context("parse encrypted config header")
}

fn parse_legacy_plaintext_config(raw: &[u8], from_legacy_path: bool) -> Result<ConfigFile> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).context("parse plaintext config JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("plaintext config must be a JSON object"))?;

    const ENVELOPE_FIELDS: &[&str] = &[
        "format_version",
        "kdf",
        "cipher",
        "salt",
        "nonce",
        "payload",
    ];
    if ENVELOPE_FIELDS
        .iter()
        .any(|field| object.contains_key(*field))
    {
        return Err(anyhow::anyhow!(
            "encrypted config envelope must not be treated as plaintext"
        ));
    }

    const CONFIG_FIELDS: &[&str] = &[
        "sessions",
        "session_folders",
        "font_defaults_version",
        "terminal_font_family",
        "ui_font_family",
        "theme_mode",
        "window_bounds",
        "sync_device_id",
    ];
    if !from_legacy_path
        && !CONFIG_FIELDS
            .iter()
            .any(|field| object.contains_key(*field))
    {
        return Err(anyhow::anyhow!(
            "JSON object does not contain recognized legacy config fields"
        ));
    }

    serde_json::from_value(value).context("parse recognized plaintext config")
}

fn encrypt_config_v2(config: &ConfigFile, key: &MasterKey) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);

    let plaintext = serde_json::to_vec(config).context("serialize config")?;
    let ciphertext = XChaCha20Poly1305::new(key.as_bytes().into())
        .encrypt(XNonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| anyhow::anyhow!("encrypt version 2 config payload"))?;

    serde_json::to_vec_pretty(&EncryptedConfigEnvelopeV2 {
        format_version: 2,
        kdf: "os-keyring".to_string(),
        cipher: "xchacha20poly1305".to_string(),
        nonce: STANDARD.encode(nonce),
        payload: STANDARD.encode(ciphertext),
    })
    .context("serialize version 2 encrypted config envelope")
}

fn decrypt_config_v2(raw: &[u8], key: &MasterKey) -> Result<ConfigFile> {
    let envelope: EncryptedConfigEnvelopeV2 =
        serde_json::from_slice(raw).context("parse version 2 encrypted config envelope")?;
    if envelope.format_version != 2
        || envelope.kdf != "os-keyring"
        || envelope.cipher != "xchacha20poly1305"
    {
        return Err(anyhow::anyhow!(
            "unsupported version 2 encrypted config format"
        ));
    }

    let nonce = STANDARD
        .decode(envelope.nonce)
        .context("decode version 2 config nonce")?;
    if nonce.len() != 24 {
        return Err(anyhow::anyhow!("invalid version 2 config nonce"));
    }
    let ciphertext = STANDARD
        .decode(envelope.payload)
        .context("decode version 2 encrypted config payload")?;
    let plaintext = XChaCha20Poly1305::new(key.as_bytes().into())
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| {
            anyhow::anyhow!(
                "cannot decrypt version 2 config; master key mismatch or corrupted data"
            )
        })?;

    serde_json::from_slice(&plaintext).context("parse decrypted version 2 config")
}

#[cfg(test)]
fn encrypt_config_v1(config: &ConfigFile, password: &str) -> Result<Vec<u8>> {
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

fn decrypt_config_v1(raw: &[u8], password: &str) -> Result<ConfigFile> {
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
    use std::{
        cell::{Cell, RefCell},
        sync::Arc,
        time::Duration,
    };

    use super::*;

    use crate::session::config_key::{ConfigKeyProvider, MasterKey};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[derive(Default)]
    struct TestKeyProvider {
        key: RefCell<Option<MasterKey>>,
        create_calls: Cell<usize>,
    }

    impl ConfigKeyProvider for TestKeyProvider {
        fn load_existing(&self) -> Result<MasterKey> {
            self.key
                .borrow()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("test master key is missing"))
        }

        fn load_or_create(&self) -> Result<MasterKey> {
            self.create_calls.set(self.create_calls.get() + 1);
            if let Some(key) = self.key.borrow().clone() {
                return Ok(key);
            }
            let key = MasterKey::from_secret(vec![41; 32])?;
            self.key.replace(Some(key.clone()));
            Ok(key)
        }
    }

    fn portable_apply_session(id: &str, host: &str) -> Session {
        let mut session = Session::password(
            host.to_string(),
            22,
            "remote-user".to_string(),
            format!("password-{id}"),
        );
        session.id = id.to_string();
        session.name = format!("Remote {id}");
        session
    }

    fn portable_apply_remote_config() -> ConfigFile {
        ConfigFile {
            follow_system_theme: false,
            theme_mode: "remote-dark".to_string(),
            light_theme_name: "remote-light-theme".to_string(),
            dark_theme_name: "remote-dark-theme".to_string(),
            locale: "remote-locale".to_string(),
            terminal_font_size: 21.0,
            ui_font_size: 17.0,
            right_click_copy_paste: true,
            keyword_highlight: true,
            editor_soft_wrap: true,
            history_completion_plugin_enabled: false,
            ui_font_family: "Remote UI Font".to_string(),
            terminal_font_family: "Remote Terminal Font".to_string(),
            cursor_style: CursorStyle::BeamBlink,
            sessions: vec![
                portable_apply_session("remote-a", "a.remote.test"),
                portable_apply_session("remote-b", "b.remote.test"),
            ],
            session_folders: vec![SessionFolder {
                id: "remote-folder".to_string(),
                name: "Remote Folder".to_string(),
                session_ids: vec!["remote-b".to_string(), "remote-a".to_string()],
            }],
            show_hidden_files: true,
            lock_layout: true,
            monitoring_position: "RemoteStatusBar".to_string(),
            key_bindings: std::collections::HashMap::from([
                ("Copy".to_string(), "remote-copy".to_string()),
                ("Paste".to_string(), "remote-paste".to_string()),
            ]),
            use_proxy: true,
            read_env_proxy: false,
            global_proxy_type: "https".to_string(),
            global_proxy_host: "remote-proxy.test".to_string(),
            global_proxy_port: Some(9443),
            global_proxy_user: "remote-proxy-user".to_string(),
            global_proxy_password: "remote-proxy-password".to_string(),
            ..ConfigFile::default()
        }
    }

    fn portable_apply_local_config() -> ConfigFile {
        ConfigFile {
            font_defaults_version: 77,
            window_bounds: Some(SavedWindowBounds::Windowed {
                x: 10_001.0,
                y: 10_002.0,
                width: 10_003.0,
                height: 10_004.0,
            }),
            workspace_panels: Some(vec![20_001.0, 20_002.0]),
            body_panels: Some(vec![30_001.0, 30_002.0]),
            transfers: vec![crate::terminal::Transfer {
                tab_id: "local-transfer-tab".to_string(),
                generation: 42,
                tab_title: "Local Transfer".to_string(),
                info: crate::terminal::TransferInfo {
                    id: "local-transfer-id".to_string(),
                    name: "local-transfer-name".to_string(),
                    source: "local-transfer-source".to_string(),
                    target: "local-transfer-target".to_string(),
                    kind: crate::terminal::TransferType::Download,
                    total_bytes: Some(12_345),
                },
                transferred: 1_234,
                total: Some(12_345),
                state: crate::terminal::TransferState::Paused,
            }],
            sidebar_collapsed: true,
            sftp_panel_minimized: true,
            sync_endpoint: "https://local-webdav.test/config".to_string(),
            sync_username: "local-webdav-user".to_string(),
            sync_etag: Some("old-etag".to_string()),
            sync_etag_target: "old-target".to_string(),
            sync_device_id: "local-device-id".to_string(),
            sync_backend: "r2".to_string(),
            sync_etag_backend: "legacy-backend".to_string(),
            sync_s3_endpoint: "https://local-s3.test".to_string(),
            sync_s3_region: "local-region".to_string(),
            sync_s3_bucket: "local-s3-bucket".to_string(),
            sync_s3_object_key: "local-s3-key.json".to_string(),
            sync_r2_account_id: "local-r2-account".to_string(),
            sync_r2_bucket: "local-r2-bucket".to_string(),
            sync_r2_object_key: "local-r2-key.json".to_string(),
            sync_r2_access_key_id: "local-r2-access-key-id".to_string(),
            sync_remember_encryption_password: true,
            ..ConfigFile::default()
        }
    }

    fn portable_apply_local_only_snapshot(config: &ConfigFile) -> serde_json::Value {
        serde_json::json!({
            "font_defaults_version": config.font_defaults_version,
            "window_bounds": &config.window_bounds,
            "workspace_panels": &config.workspace_panels,
            "body_panels": &config.body_panels,
            "transfers": &config.transfers,
            "sidebar_collapsed": config.sidebar_collapsed,
            "sftp_panel_minimized": config.sftp_panel_minimized,
            "sync_endpoint": &config.sync_endpoint,
            "sync_username": &config.sync_username,
            "sync_device_id": &config.sync_device_id,
            "sync_backend": &config.sync_backend,
            "sync_etag_backend": &config.sync_etag_backend,
            "sync_s3_endpoint": &config.sync_s3_endpoint,
            "sync_s3_region": &config.sync_s3_region,
            "sync_s3_bucket": &config.sync_s3_bucket,
            "sync_s3_object_key": &config.sync_s3_object_key,
            "sync_r2_account_id": &config.sync_r2_account_id,
            "sync_r2_bucket": &config.sync_r2_bucket,
            "sync_r2_object_key": &config.sync_r2_object_key,
            "sync_r2_access_key_id": &config.sync_r2_access_key_id,
            "sync_remember_encryption_password": config.sync_remember_encryption_password,
        })
    }

    fn portable_apply_preferences_snapshot(config: &ConfigFile) -> serde_json::Value {
        let portable = crate::sync::PortableConfigV2::from(config);
        serde_json::json!({
            "preferences": portable.preferences,
            "key_bindings": portable.key_bindings,
            "proxy": portable.proxy,
        })
    }

    #[test]
    fn default_terminal_font_follows_the_system_monospace_font() {
        let config = ConfigFile::default();
        assert_eq!(config.ui_font_family, "Noto Sans CJK SC");
        assert_eq!(config.terminal_font_family, ".SystemMonospace");
        assert_eq!(config.terminal_font_size, 16.0);
        assert_eq!(config.font_defaults_version, 4);
    }

    #[test]
    fn default_ssh_rsa_algorithms_use_sha2_only() {
        use russh::keys::{Algorithm, HashAlg};

        let rsa_algorithms = russh::Preferred::default()
            .key
            .iter()
            .filter_map(|algorithm| match algorithm {
                Algorithm::Rsa { .. } => Some(algorithm.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rsa_algorithms,
            vec![
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512),
                },
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256),
                },
            ]
        );
    }

    fn direct_proxy_config() -> ConnectionProxyConfig {
        ConnectionProxyConfig {
            read_env_proxy: false,
            use_global_proxy: false,
            global_proxy: ProxyEndpoint {
                proxy_type: "socks5".to_string(),
                host: String::new(),
                port: None,
                user: String::new(),
                password: String::new(),
            },
            env_proxy: Ok(None),
            allow_direct: true,
        }
    }

    fn proxy_endpoint(proxy_type: &str, host: &str, port: u16) -> ProxyEndpoint {
        ProxyEndpoint {
            proxy_type: proxy_type.to_string(),
            host: host.to_string(),
            port: Some(port),
            user: String::new(),
            password: String::new(),
        }
    }

    fn test_tls_configs(names: &[&str]) -> (Arc<rustls::ServerConfig>, Arc<rustls::ClientConfig>) {
        use rcgen::{
            BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
            KeyUsagePurpose,
        };
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(vec!["ashell-test-ca".to_string()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::CrlSign,
        ];
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();

        let leaf_key = KeyPair::generate().unwrap();
        let mut leaf_params = CertificateParams::new(
            names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();

        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf_cert.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
            )
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca_cert.der().clone()).unwrap();
        let client = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        (Arc::new(server), Arc::new(client))
    }

    const TEST_PROXY_TASK_TIMEOUT: Duration = Duration::from_secs(2);

    async fn abort_and_join_test_proxy_task(
        task: tokio::task::JoinHandle<Result<()>>,
    ) -> Result<()> {
        task.abort();
        match task.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => {
                Err(anyhow::Error::new(error).context("join test proxy server task after abort"))
            }
        }
    }

    async fn join_test_proxy_task(
        mut task: tokio::task::JoinHandle<Result<()>>,
    ) -> Result<Result<()>> {
        match tokio::time::timeout(TEST_PROXY_TASK_TIMEOUT, &mut task).await {
            Ok(result) => Ok(result.context("join test proxy server task")?),
            Err(_) => {
                abort_and_join_test_proxy_task(task).await?;
                bail!("test proxy server task timed out");
            }
        }
    }

    async fn assert_test_proxy_succeeded(task: tokio::task::JoinHandle<Result<()>>) {
        join_test_proxy_task(task)
            .await
            .expect("test proxy server task must not time out or panic")
            .expect("test proxy server task must succeed");
    }

    async fn assert_tls_handshake_rejected(task: tokio::task::JoinHandle<Result<()>>) {
        let result = join_test_proxy_task(task)
            .await
            .expect("test TLS proxy task must not time out or panic");
        let error = result.expect_err("test TLS proxy handshake must be rejected");
        assert!(
            format!("{error:#}").contains("accept test TLS connection"),
            "unexpected test TLS proxy error: {error:#}"
        );
    }

    async fn spawn_tls_proxy(
        server_config: Arc<rustls::ServerConfig>,
        response: Vec<u8>,
    ) -> (u16, tokio::task::JoinHandle<Result<()>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (tcp, _) = listener
                .accept()
                .await
                .context("accept test TLS proxy TCP connection")?;
            let mut tls = tokio_rustls::TlsAcceptor::from(server_config)
                .accept(tcp)
                .await
                .context("accept test TLS connection")?;
            let mut request = Vec::new();
            loop {
                request.push(tls.read_u8().await.context("read test CONNECT request")?);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
                if request.len() >= 16 * 1024 {
                    bail!("test CONNECT request exceeded 16 KiB");
                }
            }
            tls.write_all(&response)
                .await
                .context("write test TLS proxy response")?;
            tls.flush().await.context("flush test TLS proxy response")?;
            Ok(())
        });
        (port, task)
    }

    fn resolved_https_proxy(port: u16) -> ResolvedProxy {
        ResolvedProxy {
            kind: ProxyKind::Https,
            host: "localhost".to_string(),
            port,
            user: String::new(),
            password: String::new(),
        }
    }

    #[test]
    fn explicit_direct_bypasses_environment_global_and_persistence_gate() {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = "direct".to_string();

        let mut config = direct_proxy_config();
        config.read_env_proxy = true;
        config.env_proxy = Err("invalid environment proxy".to_string());
        config.use_global_proxy = true;
        config.global_proxy = proxy_endpoint("unknown", "", 0);
        config.allow_direct = false;

        assert_eq!(
            resolve_proxy(&session, &config).unwrap(),
            ProxyRoute::Direct
        );
    }

    #[test]
    fn none_and_empty_session_proxy_inherit_environment_then_global() {
        for session_proxy_type in ["", "none"] {
            let mut session = Session::password(
                "example.test".to_string(),
                22,
                "root".to_string(),
                "secret".to_string(),
            );
            session.proxy_type = session_proxy_type.to_string();

            let mut config = direct_proxy_config();
            config.read_env_proxy = true;
            config.env_proxy = Ok(Some(proxy_endpoint("https", "env.proxy", 443)));
            config.use_global_proxy = true;
            config.global_proxy = proxy_endpoint("http", "global.proxy", 8080);

            assert_eq!(
                resolve_proxy(&session, &config).unwrap(),
                ProxyRoute::Proxy(ResolvedProxy {
                    kind: ProxyKind::Https,
                    host: "env.proxy".to_string(),
                    port: 443,
                    user: String::new(),
                    password: String::new(),
                })
            );

            config.env_proxy = Ok(None);
            assert_eq!(
                resolve_proxy(&session, &config).unwrap(),
                ProxyRoute::Proxy(ResolvedProxy {
                    kind: ProxyKind::Http,
                    host: "global.proxy".to_string(),
                    port: 8080,
                    user: String::new(),
                    password: String::new(),
                })
            );
        }
    }

    #[test]
    fn legacy_explicit_proxy_types_remain_supported() {
        for (proxy_type, expected_kind) in [
            ("socks5", ProxyKind::Socks5),
            ("socks5h", ProxyKind::Socks5),
            ("http", ProxyKind::Http),
        ] {
            let mut session = Session::password(
                "example.test".to_string(),
                22,
                "root".to_string(),
                "secret".to_string(),
            );
            session.proxy_type = proxy_type.to_string();
            session.proxy_host = "session.proxy".to_string();
            session.proxy_port = Some(8080);

            assert_eq!(
                resolve_proxy(&session, &direct_proxy_config()).unwrap(),
                ProxyRoute::Proxy(ResolvedProxy {
                    kind: expected_kind,
                    host: "session.proxy".to_string(),
                    port: 8080,
                    user: String::new(),
                    password: String::new(),
                })
            );
        }
    }

    #[test]
    fn https_environment_proxy_uses_port_443_by_default() {
        let proxy = parse_env_proxy("HTTPS_PROXY", "https://proxy.example").unwrap();

        assert_eq!(proxy.proxy_type, "https");
        assert_eq!(proxy.host, "proxy.example");
        assert_eq!(proxy.port, Some(443));
    }

    #[test]
    fn global_https_proxy_is_resolved_for_an_inherited_session() {
        let session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        let mut config = direct_proxy_config();
        config.use_global_proxy = true;
        config.global_proxy = proxy_endpoint("https", "global.proxy", 443);

        assert_eq!(
            resolve_proxy(&session, &config).unwrap(),
            ProxyRoute::Proxy(ResolvedProxy {
                kind: ProxyKind::Https,
                host: "global.proxy".to_string(),
                port: 443,
                user: String::new(),
                password: String::new(),
            })
        );
    }

    #[tokio::test]
    async fn trusted_https_proxy_completes_tls_connect_and_preserves_tunnel_bytes() {
        let (server_config, client_config) = test_tls_configs(&["localhost"]);
        let (port, server) = spawn_tls_proxy(
            server_config,
            b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n".to_vec(),
        )
        .await;
        let mut stream = connect_https_proxy_with_config(
            &resolved_https_proxy(port),
            "target.example",
            22,
            client_config,
        )
        .await
        .expect("trusted HTTPS proxy should establish a CONNECT tunnel");
        let mut banner = [0_u8; 14];
        stream.read_exact(&mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-test\r\n");
        assert_test_proxy_succeeded(server).await;
    }

    #[tokio::test]
    async fn https_proxy_rejects_an_untrusted_certificate() {
        let (server_config, _) = test_tls_configs(&["localhost"]);
        let (port, server) =
            spawn_tls_proxy(server_config, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).await;
        let client = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        );
        let error = connect_https_proxy_with_config(
            &resolved_https_proxy(port),
            "target.example",
            22,
            client,
        )
        .await
        .err()
        .expect("an untrusted HTTPS proxy certificate must be rejected");
        assert!(format!("{error:#}").contains("certificate validation"));
        assert_tls_handshake_rejected(server).await;
    }

    #[tokio::test]
    async fn https_proxy_rejects_a_mismatched_server_name() {
        let (server_config, client_config) = test_tls_configs(&["wrong.example"]);
        let (port, server) =
            spawn_tls_proxy(server_config, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).await;
        let error = connect_https_proxy_with_config(
            &resolved_https_proxy(port),
            "target.example",
            22,
            client_config,
        )
        .await
        .err()
        .expect("a mismatched HTTPS proxy server name must be rejected");
        assert!(format!("{error:#}").contains("certificate validation"));
        assert_tls_handshake_rejected(server).await;
    }

    #[tokio::test]
    async fn https_proxy_surfaces_non_success_connect_status() {
        let (server_config, client_config) = test_tls_configs(&["localhost"]);
        let (port, server) = spawn_tls_proxy(
            server_config,
            b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".to_vec(),
        )
        .await;
        let error = connect_https_proxy_with_config(
            &resolved_https_proxy(port),
            "target.example",
            22,
            client_config,
        )
        .await
        .err()
        .expect("a non-success HTTPS CONNECT status must be returned");
        assert!(format!("{error:#}").contains("status 407"));
        assert_test_proxy_succeeded(server).await;
    }

    #[tokio::test]
    async fn https_tls_failure_does_not_fall_back_to_direct() {
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target_listener.local_addr().unwrap().port();
        let (server_config, _) = test_tls_configs(&["localhost"]);
        let (proxy_port, proxy_server) =
            spawn_tls_proxy(server_config, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()).await;
        let untrusted_client = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        );
        let mut session = Session::password(
            "127.0.0.1".to_string(),
            target_port,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = "https".to_string();
        session.proxy_host = "localhost".to_string();
        session.proxy_port = Some(proxy_port);
        session.proxy_user = "https-proxy-user-must-not-leak".to_string();
        session.proxy_password = "https-proxy-password-must-not-leak".to_string();
        let config = direct_proxy_config();
        let connect = connect_proxy_with_tls_config(&session, &config, Some(untrusted_client));

        let error = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::pin!(connect);
            tokio::select! {
                biased;
                accepted = target_listener.accept() => match accepted {
                    Ok((_, peer_address)) => panic!(
                        "HTTPS TLS failure fell back to direct target {peer_address}"
                    ),
                    Err(error) => panic!("accept direct target connection: {error}"),
                },
                result = &mut connect => match result {
                    Ok(_) => panic!("HTTPS TLS failure unexpectedly returned a stream"),
                    Err(error) => error,
                },
            }
        })
        .await
        .expect("HTTPS TLS failure did not finish within the overall timeout");
        let error_chain = format!("{error:#}");
        assert!(error_chain.contains("certificate validation"));
        for sensitive_value in [
            session.proxy_user.as_str(),
            session.proxy_password.as_str(),
            "Basic",
            "Proxy-Authorization",
        ] {
            assert!(
                !error_chain.contains(sensitive_value),
                "HTTPS proxy error leaked sensitive authentication data"
            );
        }
        assert_tls_handshake_rejected(proxy_server).await;
    }

    #[tokio::test]
    async fn https_connect_failure_does_not_fall_back_to_direct() {
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target_listener.local_addr().unwrap().port();
        let (server_config, client_config) = test_tls_configs(&["localhost"]);
        let (proxy_port, proxy_server) = spawn_tls_proxy(
            server_config,
            b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".to_vec(),
        )
        .await;
        let mut session = Session::password(
            "127.0.0.1".to_string(),
            target_port,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = "https".to_string();
        session.proxy_host = "localhost".to_string();
        session.proxy_port = Some(proxy_port);
        session.proxy_user = "connect-proxy-user-must-not-leak".to_string();
        session.proxy_password = "connect-proxy-password-must-not-leak".to_string();
        let config = direct_proxy_config();
        let connect = connect_proxy_with_tls_config(&session, &config, Some(client_config));

        let error = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::pin!(connect);
            tokio::select! {
                biased;
                accepted = target_listener.accept() => match accepted {
                    Ok((_, peer_address)) => panic!(
                        "HTTPS CONNECT failure fell back to direct target {peer_address}"
                    ),
                    Err(error) => panic!("accept direct target connection: {error}"),
                },
                result = &mut connect => match result {
                    Ok(_) => panic!("HTTPS CONNECT failure unexpectedly returned a stream"),
                    Err(error) => error,
                },
            }
        })
        .await
        .expect("HTTPS CONNECT failure did not finish within the overall timeout");
        let error_chain = format!("{error:#}");
        assert!(error_chain.contains("status 407"));
        for sensitive_value in [
            session.proxy_user.as_str(),
            session.proxy_password.as_str(),
            "Basic",
            "Proxy-Authorization",
        ] {
            assert!(
                !error_chain.contains(sensitive_value),
                "HTTPS proxy error leaked sensitive authentication data"
            );
        }
        assert_test_proxy_succeeded(proxy_server).await;
    }

    #[test]
    fn unknown_type_and_zero_port_fail_closed() {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = "ftp".to_string();
        session.proxy_host = "proxy.example".to_string();
        session.proxy_port = Some(21);

        let unsupported = resolve_proxy(&session, &direct_proxy_config())
            .expect_err("unknown proxy types must be rejected");
        assert!(unsupported.to_string().contains("unsupported"));

        session.proxy_type = "http".to_string();
        session.proxy_port = Some(0);
        let zero_port = resolve_proxy(&session, &direct_proxy_config())
            .expect_err("zero proxy ports must be rejected");
        assert!(zero_port.to_string().contains("missing or invalid"));

        session.proxy_port = None;
        let missing_port = resolve_proxy(&session, &direct_proxy_config())
            .expect_err("missing proxy ports must be rejected");
        assert!(missing_port.to_string().contains("missing or invalid"));
    }

    #[test]
    fn legacy_session_without_proxy_type_inherits_proxy_settings() {
        let session: Session = serde_json::from_str(
            r#"{"id":"1","name":"legacy","host":"example.test","port":22,"user":"root","auth":"password"}"#,
        )
        .unwrap();

        assert_eq!(session.proxy_type, "none");
    }

    #[test]
    fn session_proxy_has_priority_over_environment_and_global_proxy() {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = "http".to_string();
        session.proxy_host = "session.proxy".to_string();
        session.proxy_port = Some(8080);
        let mut config = direct_proxy_config();
        config.read_env_proxy = true;
        config.env_proxy = Ok(Some(proxy_endpoint("socks5", "env.proxy", 1080)));
        config.use_global_proxy = true;
        config.global_proxy = proxy_endpoint("socks5", "global.proxy", 1080);

        assert_eq!(
            resolve_proxy(&session, &config).unwrap(),
            ProxyRoute::Proxy(ResolvedProxy {
                kind: ProxyKind::Http,
                host: "session.proxy".to_string(),
                port: 8080,
                user: String::new(),
                password: String::new(),
            })
        );
    }

    #[test]
    fn environment_proxy_has_priority_over_global_proxy() {
        let session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        let mut config = direct_proxy_config();
        config.read_env_proxy = true;
        config.env_proxy = Ok(Some(proxy_endpoint("socks5", "env.proxy", 1080)));
        config.use_global_proxy = true;
        config.global_proxy = proxy_endpoint("http", "global.proxy", 8080);

        assert!(matches!(
            resolve_proxy(&session, &config).unwrap(),
            ProxyRoute::Proxy(ResolvedProxy {
                kind: ProxyKind::Socks5,
                ref host,
                port: 1080,
                ..
            }) if host == "env.proxy"
        ));
    }

    #[test]
    fn invalid_selected_proxy_does_not_fall_back_to_direct_connection() {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        session.proxy_type = "http".to_string();
        session.proxy_host = String::new();
        session.proxy_port = Some(8080);

        let error = resolve_proxy(&session, &direct_proxy_config()).unwrap_err();
        assert!(error.to_string().contains("host is empty"));
    }

    #[test]
    fn unavailable_persistent_configuration_refuses_unconfirmed_direct_connection() {
        let session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "secret".to_string(),
        );
        let mut config = direct_proxy_config();
        config.allow_direct = false;

        let error = resolve_proxy(&session, &config).unwrap_err();
        assert!(error.to_string().contains("refusing"));
    }

    #[test]
    fn environment_proxy_parser_rejects_unsupported_schemes() {
        let error = parse_env_proxy("ALL_PROXY", "ftp://proxy.example:21").unwrap_err();
        assert!(error.contains("unsupported proxy scheme 'ftp'"));

        let proxy = parse_env_proxy("HTTP_PROXY", "http://user:pass@proxy.example").unwrap();
        assert_eq!(proxy.host, "proxy.example");
        assert_eq!(proxy.port, Some(80));
        assert_eq!(proxy.user, "user");
        assert_eq!(proxy.password, "pass");
    }

    async fn connect_over_duplex(
        response: Vec<u8>,
    ) -> Result<tokio::io::BufStream<tokio::io::DuplexStream>> {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let mut request = Vec::new();
            loop {
                request.push(
                    server
                        .read_u8()
                        .await
                        .context("read HTTP CONNECT test request")?,
                );
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
                if request.len() >= MAX_HTTP_CONNECT_RESPONSE_BYTES {
                    bail!("HTTP CONNECT test request exceeded 16 KiB");
                }
            }
            let request =
                String::from_utf8(request).context("decode HTTP CONNECT test request as UTF-8")?;
            if !request
                .starts_with("CONNECT target.example:22 HTTP/1.1\r\nHost: target.example:22\r\n")
            {
                bail!("HTTP CONNECT test request has an unexpected authority or Host header");
            }
            server
                .write_all(&response)
                .await
                .context("write HTTP CONNECT test response")?;
            server
                .flush()
                .await
                .context("flush HTTP CONNECT test response")?;
            Ok(())
        });
        let connect_result = match tokio::time::timeout(
            TEST_PROXY_TASK_TIMEOUT,
            establish_http_connect(
                client,
                &ResolvedProxy {
                    kind: ProxyKind::Http,
                    host: "proxy.example".to_string(),
                    port: 8080,
                    user: String::new(),
                    password: String::new(),
                },
                "target.example",
                22,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                abort_and_join_test_proxy_task(server_task).await?;
                bail!("establish HTTP CONNECT over duplex timed out");
            }
        };
        join_test_proxy_task(server_task).await??;
        connect_result
    }

    #[test]
    fn http_connect_validation_accepts_only_real_2xx_statuses() {
        for status in [200, 204, 299] {
            let response = format!("HTTP/1.1 {status} Result\r\n\r\n");
            validate_http_connect_response(response.as_bytes()).unwrap();
        }
        let error = validate_http_connect_response(b"HTTP/1.1 300 Redirect\r\n\r\n").unwrap_err();
        assert!(error.to_string().contains("status 300"));
    }

    #[tokio::test]
    async fn http_connect_rejects_headers_larger_than_16_kib() {
        let mut response = b"HTTP/1.1 200 OK\r\nX-Fill: ".to_vec();
        response.extend(vec![b'a'; 16 * 1024]);
        let error = connect_over_duplex(response).await.unwrap_err();
        assert!(error.to_string().contains("exceed 16 KiB"));
    }

    #[tokio::test]
    async fn http_connect_rejects_an_incomplete_response() {
        let error = connect_over_duplex(b"HTTP/1.1 200 OK\r\n".to_vec())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("read HTTP proxy CONNECT response"));
    }

    #[tokio::test]
    async fn http_connect_preserves_bytes_after_response_headers() {
        let mut stream = connect_over_duplex(
            b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n".to_vec(),
        )
        .await
        .unwrap();
        let mut banner = [0_u8; 14];
        stream.read_exact(&mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-test\r\n");
    }

    #[test]
    fn http_connect_validation_requires_a_success_status_code() {
        validate_http_connect_response(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();

        let error = validate_http_connect_response(
            b"HTTP/1.1 500 Proxy Error\r\nX-Debug: expected 200 later\r\n\r\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("status 500"));
        assert!(validate_http_connect_response(b"HTTP/1.1 200").is_err());
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
    fn editor_soft_wrap_is_backward_compatible_and_serializable() {
        let legacy_config: ConfigFile = serde_json::from_str("{}").unwrap();
        assert!(!legacy_config.editor_soft_wrap);

        let config = ConfigFile {
            editor_soft_wrap: true,
            ..ConfigFile::default()
        };
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("\"editor_soft_wrap\":true"));

        let restored: ConfigFile = serde_json::from_str(&serialized).unwrap();
        assert!(restored.editor_soft_wrap);
    }

    #[test]
    fn legacy_config_defaults_to_terminal_switch_follow() {
        let legacy_config: ConfigFile = serde_json::from_str("{}").unwrap();

        assert_eq!(
            legacy_config.sftp_cwd_sync_mode,
            SftpCwdSyncMode::OnTerminalSwitch
        );
    }

    #[test]
    fn unknown_sftp_cwd_sync_mode_falls_back_to_terminal_switch() {
        let config: ConfigFile =
            serde_json::from_str(r#"{"sftp_cwd_sync_mode":"future-mode"}"#).unwrap();

        assert_eq!(config.sftp_cwd_sync_mode, SftpCwdSyncMode::OnTerminalSwitch);
    }

    #[test]
    fn realtime_sftp_cwd_sync_mode_serializes_and_deserializes() {
        let config = ConfigFile {
            sftp_cwd_sync_mode: SftpCwdSyncMode::Realtime,
            ..ConfigFile::default()
        };

        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains(r#""sftp_cwd_sync_mode":"realtime""#));

        let restored: ConfigFile = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.sftp_cwd_sync_mode, SftpCwdSyncMode::Realtime);

        let key = MasterKey::from_secret(vec![29; 32]).unwrap();
        let encrypted = encrypt_config_v2(&config, &key).unwrap();
        let decrypted = decrypt_config_v2(&encrypted, &key).unwrap();
        assert_eq!(decrypted.sftp_cwd_sync_mode, SftpCwdSyncMode::Realtime);
    }

    #[test]
    fn sync_target_fields_are_backward_compatible() {
        let legacy_config: ConfigFile = serde_json::from_str("{}").unwrap();

        assert!(legacy_config.sync_etag_target.is_empty());
        assert!(legacy_config.sync_r2_account_id.is_empty());
        assert!(legacy_config.sync_r2_bucket.is_empty());
        assert_eq!(legacy_config.sync_r2_object_key, "jshell-sync.json");
        assert!(legacy_config.sync_r2_access_key_id.is_empty());
        assert!(!legacy_config.sync_remember_encryption_password);
    }

    #[test]
    fn sync_etag_matches_only_the_current_webdav_target() {
        let mut store = ConfigStore::in_memory();
        store.set_sync_backend("webdav");
        store.set_sync_connection(
            "https://dav.example.test/config/".to_string(),
            "alice".to_string(),
        );

        store.set_sync_etag(Some("webdav-etag".to_string()));

        assert_eq!(store.sync_etag(), Some("webdav-etag"));
        assert_eq!(
            store.cache.sync_etag_target,
            crate::sync::SyncTargetId::for_webdav(
                "https://dav.example.test/config/jshell-sync.json",
                "alice",
            )
            .to_string()
        );

        store.cache.sync_endpoint = "https://dav.example.test/other".to_string();
        assert_eq!(store.sync_etag(), None);
    }

    #[test]
    fn sync_etag_does_not_cross_bucket_key_or_provider_boundaries() {
        let mut store = ConfigStore::in_memory();
        store.set_sync_backend("s3");
        store.set_sync_s3_connection(
            "https://objects.example.test/".to_string(),
            "auto".to_string(),
            "bucket-a".to_string(),
            "/config-a.json".to_string(),
        );
        store.set_sync_etag(Some("s3-etag".to_string()));
        assert_eq!(store.sync_etag(), Some("s3-etag"));

        store.cache.sync_s3_bucket = "bucket-b".to_string();
        assert_eq!(store.sync_etag(), None);

        store.cache.sync_s3_bucket = "bucket-a".to_string();
        store.cache.sync_s3_object_key = "config-b.json".to_string();
        assert_eq!(store.sync_etag(), None);

        store.cache.sync_s3_object_key = "config-a.json".to_string();
        store.cache.sync_r2_account_id = "objects.example.test".to_string();
        store.cache.sync_r2_bucket = "bucket-a".to_string();
        store.cache.sync_r2_object_key = "config-a.json".to_string();
        store.set_sync_backend("r2");
        assert_eq!(store.sync_backend(), "r2");
        assert_eq!(store.sync_etag(), None);
    }

    #[test]
    fn legacy_sync_etag_backend_never_authorizes_an_etag_match() {
        let mut store = ConfigStore::in_memory();
        store.set_sync_backend("webdav");
        store.set_sync_connection(
            "https://dav.example.test/config".to_string(),
            "alice".to_string(),
        );
        store.cache.sync_etag = Some("legacy-etag".to_string());
        store.cache.sync_etag_backend = "webdav".to_string();
        store.cache.sync_etag_target.clear();

        assert_eq!(store.sync_etag(), None);
    }

    #[test]
    fn setting_sync_etag_without_a_valid_target_clears_the_binding() {
        let mut store = ConfigStore::in_memory();
        store.cache.sync_etag = Some("stale-etag".to_string());
        store.cache.sync_etag_target = "stale-target".to_string();
        store.set_sync_backend("r2");

        store.set_sync_etag(Some("new-etag".to_string()));

        assert_eq!(store.sync_etag(), None);
        assert_eq!(store.cache.sync_etag, None);
        assert!(store.cache.sync_etag_target.is_empty());
    }

    #[test]
    fn sync_connection_persist_failure_preserves_cache_and_disk() {
        let root = std::env::temp_dir().join(format!("jshell-sync-connection-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        let master_key = MasterKey::from_secret(vec![73; 32]).unwrap();
        let mut store = ConfigStore {
            path: path.clone(),
            cache: portable_apply_local_config(),
            master_key,
        };
        store.save().unwrap();
        let cache_before = serde_json::to_value(&store.cache).unwrap();
        let disk_before = fs::read(&path).unwrap();
        let connection = SyncConnectionSnapshot::r2(
            "0123456789abcdef0123456789abcdef".to_string(),
            "new-bucket".to_string(),
            "new-object.json".to_string(),
            "new-access-key".to_string(),
            true,
        );

        let error = store
            .persist_sync_connection_with_persist(
                &connection,
                Some("new-etag".to_string()),
                |_path, _bytes| Err(anyhow::anyhow!("simulated sync connection failure")),
            )
            .expect_err("persist failure must abort the connection candidate");

        assert!(format!("{error:#}").contains("simulated sync connection failure"));
        assert_eq!(serde_json::to_value(&store.cache).unwrap(), cache_before);
        assert_eq!(fs::read(&path).unwrap(), disk_before);
        fs::remove_dir_all(root).unwrap();
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
    fn config_encryption_v2_roundtrip_rejects_wrong_master_key() {
        let mut config = ConfigFile::default();
        config.sessions.push(Session::password(
            "example.test".to_string(),
            22,
            "admin".to_string(),
            "do-not-store-in-plaintext".to_string(),
        ));
        let key = crate::session::config_key::MasterKey::from_secret(vec![17; 32]).unwrap();
        let wrong_key = crate::session::config_key::MasterKey::from_secret(vec![19; 32]).unwrap();

        let encrypted = encrypt_config_v2(&config, &key).expect("encrypt version 2 config");
        let envelope: serde_json::Value = serde_json::from_slice(&encrypted).unwrap();

        assert_eq!(config_format_version(&encrypted).unwrap(), 2);
        assert_eq!(envelope["kdf"], "os-keyring");
        assert_eq!(envelope["cipher"], "xchacha20poly1305");
        assert!(envelope.get("salt").is_none());
        assert!(!String::from_utf8_lossy(&encrypted).contains("do-not-store-in-plaintext"));

        let decrypted = decrypt_config_v2(&encrypted, &key).expect("decrypt version 2 config");
        assert_eq!(decrypted.sessions[0].password, "do-not-store-in-plaintext");
        assert!(decrypt_config_v2(&encrypted, &wrong_key).is_err());
    }

    #[test]
    fn config_encryption_v1_remains_available_for_migration() {
        let config = ConfigFile::default();
        let hardware_id = "legacy-hardware-id";

        let encrypted =
            encrypt_config_v1(&config, hardware_id).expect("encrypt legacy version 1 config");

        assert_eq!(config_format_version(&encrypted).unwrap(), 1);
        let decrypted = decrypt_config_v1(&encrypted, hardware_id)
            .expect("decrypt legacy version 1 config during migration");
        assert_eq!(decrypted.terminal_font_family, config.terminal_font_family);
        assert!(decrypt_config_v1(&encrypted, "different-hardware-id").is_err());
    }

    #[test]
    fn config_store_encryption_v2_missing_key_preserves_file() {
        let path = std::env::temp_dir().join(format!(
            "jshell-config-v2-missing-key-{}.json",
            Uuid::new_v4()
        ));
        let legacy_path = path.with_extension("legacy.json");
        let encryption_key = MasterKey::from_secret(vec![43; 32]).unwrap();
        let original = encrypt_config_v2(&ConfigFile::default(), &encryption_key).unwrap();
        fs::write(&path, &original).unwrap();
        let provider = TestKeyProvider::default();

        let error = ConfigStore::load_with_key_provider(
            path.clone(),
            legacy_path,
            &provider,
            "legacy-hardware-id",
        )
        .err()
        .expect("version 2 config without its key must fail closed");

        assert!(format!("{error:#}").contains("missing"));
        assert_eq!(provider.create_calls.get(), 0);
        assert_eq!(fs::read(&path).unwrap(), original);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_store_encryption_first_run_creates_key_and_saves_v2() {
        let path =
            std::env::temp_dir().join(format!("jshell-config-first-run-{}.json", Uuid::new_v4()));
        let provider = TestKeyProvider::default();
        let mut store = ConfigStore::load_with_key_provider(
            path.clone(),
            path.with_extension("legacy.json"),
            &provider,
            "legacy-hardware-id",
        )
        .unwrap();
        store.cache.ui_font_size = 18.0;

        store.save().unwrap();

        assert_eq!(provider.create_calls.get(), 1);
        let raw = fs::read(&path).unwrap();
        assert_eq!(config_format_version(&raw).unwrap(), 2);
        let key = provider.key.borrow().clone().unwrap();
        assert_eq!(decrypt_config_v2(&raw, &key).unwrap().ui_font_size, 18.0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_store_encryption_v2_reuses_existing_key() {
        let path = std::env::temp_dir().join(format!(
            "jshell-config-v2-existing-key-{}.json",
            Uuid::new_v4()
        ));
        let key = MasterKey::from_secret(vec![53; 32]).unwrap();
        let config = ConfigFile {
            ui_font_size: 17.0,
            ..ConfigFile::default()
        };
        fs::write(&path, encrypt_config_v2(&config, &key).unwrap()).unwrap();
        let provider = TestKeyProvider {
            key: RefCell::new(Some(key)),
            ..TestKeyProvider::default()
        };

        let store = ConfigStore::load_with_key_provider(
            path.clone(),
            path.with_extension("legacy.json"),
            &provider,
            "wrong-legacy-hardware-id",
        )
        .unwrap();

        assert_eq!(store.ui_font_size(), 17.0);
        assert_eq!(provider.create_calls.get(), 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_store_encryption_v1_migrates_once_and_stops_using_hardware_id() {
        let path = std::env::temp_dir().join(format!(
            "jshell-config-v1-migration-{}.json",
            Uuid::new_v4()
        ));
        let mut config = ConfigFile::default();
        config.sessions.push(Session::password(
            "migration.example".to_string(),
            22,
            "root".to_string(),
            "legacy-password".to_string(),
        ));
        fs::write(
            &path,
            encrypt_config_v1(&config, "original-hardware-id").unwrap(),
        )
        .unwrap();
        let provider = TestKeyProvider::default();

        let migrated = ConfigStore::load_with_key_provider(
            path.clone(),
            path.with_extension("legacy.json"),
            &provider,
            "original-hardware-id",
        )
        .unwrap();

        assert_eq!(migrated.sessions()[0].password, "legacy-password");
        assert_eq!(config_format_version(&fs::read(&path).unwrap()).unwrap(), 2);
        assert_eq!(provider.create_calls.get(), 1);

        let reopened = ConfigStore::load_with_key_provider(
            path.clone(),
            path.with_extension("legacy.json"),
            &provider,
            "different-hardware-id",
        )
        .unwrap();
        assert_eq!(reopened.sessions()[0].host, "migration.example");
        assert_eq!(provider.create_calls.get(), 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_store_encryption_corrupted_v2_preserves_file() {
        let path = std::env::temp_dir().join(format!(
            "jshell-config-v2-corrupted-{}.json",
            Uuid::new_v4()
        ));
        let key = MasterKey::from_secret(vec![59; 32]).unwrap();
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&encrypt_config_v2(&ConfigFile::default(), &key).unwrap())
                .unwrap();
        envelope["payload"] = serde_json::Value::String("AAAA".to_string());
        let corrupted = serde_json::to_vec_pretty(&envelope).unwrap();
        fs::write(&path, &corrupted).unwrap();
        let provider = TestKeyProvider {
            key: RefCell::new(Some(key)),
            ..TestKeyProvider::default()
        };

        assert!(
            ConfigStore::load_with_key_provider(
                path.clone(),
                path.with_extension("legacy.json"),
                &provider,
                "legacy-hardware-id",
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), corrupted);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_store_encryption_recognized_plaintext_migrates_but_unknown_json_does_not() {
        let path = std::env::temp_dir().join(format!(
            "jshell-config-plaintext-migration-{}.json",
            Uuid::new_v4()
        ));
        fs::write(&path, br#"{"sessions":[],"ui_font_size":18.0}"#).unwrap();
        let provider = TestKeyProvider::default();

        let migrated = ConfigStore::load_with_key_provider(
            path.clone(),
            path.with_extension("legacy.json"),
            &provider,
            "legacy-hardware-id",
        )
        .unwrap();

        assert_eq!(migrated.ui_font_size(), 18.0);
        assert_eq!(config_format_version(&fs::read(&path).unwrap()).unwrap(), 2);

        let unknown_path = path.with_extension("unknown.json");
        let unknown = br#"{"unrecognized":true}"#;
        fs::write(&unknown_path, unknown).unwrap();
        let unknown_provider = TestKeyProvider::default();
        assert!(
            ConfigStore::load_with_key_provider(
                unknown_path.clone(),
                unknown_path.with_extension("legacy.json"),
                &unknown_provider,
                "legacy-hardware-id",
            )
            .is_err()
        );
        assert_eq!(unknown_provider.create_calls.get(), 0);
        assert_eq!(fs::read(&unknown_path).unwrap(), unknown);

        let _ = fs::remove_file(path);
        let _ = fs::remove_file(unknown_path);
    }

    #[test]
    fn security_migration_persist_failure_is_returned_and_preserves_source_bytes() {
        let root = std::env::temp_dir().join(format!(
            "jshell-security-migration-failure-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        let original = br#"{"sessions":[],"ui_font_size":18.0}"#;
        fs::write(&path, original).unwrap();
        let provider = TestKeyProvider::default();

        let error = ConfigStore::load_with_key_provider_and_persist(
            path.clone(),
            root.join("legacy.json"),
            &provider,
            "legacy-hardware-id",
            |_| Err(anyhow::anyhow!("simulated migration persistence failure")),
        )
        .err()
        .expect("security migration persistence failure must abort loading");

        assert!(format!("{error:#}").contains("simulated migration persistence failure"));
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_legacy_location_migration_removes_the_old_source_file() {
        let root = std::env::temp_dir().join(format!(
            "jshell-legacy-location-migration-{}",
            Uuid::new_v4()
        ));
        let new_path = root.join("jshell").join("sessions.json");
        let legacy_path = root.join("ashell").join("sessions.json");
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(&legacy_path, br#"{"sessions":[],"ui_font_size":17.0}"#).unwrap();
        let provider = TestKeyProvider::default();

        let store = ConfigStore::load_with_key_provider(
            new_path.clone(),
            legacy_path.clone(),
            &provider,
            "legacy-hardware-id",
        )
        .unwrap();

        assert_eq!(store.ui_font_size(), 17.0);
        assert_eq!(
            config_format_version(&fs::read(&new_path).unwrap()).unwrap(),
            2
        );
        assert!(!legacy_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_apply_v2_replaces_portable_fields_and_preserves_local_only_state() {
        let root =
            std::env::temp_dir().join(format!("jshell-portable-apply-v2-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        let master_key = MasterKey::from_secret(vec![67; 32]).unwrap();
        let local_config = portable_apply_local_config();
        let local_only_before = portable_apply_local_only_snapshot(&local_config);
        let mut store = ConfigStore {
            path: path.clone(),
            cache: local_config,
            master_key: master_key.clone(),
        };
        store.save().unwrap();

        let remote_portable = crate::sync::PortableConfigV2::from(&portable_apply_remote_config());
        let payload = crate::sync::DecodedSyncPayload::V2(Box::new(
            crate::sync::SyncPayloadV2::new(remote_portable.clone()),
        ));
        let target = crate::sync::SyncTargetId::for_r2(
            "download-account",
            "download-bucket",
            "download-key.json",
        );

        store
            .apply_decoded_sync_payload(&payload, &target, Some("download-etag".to_string()))
            .unwrap();

        assert_eq!(
            serde_json::to_value(store.portable_config()).unwrap(),
            serde_json::to_value(&remote_portable).unwrap()
        );
        assert_eq!(
            portable_apply_local_only_snapshot(&store.cache),
            local_only_before
        );
        assert_eq!(store.cache.sync_etag.as_deref(), Some("download-etag"));
        assert_eq!(store.cache.sync_etag_target, target.to_string());

        let persisted = decrypt_config_v2(&fs::read(&path).unwrap(), &master_key).unwrap();
        assert_eq!(
            serde_json::to_value(crate::sync::PortableConfigV2::from(&persisted)).unwrap(),
            serde_json::to_value(&remote_portable).unwrap()
        );
        assert_eq!(
            portable_apply_local_only_snapshot(&persisted),
            local_only_before
        );
        assert_eq!(persisted.sync_etag.as_deref(), Some("download-etag"));
        assert_eq!(persisted.sync_etag_target, target.to_string());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_apply_v1_replaces_only_sessions_and_normalizes_local_folders() {
        let mut store = ConfigStore::in_memory();
        store.cache = ConfigFile {
            follow_system_theme: false,
            theme_mode: "local-theme".to_string(),
            locale: "local-locale".to_string(),
            terminal_font_size: 19.0,
            ui_font_size: 13.0,
            key_bindings: std::collections::HashMap::from([(
                "Copy".to_string(),
                "local-copy".to_string(),
            )]),
            use_proxy: true,
            global_proxy_host: "local-proxy.test".to_string(),
            sessions: vec![portable_apply_session("old-session", "old.local.test")],
            session_folders: vec![SessionFolder {
                id: "local-folder".to_string(),
                name: "Local Folder".to_string(),
                session_ids: vec![
                    "new-b".to_string(),
                    "unknown".to_string(),
                    "new-b".to_string(),
                    "new-a".to_string(),
                    "new-a".to_string(),
                ],
            }],
            ..ConfigFile::default()
        };
        let preferences_before = portable_apply_preferences_snapshot(&store.cache);
        let folders_before = store.cache.session_folders.clone();
        let payload = crate::sync::DecodedSyncPayload::LegacyV1(crate::sync::LegacySyncPayloadV1 {
            schema_version: 1,
            revision: "legacy-revision".to_string(),
            updated_at: "2026-08-01T00:00:00Z".to_string(),
            device_id: "legacy-device".to_string(),
            sessions: vec![
                portable_apply_session("new-a", "a.new.test"),
                portable_apply_session("new-b", "b.new.test"),
            ],
        });
        let target =
            crate::sync::SyncTargetId::for_webdav("https://dav.test/config", "legacy-user");

        store
            .apply_decoded_sync_payload(&payload, &target, Some("legacy-etag".to_string()))
            .unwrap();

        assert_eq!(
            store
                .sessions()
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new-a", "new-b"]
        );
        assert_eq!(
            portable_apply_preferences_snapshot(&store.cache),
            preferences_before
        );
        assert_eq!(store.cache.session_folders[0].id, folders_before[0].id);
        assert_eq!(store.cache.session_folders[0].name, folders_before[0].name);
        assert_eq!(
            store.cache.session_folders[0].session_ids,
            vec!["new-b".to_string(), "new-a".to_string()]
        );
    }

    #[test]
    fn portable_apply_persist_failure_preserves_cache_disk_and_etag_binding() {
        let root =
            std::env::temp_dir().join(format!("jshell-portable-apply-failure-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        let master_key = MasterKey::from_secret(vec![71; 32]).unwrap();
        let mut store = ConfigStore {
            path: path.clone(),
            cache: portable_apply_local_config(),
            master_key,
        };
        store.save().unwrap();
        let cache_before = serde_json::to_value(&store.cache).unwrap();
        let disk_before = fs::read(&path).unwrap();
        let etag_before = store.cache.sync_etag.clone();
        let target_before = store.cache.sync_etag_target.clone();
        let remote_portable = crate::sync::PortableConfigV2::from(&portable_apply_remote_config());
        let payload = crate::sync::DecodedSyncPayload::V2(Box::new(
            crate::sync::SyncPayloadV2::new(remote_portable),
        ));
        let target = crate::sync::SyncTargetId::for_s3(
            "https://download-s3.test",
            "download-region",
            "download-bucket",
            "download-key.json",
        );

        let error = store
            .apply_decoded_sync_payload_with_persist(
                &payload,
                &target,
                Some("new-etag".to_string()),
                |_path, _bytes| Err(anyhow::anyhow!("simulated portable persist failure")),
            )
            .expect_err("persist failure must abort the candidate application");

        assert!(format!("{error:#}").contains("simulated portable persist failure"));
        assert_eq!(serde_json::to_value(&store.cache).unwrap(), cache_before);
        assert_eq!(fs::read(&path).unwrap(), disk_before);
        assert_eq!(store.cache.sync_etag, etag_before);
        assert_eq!(store.cache.sync_etag_target, target_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_apply_with_connection_persist_failure_preserves_cache_and_disk() {
        let root = std::env::temp_dir().join(format!(
            "jshell-portable-connection-apply-failure-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        let master_key = MasterKey::from_secret(vec![79; 32]).unwrap();
        let mut store = ConfigStore {
            path: path.clone(),
            cache: portable_apply_local_config(),
            master_key,
        };
        store.save().unwrap();
        let cache_before = serde_json::to_value(&store.cache).unwrap();
        let disk_before = fs::read(&path).unwrap();
        let payload =
            crate::sync::DecodedSyncPayload::V2(Box::new(crate::sync::SyncPayloadV2::new(
                crate::sync::PortableConfigV2::from(&portable_apply_remote_config()),
            )));
        let connection = SyncConnectionSnapshot::r2(
            "0123456789abcdef0123456789abcdef".to_string(),
            "download-bucket".to_string(),
            "download-key.json".to_string(),
            "download-access-key".to_string(),
            true,
        );

        let error = store
            .apply_decoded_sync_payload_with_connection_with_persist(
                &payload,
                &connection,
                Some("download-etag".to_string()),
                |_path, _bytes| Err(anyhow::anyhow!("simulated combined persist failure")),
            )
            .expect_err("combined candidate persistence must be atomic");

        assert!(format!("{error:#}").contains("simulated combined persist failure"));
        assert_eq!(serde_json::to_value(&store.cache).unwrap(), cache_before);
        assert_eq!(fs::read(&path).unwrap(), disk_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_config_write_replaces_existing_file() {
        let root =
            std::env::temp_dir().join(format!("jshell-atomic-config-write-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        fs::write(&path, b"previous complete config").unwrap();

        atomic_write_config(&path, b"replacement complete config").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"replacement complete config");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_config_write_keeps_existing_file_when_replace_fails() {
        let root =
            std::env::temp_dir().join(format!("jshell-atomic-config-write-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        let original = b"previous complete config";
        fs::write(&path, original).unwrap();

        let error =
            atomic_write_config_with(&path, b"replacement complete config", |_staged, _target| {
                Err(anyhow::anyhow!("simulated atomic replace failure"))
            })
            .expect_err("replace failure must be returned");

        assert!(format!("{error:#}").contains("simulated atomic replace failure"));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_config_write_treats_post_commit_sync_failure_as_committed() {
        let root =
            std::env::temp_dir().join(format!("jshell-atomic-config-write-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sessions.json");
        fs::write(&path, b"previous complete config").unwrap();

        let result = atomic_write_config_with_post_commit(
            &path,
            b"replacement complete config",
            |staged, target| {
                staged
                    .persist(target)
                    .map_err(|err| anyhow::Error::new(err.error))
            },
            |_persisted, _parent| {
                Err(anyhow::anyhow!(
                    "simulated post-commit sync failure with secret-sentinel"
                ))
            },
        );

        assert!(result.is_ok());
        assert_eq!(fs::read(&path).unwrap(), b"replacement complete config");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_config_encryption_roundtrip() {
        let config = ConfigFile::default();
        let password = "test-password-123";
        let encrypted = encrypt_config_v1(&config, password).unwrap();

        // Ensure it doesn't contain plain text fields of default config
        let encrypted_str = String::from_utf8_lossy(&encrypted);
        assert!(!encrypted_str.contains("Noto Sans CJK SC"));
        assert!(encrypted_str.contains("argon2id"));

        let decrypted = decrypt_config_v1(&encrypted, password).unwrap();
        assert_eq!(decrypted.terminal_font_family, config.terminal_font_family);

        // Decrypt with wrong password should fail
        assert!(decrypt_config_v1(&encrypted, "wrong-password").is_err());
    }

    #[test]
    fn test_save_merged_preferences() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("ashell-test-config-{}.json", Uuid::new_v4()));
        let mut store = ConfigStore {
            path: path.clone(),
            cache: ConfigFile::default(),
            master_key: MasterKey::from_secret(vec![47; 32]).unwrap(),
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

        let local_config = ConfigFile {
            ui_font_size: 18.0,
            terminal_font_size: 20.0,
            show_hidden_files: true,
            sftp_cwd_sync_mode: SftpCwdSyncMode::Realtime,
            key_bindings: std::collections::HashMap::from([(
                "OpenSettings".to_string(),
                "ctrl-shift-,".to_string(),
            )]),
            use_proxy: true,
            read_env_proxy: true,
            global_proxy_type: "https".to_string(),
            global_proxy_host: "proxy.example.test".to_string(),
            global_proxy_port: Some(8443),
            global_proxy_user: "proxy-user".to_string(),
            global_proxy_password: "proxy-password".to_string(),
            sync_remember_encryption_password: true,
            ..ConfigFile::default()
        };

        store.save_merged_preferences(local_config).unwrap();

        let loaded_bytes = fs::read(&path).unwrap();
        let decrypted = decrypt_config_v2(&loaded_bytes, &store.master_key).unwrap();

        assert_eq!(decrypted.ui_font_size, 18.0);
        assert_eq!(decrypted.terminal_font_size, 20.0);
        assert!(decrypted.show_hidden_files);
        assert_eq!(decrypted.sftp_cwd_sync_mode, SftpCwdSyncMode::Realtime);
        assert_eq!(
            decrypted
                .key_bindings
                .get("OpenSettings")
                .map(String::as_str),
            Some("ctrl-shift-,")
        );
        assert!(decrypted.use_proxy);
        assert!(decrypted.read_env_proxy);
        assert_eq!(decrypted.global_proxy_type, "https");
        assert_eq!(decrypted.global_proxy_host, "proxy.example.test");
        assert_eq!(decrypted.global_proxy_port, Some(8443));
        assert_eq!(decrypted.global_proxy_user, "proxy-user");
        assert_eq!(decrypted.global_proxy_password, "proxy-password");
        assert!(decrypted.sync_remember_encryption_password);

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
