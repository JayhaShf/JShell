pub mod controller;
pub mod language;
pub mod large_file;
pub mod remote;
pub mod ui;

use gpui::Entity;
use gpui_component::input::InputState;

use crate::session::config::Session;

pub const MIB: u64 = 1024 * 1024;
pub const EDITABLE_MAX_BYTES: u64 = 10 * MIB;
pub const PAGED_MAX_BYTES: u64 = 100 * MIB;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentMode {
    Editable,
    PagedReadOnly,
    MetadataOnly,
}

pub fn mode_for_size(size: u64) -> DocumentMode {
    if size <= EDITABLE_MAX_BYTES {
        DocumentMode::Editable
    } else if size <= PAGED_MAX_BYTES {
        DocumentMode::PagedReadOnly
    } else {
        DocumentMode::MetadataOnly
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Lf => text.as_bytes().to_vec(),
            Self::CrLf => text.replace('\n', "\r\n").into_bytes(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Utf8,
    UnsupportedEncoding,
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnlyReason {
    UnsupportedEncoding,
    Binary,
    LargeFile,
    MetadataOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveState {
    Idle,
    Checking,
    Saving,
    Saved,
    Conflict,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentConnectionState {
    Online,
    Reconnecting,
    Offline(String),
}

#[derive(Clone, Debug)]
pub struct LoadedDocument {
    pub metadata: remote::RemoteMetadata,
    pub mode: DocumentMode,
    pub language: String,
    pub text: String,
    pub original_hash: [u8; 32],
    pub line_ending: LineEnding,
    pub had_bom: bool,
    pub read_only_reason: Option<ReadOnlyReason>,
    pub large_file: Option<large_file::LargeFileState>,
}

#[derive(Clone, Debug)]
pub enum DocumentEvent {
    Loaded {
        document_id: String,
        operation_id: String,
        loaded: LoadedDocument,
    },
    LoadFailed {
        document_id: String,
        operation_id: String,
        error: String,
        offline: bool,
    },
    Saved {
        document_id: String,
        operation_id: String,
        revision: u64,
        metadata: remote::RemoteMetadata,
        original_hash: [u8; 32],
    },
    SaveFailed {
        document_id: String,
        operation_id: String,
        error: String,
        offline: bool,
    },
    PageLoaded {
        document_id: String,
        operation_id: String,
        requested_offset: u64,
        page: large_file::NormalizedWindow,
    },
    PageFailed {
        document_id: String,
        operation_id: String,
        error: String,
        offline: bool,
    },
}

pub struct RemoteDocument {
    pub key: remote::RemoteDocumentKey,
    pub session: Session,
    pub editor: Entity<InputState>,
    pub large_file_viewer: Entity<InputState>,
    pub large_file_search: Entity<InputState>,
    pub language: String,
    pub original_hash: [u8; 32],
    pub metadata: Option<remote::RemoteMetadata>,
    pub line_ending: LineEnding,
    pub had_bom: bool,
    pub revisions: DocumentRevisions,
    pub mode: DocumentMode,
    pub read_only_reason: Option<ReadOnlyReason>,
    pub load_state: LoadState,
    pub save_state: SaveState,
    pub connection_state: DocumentConnectionState,
    pub operation_id: String,
    pub backend: remote::SftpRemoteFileBackend,
    pub suppress_changes: bool,
    pub soft_wrap: bool,
    pub close_after_save: bool,
    pub pending_save: Option<controller::PendingSave>,
    pub large_file: Option<large_file::LargeFileState>,
    pub _subscription: gpui::Subscription,
    pub _large_file_search_subscription: gpui::Subscription,
}

impl RemoteDocument {
    pub fn title(&self) -> &str {
        self.key
            .remote_path
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&self.key.remote_path)
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only_reason.is_some() || self.mode != DocumentMode::Editable
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedText {
    pub text: String,
    pub kind: TextKind,
    pub line_ending: LineEnding,
    pub had_bom: bool,
    pub read_only: bool,
}

pub fn inspect_text(bytes: &[u8]) -> InspectedText {
    if bytes.iter().take(1024).any(|byte| *byte == 0) {
        return InspectedText {
            text: String::new(),
            kind: TextKind::Binary,
            line_ending: LineEnding::Lf,
            had_bom: false,
            read_only: true,
        };
    }

    let had_bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let body = if had_bom { &bytes[3..] } else { bytes };
    let line_ending = if body.windows(2).any(|pair| pair == b"\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };
    let (text, kind, read_only) = match std::str::from_utf8(body) {
        Ok(text) => (text.to_string(), TextKind::Utf8, false),
        Err(_) => (
            String::from_utf8_lossy(body).into_owned(),
            TextKind::UnsupportedEncoding,
            true,
        ),
    };

    InspectedText {
        text: text.replace("\r\n", "\n").replace('\r', "\n"),
        kind,
        line_ending,
        had_bom,
        read_only,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentRevisions {
    edit: u64,
    saved: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceTab {
    Session { id: String, group_id: String },
    RemoteDocument { id: String, document_id: String },
}

impl WorkspaceTab {
    pub fn id(&self) -> &str {
        match self {
            Self::Session { id, .. } | Self::RemoteDocument { id, .. } => id,
        }
    }
}

pub fn active_document_id<'a>(
    workspaces: &'a [WorkspaceTab],
    active_workspace_id: Option<&str>,
) -> Option<&'a str> {
    let active_workspace_id = active_workspace_id?;
    workspaces.iter().find_map(|workspace| match workspace {
        WorkspaceTab::RemoteDocument { id, document_id } if id == active_workspace_id => {
            Some(document_id.as_str())
        }
        _ => None,
    })
}

pub fn ordered_dirty_document_ids(
    workspaces: &[WorkspaceTab],
    mut is_dirty: impl FnMut(&str) -> bool,
) -> Vec<&str> {
    workspaces
        .iter()
        .filter_map(|workspace| match workspace {
            WorkspaceTab::RemoteDocument { document_id, .. } if is_dirty(document_id) => {
                Some(document_id.as_str())
            }
            _ => None,
        })
        .collect()
}

impl DocumentRevisions {
    pub fn changed(&mut self) {
        self.edit = self.edit.saturating_add(1);
    }

    pub fn begin_save(&self) -> u64 {
        self.edit
    }

    pub fn finish_save(&mut self, saved_revision: u64) {
        self.saved = self.saved.max(saved_revision);
    }

    pub fn is_dirty(&self) -> bool {
        self.edit != self.saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_document_size_thresholds() {
        assert_eq!(mode_for_size(10 * MIB), DocumentMode::Editable);
        assert_eq!(mode_for_size(10 * MIB + 1), DocumentMode::PagedReadOnly);
        assert_eq!(mode_for_size(100 * MIB), DocumentMode::PagedReadOnly);
        assert_eq!(mode_for_size(100 * MIB + 1), DocumentMode::MetadataOnly);
    }

    #[test]
    fn inspects_utf8_bom_crlf_and_binary_content() {
        let decoded = inspect_text(b"\xef\xbb\xbfalpha\r\nbeta\r\n");
        assert_eq!(decoded.text, "alpha\nbeta\n");
        assert_eq!(decoded.line_ending, LineEnding::CrLf);
        assert!(!decoded.read_only);

        assert_eq!(inspect_text(b"abc\0def").kind, TextKind::Binary);
        assert!(inspect_text(&[0xff, 0xfe, 0x41]).read_only);
    }

    #[test]
    fn revisions_remain_dirty_when_typing_continues_during_save() {
        let mut revisions = DocumentRevisions::default();
        revisions.changed();
        let snapshot = revisions.begin_save();
        revisions.changed();
        revisions.finish_save(snapshot);
        assert!(revisions.is_dirty());
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::{WorkspaceTab, active_document_id, ordered_dirty_document_ids};

    #[test]
    fn exposes_stable_ids_for_both_workspace_kinds() {
        let terminal = WorkspaceTab::Session {
            id: "w1".into(),
            group_id: "g1".into(),
        };
        let document = WorkspaceTab::RemoteDocument {
            id: "w2".into(),
            document_id: "d1".into(),
        };
        assert_eq!(terminal.id(), "w1");
        assert_eq!(document.id(), "w2");
    }

    #[test]
    fn resolves_only_the_active_document_workspace() {
        let workspaces = vec![
            WorkspaceTab::Session {
                id: "w1".into(),
                group_id: "g1".into(),
            },
            WorkspaceTab::RemoteDocument {
                id: "w2".into(),
                document_id: "d1".into(),
            },
        ];

        assert_eq!(active_document_id(&workspaces, Some("w2")), Some("d1"));
        assert_eq!(active_document_id(&workspaces, Some("w1")), None);
        assert_eq!(active_document_id(&workspaces, Some("missing")), None);
    }

    #[test]
    fn dirty_documents_follow_workspace_order() {
        let workspaces = vec![
            WorkspaceTab::RemoteDocument {
                id: "w2".into(),
                document_id: "d2".into(),
            },
            WorkspaceTab::Session {
                id: "w1".into(),
                group_id: "g1".into(),
            },
            WorkspaceTab::RemoteDocument {
                id: "w3".into(),
                document_id: "clean".into(),
            },
            WorkspaceTab::RemoteDocument {
                id: "w4".into(),
                document_id: "d1".into(),
            },
        ];

        assert_eq!(
            ordered_dirty_document_ids(&workspaces, |id| id != "clean"),
            vec!["d2", "d1"]
        );
    }
}
