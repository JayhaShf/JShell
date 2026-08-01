use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::session::config::{AuthMethod, ConfigFile, CursorStyle, Session, SessionFolder};

const LEGACY_SYNC_SCHEMA_VERSION: u32 = 1;
const SYNC_SCHEMA_VERSION: u32 = 2;
const PORTABLE_PREFERENCE_CATEGORIES: &[&str] = &["preferences", "key_bindings", "proxy"];
const PORTABLE_PREFERENCE_CATEGORY_COUNT: usize = PORTABLE_PREFERENCE_CATEGORIES.len();

#[derive(Clone, Serialize)]
pub struct LegacySyncPayloadV1 {
    pub schema_version: u32,
    pub revision: String,
    pub updated_at: String,
    pub device_id: String,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncPayloadV2 {
    pub schema_version: u32,
    pub revision: String,
    pub updated_at: String,
    pub portable_config: PortableConfigV2,
}

impl SyncPayloadV2 {
    pub fn new(portable_config: PortableConfigV2) -> Self {
        Self {
            schema_version: SYNC_SCHEMA_VERSION,
            revision: Uuid::new_v4().to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            portable_config,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct PortableConfigV2 {
    pub sessions: Vec<Session>,
    pub session_folders: Vec<SessionFolder>,
    pub preferences: PortablePreferencesV2,
    pub key_bindings: HashMap<String, String>,
    pub proxy: PortableProxyV2,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySyncPayloadV1Dto {
    schema_version: u32,
    revision: String,
    updated_at: String,
    device_id: String,
    sessions: Vec<PortableSessionV2>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableConfigV2Dto {
    sessions: Vec<PortableSessionV2>,
    session_folders: Vec<PortableSessionFolderV2>,
    preferences: PortablePreferencesV2,
    key_bindings: HashMap<String, String>,
    proxy: PortableProxyV2,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableSessionV2 {
    id: String,
    name: String,
    host: String,
    port: u16,
    user: String,
    auth: AuthMethod,
    #[serde(default)]
    password: String,
    #[serde(default)]
    private_key_path: String,
    #[serde(default)]
    private_key_inline: String,
    #[serde(default)]
    passphrase: String,
    #[serde(default)]
    last_used: Option<String>,
    #[serde(default = "default_portable_session_proxy_type")]
    proxy_type: String,
    #[serde(default)]
    proxy_host: String,
    #[serde(default)]
    proxy_port: Option<u16>,
    #[serde(default)]
    proxy_user: String,
    #[serde(default)]
    proxy_password: String,
    #[serde(default = "default_portable_session_protocol")]
    protocol: String,
    #[serde(default = "default_portable_session_baud_rate")]
    baud_rate: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableSessionFolderV2 {
    id: String,
    name: String,
    #[serde(default)]
    session_ids: Vec<String>,
}

fn default_portable_session_proxy_type() -> String {
    "none".to_string()
}

fn default_portable_session_protocol() -> String {
    "ssh".to_string()
}

fn default_portable_session_baud_rate() -> u32 {
    115_200
}

impl<'de> Deserialize<'de> for LegacySyncPayloadV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(LegacySyncPayloadV1Dto::deserialize(deserializer)?.into())
    }
}

impl From<LegacySyncPayloadV1Dto> for LegacySyncPayloadV1 {
    fn from(payload: LegacySyncPayloadV1Dto) -> Self {
        Self {
            schema_version: payload.schema_version,
            revision: payload.revision,
            updated_at: payload.updated_at,
            device_id: payload.device_id,
            sessions: payload.sessions.into_iter().map(Session::from).collect(),
        }
    }
}

impl<'de> Deserialize<'de> for PortableConfigV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(PortableConfigV2Dto::deserialize(deserializer)?.into())
    }
}

impl From<PortableConfigV2Dto> for PortableConfigV2 {
    fn from(config: PortableConfigV2Dto) -> Self {
        Self {
            sessions: config.sessions.into_iter().map(Session::from).collect(),
            session_folders: config
                .session_folders
                .into_iter()
                .map(SessionFolder::from)
                .collect(),
            preferences: config.preferences,
            key_bindings: config.key_bindings,
            proxy: config.proxy,
        }
    }
}

impl From<PortableSessionV2> for Session {
    fn from(session: PortableSessionV2) -> Self {
        Self {
            id: session.id,
            name: session.name,
            host: session.host,
            port: session.port,
            user: session.user,
            auth: session.auth,
            password: session.password,
            private_key_path: session.private_key_path,
            private_key_inline: session.private_key_inline,
            passphrase: session.passphrase,
            last_used: session.last_used,
            proxy_type: session.proxy_type,
            proxy_host: session.proxy_host,
            proxy_port: session.proxy_port,
            proxy_user: session.proxy_user,
            proxy_password: session.proxy_password,
            protocol: session.protocol,
            baud_rate: session.baud_rate,
        }
    }
}

impl From<PortableSessionFolderV2> for SessionFolder {
    fn from(folder: PortableSessionFolderV2) -> Self {
        Self {
            id: folder.id,
            name: folder.name,
            session_ids: folder.session_ids,
        }
    }
}

impl PortableConfigV2 {
    pub const fn preference_category_count() -> usize {
        PORTABLE_PREFERENCE_CATEGORY_COUNT
    }
}

impl From<&ConfigFile> for PortableConfigV2 {
    fn from(config: &ConfigFile) -> Self {
        Self {
            sessions: config.sessions.clone(),
            session_folders: config.session_folders.clone(),
            preferences: PortablePreferencesV2 {
                follow_system_theme: config.follow_system_theme,
                theme_mode: config.theme_mode.clone(),
                light_theme_name: config.light_theme_name.clone(),
                dark_theme_name: config.dark_theme_name.clone(),
                locale: config.locale.clone(),
                terminal_font_size: config.terminal_font_size,
                ui_font_size: config.ui_font_size,
                right_click_copy_paste: config.right_click_copy_paste,
                keyword_highlight: config.keyword_highlight,
                editor_soft_wrap: config.editor_soft_wrap,
                history_completion_plugin_enabled: config.history_completion_plugin_enabled,
                ui_font_family: config.ui_font_family.clone(),
                terminal_font_family: config.terminal_font_family.clone(),
                cursor_style: config.cursor_style,
                show_hidden_files: config.show_hidden_files,
                lock_layout: config.lock_layout,
                monitoring_position: config.monitoring_position.clone(),
            },
            key_bindings: config.key_bindings.clone(),
            proxy: PortableProxyV2 {
                use_proxy: config.use_proxy,
                read_env_proxy: config.read_env_proxy,
                global_proxy_type: config.global_proxy_type.clone(),
                global_proxy_host: config.global_proxy_host.clone(),
                global_proxy_port: config.global_proxy_port,
                global_proxy_user: config.global_proxy_user.clone(),
                global_proxy_password: config.global_proxy_password.clone(),
            },
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePreferencesV2 {
    pub follow_system_theme: bool,
    pub theme_mode: String,
    pub light_theme_name: String,
    pub dark_theme_name: String,
    pub locale: String,
    pub terminal_font_size: f32,
    pub ui_font_size: f32,
    pub right_click_copy_paste: bool,
    pub keyword_highlight: bool,
    pub editor_soft_wrap: bool,
    pub history_completion_plugin_enabled: bool,
    pub ui_font_family: String,
    pub terminal_font_family: String,
    pub cursor_style: CursorStyle,
    pub show_hidden_files: bool,
    pub lock_layout: bool,
    pub monitoring_position: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableProxyV2 {
    pub use_proxy: bool,
    pub read_env_proxy: bool,
    pub global_proxy_type: String,
    pub global_proxy_host: String,
    pub global_proxy_port: Option<u16>,
    pub global_proxy_user: String,
    pub global_proxy_password: String,
}

#[derive(Clone)]
pub enum DecodedSyncPayload {
    LegacyV1(LegacySyncPayloadV1),
    V2(Box<SyncPayloadV2>),
}

impl fmt::Debug for DecodedSyncPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            Self::LegacyV1(_) => "LegacyV1",
            Self::V2(_) => "V2",
        };
        formatter
            .debug_struct("DecodedSyncPayload")
            .field("variant", &variant)
            .field("preview", &self.preview())
            .finish()
    }
}

impl DecodedSyncPayload {
    pub fn preview(&self) -> SyncPreview {
        let (schema_version, updated_at, sessions, folder_count, preference_category_count) =
            match self {
                Self::LegacyV1(payload) => (
                    payload.schema_version,
                    payload.updated_at.clone(),
                    payload.sessions.as_slice(),
                    0,
                    0,
                ),
                Self::V2(payload) => (
                    payload.schema_version,
                    payload.updated_at.clone(),
                    payload.portable_config.sessions.as_slice(),
                    payload.portable_config.session_folders.len(),
                    PortableConfigV2::preference_category_count(),
                ),
            };
        SyncPreview {
            schema_version,
            updated_at,
            session_count: sessions.len(),
            folder_count,
            preference_category_count,
            external_private_key_path_count: sessions
                .iter()
                .filter(|session| !session.private_key_path.is_empty())
                .count(),
        }
    }

    pub fn preview_against(&self, local_config: &ConfigFile) -> SyncPreview {
        let mut preview = self.preview();
        let Self::V2(payload) = self else {
            return preview;
        };
        let local = PortableConfigV2::from(local_config);
        preview.preference_category_count =
            usize::from(payload.portable_config.preferences != local.preferences)
                + usize::from(payload.portable_config.key_bindings != local.key_bindings)
                + usize::from(payload.portable_config.proxy != local.proxy);
        preview
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncPreview {
    pub schema_version: u32,
    pub updated_at: String,
    pub session_count: usize,
    pub folder_count: usize,
    pub preference_category_count: usize,
    pub external_private_key_path_count: usize,
}

#[derive(Debug, Error)]
pub(super) enum PayloadError {
    #[error("invalid synchronized configuration JSON")]
    InvalidJson,
    #[error("synchronized configuration has no numeric schema version")]
    InvalidSchemaVersion,
    #[error("unsupported synchronized configuration schema version {0}")]
    UnsupportedSchemaVersion(u64),
    #[error("invalid synchronized configuration schema {schema_version}")]
    InvalidPayload { schema_version: u32 },
    #[error("synchronized configuration contains duplicate session IDs")]
    DuplicateSessionId,
    #[error("synchronized configuration contains duplicate folder IDs")]
    DuplicateFolderId,
    #[error("synchronized configuration folder references an unknown session")]
    UnknownSessionReference,
    #[error("synchronized configuration contains an invalid {0} field")]
    InvalidField(&'static str),
}

pub(super) fn decode_payload(raw: &[u8]) -> Result<DecodedSyncPayload, PayloadError> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| PayloadError::InvalidJson)?;
    let schema_version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or(PayloadError::InvalidSchemaVersion)?;

    match schema_version {
        version if version == u64::from(LEGACY_SYNC_SCHEMA_VERSION) => {
            let payload: LegacySyncPayloadV1 =
                serde_json::from_value(value).map_err(|_| PayloadError::InvalidPayload {
                    schema_version: LEGACY_SYNC_SCHEMA_VERSION,
                })?;
            validate_sessions(&payload.sessions)?;
            Ok(DecodedSyncPayload::LegacyV1(payload))
        }
        version if version == u64::from(SYNC_SCHEMA_VERSION) => {
            let payload: SyncPayloadV2 =
                serde_json::from_value(value).map_err(|_| PayloadError::InvalidPayload {
                    schema_version: SYNC_SCHEMA_VERSION,
                })?;
            validate_portable_config(&payload.portable_config)?;
            Ok(DecodedSyncPayload::V2(Box::new(payload)))
        }
        version => Err(PayloadError::UnsupportedSchemaVersion(version)),
    }
}

fn validate_sessions(sessions: &[Session]) -> Result<HashSet<&str>, PayloadError> {
    let mut session_ids = HashSet::with_capacity(sessions.len());
    for session in sessions {
        if session.id.trim().is_empty() {
            return Err(PayloadError::InvalidField("session.id"));
        }
        if !session_ids.insert(session.id.as_str()) {
            return Err(PayloadError::DuplicateSessionId);
        }
        match session.protocol.as_str() {
            "ssh" => {
                if session.host.trim().is_empty() {
                    return Err(PayloadError::InvalidField("session.host"));
                }
                if session.port == 0 {
                    return Err(PayloadError::InvalidField("session.port"));
                }
                if session.user.trim().is_empty() {
                    return Err(PayloadError::InvalidField("session.user"));
                }
            }
            "serial" => {
                if session.host.trim().is_empty() {
                    return Err(PayloadError::InvalidField("session.host"));
                }
                if session.baud_rate == 0 {
                    return Err(PayloadError::InvalidField("session.baud_rate"));
                }
            }
            _ => return Err(PayloadError::InvalidField("session.protocol")),
        }
        match session.proxy_type.as_str() {
            "" | "none" | "direct" => {}
            "socks5" | "socks5h" | "http" | "https" => {
                if session.proxy_host.trim().is_empty() {
                    return Err(PayloadError::InvalidField("session.proxy_host"));
                }
                if session.proxy_port.is_none_or(|port| port == 0) {
                    return Err(PayloadError::InvalidField("session.proxy_port"));
                }
            }
            _ => return Err(PayloadError::InvalidField("session.proxy_type")),
        }
    }
    Ok(session_ids)
}

fn key_binding_syntax_is_valid(keystrokes: &str) -> bool {
    // KeyBinding::load parses each whitespace-separated stroke with Keystroke::parse.
    !keystrokes.trim().is_empty()
        && keystrokes
            .split_whitespace()
            .all(|keystroke| gpui::Keystroke::parse(keystroke).is_ok())
}

fn validate_portable_config(config: &PortableConfigV2) -> Result<(), PayloadError> {
    let session_ids = validate_sessions(&config.sessions)?;
    let mut folder_ids = HashSet::with_capacity(config.session_folders.len());
    for folder in &config.session_folders {
        if folder.id.trim().is_empty() {
            return Err(PayloadError::InvalidField("session_folder.id"));
        }
        if !folder_ids.insert(folder.id.as_str()) {
            return Err(PayloadError::DuplicateFolderId);
        }
        if folder
            .session_ids
            .iter()
            .any(|session_id| !session_ids.contains(session_id.as_str()))
        {
            return Err(PayloadError::UnknownSessionReference);
        }
    }

    for (action_id, keystroke) in &config.key_bindings {
        if crate::app::keybinding_recorder::default_keystroke(action_id).is_none() {
            return Err(PayloadError::InvalidField("key_bindings.action"));
        }
        if !keystroke.is_empty() && keystroke != "none" && !key_binding_syntax_is_valid(keystroke) {
            return Err(PayloadError::InvalidField("key_bindings.keystroke"));
        }
    }

    let preferences = &config.preferences;
    if !matches!(preferences.theme_mode.as_str(), "" | "light" | "dark") {
        return Err(PayloadError::InvalidField("theme_mode"));
    }
    if !matches!(preferences.locale.as_str(), "system" | "en" | "zh-CN") {
        return Err(PayloadError::InvalidField("locale"));
    }
    if !preferences.terminal_font_size.is_finite()
        || !(10.0..=24.0).contains(&preferences.terminal_font_size)
    {
        return Err(PayloadError::InvalidField("terminal_font_size"));
    }
    if !preferences.ui_font_size.is_finite()
        || !(crate::session::config::MIN_UI_FONT_SIZE..=crate::session::config::MAX_UI_FONT_SIZE)
            .contains(&preferences.ui_font_size)
    {
        return Err(PayloadError::InvalidField("ui_font_size"));
    }
    if !matches!(
        preferences.monitoring_position.as_str(),
        "Bottom" | "Sidebar" | "Hidden"
    ) {
        return Err(PayloadError::InvalidField("monitoring_position"));
    }

    let proxy = &config.proxy;
    if !matches!(
        proxy.global_proxy_type.as_str(),
        "socks5" | "socks5h" | "http" | "https"
    ) {
        return Err(PayloadError::InvalidField("global_proxy_type"));
    }
    if proxy.use_proxy {
        if proxy.global_proxy_host.trim().is_empty() {
            return Err(PayloadError::InvalidField("global_proxy_host"));
        }
        if proxy.global_proxy_port.is_none_or(|port| port == 0) {
            return Err(PayloadError::InvalidField("global_proxy_port"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use serde_json::Value;

    use super::{
        DecodedSyncPayload, LegacySyncPayloadV1, PayloadError, PortableConfigV2, SyncPayloadV2,
        decode_payload,
    };
    use crate::session::config::{
        ConfigFile, CursorStyle, SavedWindowBounds, Session, SessionFolder,
    };
    use crate::sync::SyncError;

    const TEST_SYNC_PASSWORD: &str = "correct horse battery staple";

    fn test_session(id: &str) -> Session {
        let mut session = Session::password(
            "example.test".to_string(),
            22,
            "root".to_string(),
            "session-password-sentinel".to_string(),
        );
        session.id = id.to_string();
        session.name = format!("Session {id}");
        session.passphrase = "session-passphrase-sentinel".to_string();
        session.private_key_inline = "inline-private-key-sentinel".to_string();
        session.proxy_user = "session-proxy-user-sentinel".to_string();
        session.proxy_password = "session-proxy-password-sentinel".to_string();
        session
    }

    fn populated_config() -> ConfigFile {
        ConfigFile {
            follow_system_theme: false,
            theme_mode: "dark".to_string(),
            light_theme_name: "light-theme".to_string(),
            dark_theme_name: "dark-theme".to_string(),
            locale: "zh-CN".to_string(),
            terminal_font_size: 17.0,
            ui_font_size: 15.0,
            right_click_copy_paste: true,
            keyword_highlight: true,
            editor_soft_wrap: true,
            history_completion_plugin_enabled: false,
            ui_font_family: "UI Font".to_string(),
            terminal_font_family: "Terminal Font".to_string(),
            cursor_style: CursorStyle::BeamBlink,
            sessions: vec![test_session("session-1")],
            session_folders: vec![SessionFolder {
                id: "folder-1".to_string(),
                name: "Production".to_string(),
                session_ids: vec!["session-1".to_string()],
            }],
            show_hidden_files: true,
            lock_layout: true,
            monitoring_position: "Bottom".to_string(),
            key_bindings: HashMap::from([
                ("Copy".to_string(), "ctrl-c".to_string()),
                ("Paste".to_string(), "ctrl-v".to_string()),
            ]),
            use_proxy: true,
            read_env_proxy: false,
            global_proxy_type: "https".to_string(),
            global_proxy_host: "proxy.example.test".to_string(),
            global_proxy_port: Some(8443),
            global_proxy_user: "global-proxy-user-sentinel".to_string(),
            global_proxy_password: "global-proxy-password-sentinel".to_string(),
            ..Default::default()
        }
    }

    fn object_keys(value: &Value) -> HashSet<&str> {
        value
            .as_object()
            .expect("expected JSON object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn assert_key_absent(value: &Value, forbidden: &str) {
        match value {
            Value::Object(map) => {
                assert!(
                    !map.contains_key(forbidden),
                    "found forbidden key {forbidden}"
                );
                for nested in map.values() {
                    assert_key_absent(nested, forbidden);
                }
            }
            Value::Array(values) => {
                for nested in values {
                    assert_key_absent(nested, forbidden);
                }
            }
            _ => {}
        }
    }

    fn assert_error_does_not_contain(mut rendered: Vec<String>, sentinel: &str) {
        for output in rendered.drain(..) {
            assert!(
                !output.contains(sentinel),
                "payload error exposed sentinel: {output}"
            );
        }
    }

    fn decrypt_json_payload(value: &Value) -> Result<DecodedSyncPayload, SyncError> {
        let encrypted = super::super::encrypt_payload(value, TEST_SYNC_PASSWORD).unwrap();
        super::super::decrypt_payload(&encrypted, TEST_SYNC_PASSWORD)
    }

    fn assert_invalid_sync_payload(value: &Value) {
        let error = decrypt_json_payload(value).unwrap_err();
        assert!(matches!(error, SyncError::InvalidPayload(_)));
    }

    #[test]
    fn portable_config_contains_the_explicit_whitelist() {
        let portable = PortableConfigV2::from(&populated_config());
        let json = serde_json::to_value(portable).unwrap();

        assert_eq!(
            object_keys(&json),
            HashSet::from([
                "sessions",
                "session_folders",
                "preferences",
                "key_bindings",
                "proxy",
            ])
        );
        assert_eq!(
            object_keys(&json["preferences"]),
            HashSet::from([
                "follow_system_theme",
                "theme_mode",
                "light_theme_name",
                "dark_theme_name",
                "locale",
                "terminal_font_size",
                "ui_font_size",
                "right_click_copy_paste",
                "keyword_highlight",
                "editor_soft_wrap",
                "history_completion_plugin_enabled",
                "ui_font_family",
                "terminal_font_family",
                "cursor_style",
                "show_hidden_files",
                "lock_layout",
                "monitoring_position",
            ])
        );
        assert_eq!(
            object_keys(&json["proxy"]),
            HashSet::from([
                "use_proxy",
                "read_env_proxy",
                "global_proxy_type",
                "global_proxy_host",
                "global_proxy_port",
                "global_proxy_user",
                "global_proxy_password",
            ])
        );
        assert_eq!(json["sessions"][0]["password"], "session-password-sentinel");
        assert_eq!(json["key_bindings"]["Copy"], "ctrl-c");
        assert_eq!(json["proxy"]["global_proxy_port"], 8443);
    }

    #[test]
    fn portable_config_excludes_device_local_and_sync_fields() {
        let mut config = populated_config();
        config.window_bounds = Some(SavedWindowBounds::Windowed {
            x: 91_001.0,
            y: 91_002.0,
            width: 91_003.0,
            height: 91_004.0,
        });
        config.workspace_panels = Some(vec![92_001.0]);
        config.body_panels = Some(vec![93_001.0]);
        config.sidebar_collapsed = true;
        config.sftp_panel_minimized = true;
        config.sync_endpoint = "excluded-sync-endpoint-sentinel".to_string();
        config.sync_username = "excluded-sync-username-sentinel".to_string();
        config.sync_etag = Some("excluded-sync-etag-sentinel".to_string());
        config.sync_device_id = "excluded-device-id-sentinel".to_string();
        config.sync_backend = "excluded-sync-backend-sentinel".to_string();
        config.sync_etag_backend = "excluded-sync-etag-backend-sentinel".to_string();
        config.sync_s3_endpoint = "excluded-s3-endpoint-sentinel".to_string();
        config.sync_s3_region = "excluded-s3-region-sentinel".to_string();
        config.sync_s3_bucket = "excluded-s3-bucket-sentinel".to_string();
        config.sync_s3_object_key = "excluded-s3-key-sentinel".to_string();

        let json = serde_json::to_value(PortableConfigV2::from(&config)).unwrap();
        for forbidden in [
            "window_bounds",
            "workspace_panels",
            "body_panels",
            "transfers",
            "sidebar_collapsed",
            "sftp_panel_minimized",
            "device_id",
            "sync_endpoint",
            "sync_username",
            "sync_etag",
            "sync_device_id",
            "sync_backend",
            "sync_etag_backend",
            "sync_s3_endpoint",
            "sync_s3_region",
            "sync_s3_bucket",
            "sync_s3_object_key",
        ] {
            assert_key_absent(&json, forbidden);
        }
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(!serialized.contains("excluded-"));
        assert!(!serialized.contains("91001"));
        assert!(!serialized.contains("92001"));
        assert!(!serialized.contains("93001"));
    }

    #[test]
    fn nonexistent_external_private_key_path_is_preserved_without_being_read() {
        let missing_path = std::env::temp_dir()
            .join("jshell-sync-test-key-that-does-not-exist")
            .to_string_lossy()
            .into_owned();
        let mut session = test_session("external-key");
        session.private_key_path = missing_path.clone();
        session.private_key_inline = "inline-key-remains-inline".to_string();
        let config = ConfigFile {
            sessions: vec![session],
            ..Default::default()
        };

        let portable = PortableConfigV2::from(&config);

        assert_eq!(portable.sessions[0].private_key_path, missing_path);
        assert_eq!(
            portable.sessions[0].private_key_inline,
            "inline-key-remains-inline"
        );
    }

    #[test]
    fn schema_99_is_rejected_before_typed_deserialization() {
        let error =
            decode_payload(br#"{"schema_version":99,"secret":"must-not-leak"}"#).unwrap_err();

        assert!(matches!(error, PayloadError::UnsupportedSchemaVersion(99)));
        assert!(!error.to_string().contains("must-not-leak"));
    }

    #[test]
    fn typed_payload_errors_do_not_expose_invalid_field_values() {
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let mut value = serde_json::to_value(payload).unwrap();
        value["portable_config"]["proxy"]["global_proxy_port"] =
            Value::String("payload-secret-must-not-leak".to_string());

        let error = decode_payload(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        let rendered = vec![
            error.to_string(),
            format!("{error:#}"),
            format!("{error:?}"),
        ];
        let wrapped =
            anyhow::Error::new(error).context("decode synchronized configuration payload");
        assert_error_does_not_contain(rendered, "payload-secret-must-not-leak");
        assert_error_does_not_contain(
            vec![format!("{wrapped:#}"), format!("{wrapped:?}")],
            "payload-secret-must-not-leak",
        );
    }

    #[test]
    fn v2_unknown_key_binding_action_is_rejected_as_invalid_payload() {
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let mut value = serde_json::to_value(payload).unwrap();
        value["portable_config"]["key_bindings"]["UnknownAction"] =
            Value::String("ctrl-shift-u".to_string());

        assert_invalid_sync_payload(&value);
    }

    #[test]
    fn v2_invalid_key_binding_syntax_is_rejected_as_invalid_payload() {
        const INVALID_KEYSTROKE: &str = "ctrl-a-b";
        assert!(gpui::Keystroke::parse(INVALID_KEYSTROKE).is_err());

        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let mut value = serde_json::to_value(payload).unwrap();
        value["portable_config"]["key_bindings"]["Copy"] =
            Value::String(INVALID_KEYSTROKE.to_string());

        assert_invalid_sync_payload(&value);
    }

    #[test]
    fn v2_whitespace_only_key_binding_is_rejected_as_invalid_payload() {
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let mut value = serde_json::to_value(payload).unwrap();
        value["portable_config"]["key_bindings"]["Copy"] = Value::String("   \t".to_string());

        assert_invalid_sync_payload(&value);
    }

    #[test]
    fn v2_valid_empty_and_none_key_bindings_are_accepted() {
        let mut config = populated_config();
        config.key_bindings = HashMap::from([
            ("Copy".to_string(), "ctrl-shift-c".to_string()),
            ("Paste".to_string(), String::new()),
            ("SaveDocument".to_string(), "none".to_string()),
        ]);
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&config));
        let value = serde_json::to_value(payload).unwrap();

        assert!(matches!(
            decrypt_json_payload(&value).unwrap(),
            DecodedSyncPayload::V2(_)
        ));
    }

    #[test]
    fn v2_runtime_compatible_key_binding_sequence_is_accepted() {
        const VALID_SEQUENCE: &str = "ctrl-k ctrl-c";
        assert!(
            VALID_SEQUENCE
                .split_whitespace()
                .all(|keystroke| gpui::Keystroke::parse(keystroke).is_ok())
        );

        let mut config = populated_config();
        config
            .key_bindings
            .insert("Copy".to_string(), VALID_SEQUENCE.to_string());
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&config));
        let value = serde_json::to_value(payload).unwrap();

        assert!(matches!(
            decrypt_json_payload(&value).unwrap(),
            DecodedSyncPayload::V2(_)
        ));
    }

    #[test]
    fn v2_session_unknown_field_is_rejected_as_invalid_payload() {
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let mut value = serde_json::to_value(payload).unwrap();
        value["portable_config"]["sessions"][0]["unknown_session_field"] = Value::Bool(true);

        assert_invalid_sync_payload(&value);
    }

    #[test]
    fn v2_session_folder_unknown_field_is_rejected_as_invalid_payload() {
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let mut value = serde_json::to_value(payload).unwrap();
        value["portable_config"]["session_folders"][0]["unknown_folder_field"] = Value::Bool(true);

        assert_invalid_sync_payload(&value);
    }

    #[test]
    fn v1_session_unknown_field_is_rejected_as_invalid_payload() {
        let payload = LegacySyncPayloadV1 {
            schema_version: 1,
            revision: "legacy-revision".to_string(),
            updated_at: "2026-08-01T00:00:00Z".to_string(),
            device_id: "legacy-device".to_string(),
            sessions: vec![test_session("legacy-session")],
        };
        let mut value = serde_json::to_value(payload).unwrap();
        value["sessions"][0]["unknown_session_field"] = Value::Bool(true);

        assert_invalid_sync_payload(&value);
    }

    #[test]
    fn invalid_json_errors_keep_only_the_fixed_category() {
        let error = decode_payload(br#"{"payload-secret-must-not-leak":"unterminated string}"#)
            .unwrap_err();

        assert_eq!(format!("{error:?}"), "InvalidJson");
        assert_error_does_not_contain(
            vec![error.to_string(), format!("{error:#}")],
            "payload-secret-must-not-leak",
        );
    }

    #[test]
    fn duplicate_session_ids_are_rejected() {
        let config = ConfigFile {
            sessions: vec![test_session("duplicate"), test_session("duplicate")],
            ..Default::default()
        };
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&config));

        let error = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap_err();

        assert!(matches!(error, PayloadError::DuplicateSessionId));
    }

    #[test]
    fn duplicate_folder_ids_are_rejected() {
        let config = ConfigFile {
            session_folders: vec![
                SessionFolder {
                    id: "duplicate".to_string(),
                    name: "One".to_string(),
                    session_ids: Vec::new(),
                },
                SessionFolder {
                    id: "duplicate".to_string(),
                    name: "Two".to_string(),
                    session_ids: Vec::new(),
                },
            ],
            ..Default::default()
        };
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&config));

        let error = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap_err();

        assert!(matches!(error, PayloadError::DuplicateFolderId));
    }

    #[test]
    fn folder_references_to_unknown_sessions_are_rejected() {
        let config = ConfigFile {
            session_folders: vec![SessionFolder {
                id: "folder-1".to_string(),
                name: "Broken".to_string(),
                session_ids: vec!["missing-session".to_string()],
            }],
            ..Default::default()
        };
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&config));

        let error = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap_err();

        assert!(matches!(error, PayloadError::UnknownSessionReference));
    }

    #[test]
    fn semantic_validation_rejects_invalid_preferences_and_global_proxy() {
        let mut cases = Vec::new();

        let mut invalid_theme = populated_config();
        invalid_theme.theme_mode = "payload-theme-secret".to_string();
        cases.push((invalid_theme, "theme_mode", "payload-theme-secret"));

        let mut invalid_locale = populated_config();
        invalid_locale.locale = "payload-locale-secret".to_string();
        cases.push((invalid_locale, "locale", "payload-locale-secret"));

        let mut invalid_terminal_font = populated_config();
        invalid_terminal_font.terminal_font_size = -1.0;
        cases.push((invalid_terminal_font, "terminal_font_size", "-1"));

        let mut invalid_ui_font = populated_config();
        invalid_ui_font.ui_font_size = 99.0;
        cases.push((invalid_ui_font, "ui_font_size", "99"));

        let mut invalid_monitoring = populated_config();
        invalid_monitoring.monitoring_position = "payload-position-secret".to_string();
        cases.push((
            invalid_monitoring,
            "monitoring_position",
            "payload-position-secret",
        ));

        let mut invalid_proxy = populated_config();
        invalid_proxy.use_proxy = true;
        invalid_proxy.global_proxy_type = "payload-proxy-secret".to_string();
        invalid_proxy.global_proxy_host = "proxy.example.test".to_string();
        invalid_proxy.global_proxy_port = Some(1080);
        cases.push((invalid_proxy, "global_proxy_type", "payload-proxy-secret"));

        for (config, expected_field, secret) in cases {
            let payload = SyncPayloadV2::new(PortableConfigV2::from(&config));
            let error = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap_err();

            assert!(matches!(error, PayloadError::InvalidField(field) if field == expected_field));
            assert!(!error.to_string().contains(secret));
            assert!(!format!("{error:?}").contains(secret));
        }
    }

    #[test]
    fn semantic_validation_rejects_invalid_session_protocol_and_proxy() {
        let mut invalid_protocol = populated_config();
        invalid_protocol.sessions[0].protocol = "payload-protocol-secret".to_string();
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&invalid_protocol));
        let error = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap_err();
        assert!(matches!(
            error,
            PayloadError::InvalidField("session.protocol")
        ));
        assert!(!error.to_string().contains("payload-protocol-secret"));

        let mut invalid_proxy = populated_config();
        invalid_proxy.sessions[0].proxy_type = "payload-session-proxy-secret".to_string();
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&invalid_proxy));
        let error = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap_err();
        assert!(matches!(
            error,
            PayloadError::InvalidField("session.proxy_type")
        ));
        assert!(!error.to_string().contains("payload-session-proxy-secret"));
    }

    #[test]
    fn preview_contains_only_counts_and_non_secret_metadata() {
        let payload = SyncPayloadV2::new(PortableConfigV2::from(&populated_config()));
        let decoded = decode_payload(&serde_json::to_vec(&payload).unwrap()).unwrap();
        assert!(matches!(decoded, DecodedSyncPayload::V2(_)));

        let preview = serde_json::to_value(decoded.preview()).unwrap();

        assert_eq!(
            object_keys(&preview),
            HashSet::from([
                "schema_version",
                "updated_at",
                "session_count",
                "folder_count",
                "preference_category_count",
                "external_private_key_path_count",
            ])
        );
        assert_eq!(preview["schema_version"], 2);
        assert_eq!(preview["session_count"], 1);
        assert_eq!(preview["folder_count"], 1);
        assert_eq!(
            preview["preference_category_count"],
            PortableConfigV2::preference_category_count()
        );
        assert_eq!(preview["external_private_key_path_count"], 0);
        let serialized = serde_json::to_string(&preview).unwrap();
        for secret in [
            "session-password-sentinel",
            "session-passphrase-sentinel",
            "inline-private-key-sentinel",
            "session-proxy-password-sentinel",
            "global-proxy-password-sentinel",
        ] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn preview_against_local_config_counts_only_changed_preference_categories() {
        let local = populated_config();
        let mut remote = local.clone();

        let unchanged = DecodedSyncPayload::V2(Box::new(SyncPayloadV2::new(
            PortableConfigV2::from(&remote),
        )));
        assert_eq!(
            unchanged.preview_against(&local).preference_category_count,
            0
        );

        remote.locale = "en".to_string();
        let preferences_changed = DecodedSyncPayload::V2(Box::new(SyncPayloadV2::new(
            PortableConfigV2::from(&remote),
        )));
        assert_eq!(
            preferences_changed
                .preview_against(&local)
                .preference_category_count,
            1
        );

        remote
            .key_bindings
            .insert("workspace.copy".to_string(), "ctrl-shift-c".to_string());
        remote.use_proxy = true;
        remote.global_proxy_host = "proxy.changed.test".to_string();
        let all_changed = DecodedSyncPayload::V2(Box::new(SyncPayloadV2::new(
            PortableConfigV2::from(&remote),
        )));
        assert_eq!(
            all_changed
                .preview_against(&local)
                .preference_category_count,
            3
        );
    }
}
