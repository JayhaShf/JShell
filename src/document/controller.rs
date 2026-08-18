use crate::document::remote::RemoteDocumentKey;
use crate::document::{
    DocumentMode, EDITABLE_MAX_BYTES, LoadedDocument, ReadOnlyReason, TextKind, inspect_text,
    language::detect_language,
    large_file::{LargeFileState, PAGE_BYTES, PAGE_READ_BYTES, normalize_window},
    mode_for_size,
    remote::{ByteRange, RemoteFileBackend},
};
use anyhow::{Result, anyhow};
use gpui::{AppContext as _, Context, Entity, Focusable as _, PathPromptOptions, Window};
use gpui_component::input::{InputEvent, InputState};
use rust_i18n::t;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Ashell, PaneLeaf,
    document::{
        DocumentConnectionState, DocumentEvent, DocumentRevisions, LineEnding, LoadState,
        RemoteDocument, SaveState, WorkspaceTab, remote::SftpRemoteFileBackend,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub path: String,
    pub bytes: Vec<u8>,
    pub opened_metadata: crate::document::remote::RemoteMetadata,
    pub original_hash: [u8; 32],
    pub revision: u64,
    pub operation_id: String,
}

impl SaveRequest {
    pub fn new(
        path: impl Into<String>,
        bytes: Vec<u8>,
        opened_metadata: crate::document::remote::RemoteMetadata,
        original_hash: [u8; 32],
        revision: u64,
        operation_id: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            bytes,
            opened_metadata,
            original_hash,
            revision,
            operation_id: operation_id.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSave {
    pub path: String,
    pub bytes: Vec<u8>,
    pub opened_metadata: crate::document::remote::RemoteMetadata,
    pub original_hash: [u8; 32],
    pub current_metadata: Option<crate::document::remote::RemoteMetadata>,
    pub revision: u64,
    pub operation_id: String,
}

impl PendingSave {
    fn from_request(
        request: SaveRequest,
        current_metadata: Option<crate::document::remote::RemoteMetadata>,
    ) -> Self {
        Self {
            path: request.path,
            bytes: request.bytes,
            opened_metadata: request.opened_metadata,
            original_hash: request.original_hash,
            current_metadata,
            revision: request.revision,
            operation_id: request.operation_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved {
        revision: u64,
        metadata: crate::document::remote::RemoteMetadata,
        original_hash: [u8; 32],
    },
    Conflict(PendingSave),
    RemoteDeleted(PendingSave),
    OutcomeUnknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    CloseNow,
    Prompt,
}

pub fn save_state_after_completion(dirty: bool) -> SaveState {
    if dirty {
        SaveState::Idle
    } else {
        SaveState::Saved
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteFailureDisposition {
    Failed,
    OutcomeUnknown,
}

pub fn write_failure_disposition(error: &anyhow::Error) -> WriteFailureDisposition {
    if crate::document::remote::is_connection_closed(error) {
        WriteFailureDisposition::OutcomeUnknown
    } else {
        WriteFailureDisposition::Failed
    }
}

pub fn close_decision(revisions: DocumentRevisions) -> CloseDecision {
    if revisions.is_dirty() {
        CloseDecision::Prompt
    } else {
        CloseDecision::CloseNow
    }
}

fn preserved_focus_path_after_document_removal(
    pane_root: &crate::PaneLayout,
    previous_focus: Option<&PaneLeaf>,
    removed_document_id: &str,
) -> Option<Vec<usize>> {
    match previous_focus {
        Some(PaneLeaf::Terminal(tab_id)) => pane_root.path_to_terminal(tab_id),
        Some(PaneLeaf::Document(document_id)) if document_id != removed_document_id => {
            pane_root.path_to_document(document_id)
        }
        Some(PaneLeaf::Document(_) | PaneLeaf::Empty) | None => None,
    }
}

pub async fn run_save_check<B: RemoteFileBackend>(
    backend: &B,
    request: SaveRequest,
) -> Result<SaveOutcome> {
    let current_metadata = match backend.stat(&request.path).await {
        Ok(metadata) => metadata,
        Err(error) if crate::document::remote::is_not_found(&error) => {
            return Ok(SaveOutcome::RemoteDeleted(PendingSave::from_request(
                request, None,
            )));
        }
        Err(error) => return Err(error),
    };

    if crate::document::remote::has_conflict(&request.opened_metadata, &current_metadata) {
        return Ok(SaveOutcome::Conflict(PendingSave::from_request(
            request,
            Some(current_metadata),
        )));
    }

    let current_bytes = match backend.read(&request.path, None).await {
        Ok(bytes) => bytes,
        Err(error) if crate::document::remote::is_not_found(&error) => {
            return Ok(SaveOutcome::RemoteDeleted(PendingSave::from_request(
                request, None,
            )));
        }
        Err(error) => return Err(error),
    };
    if <[u8; 32]>::from(Sha256::digest(&current_bytes)) != request.original_hash {
        return Ok(SaveOutcome::Conflict(PendingSave::from_request(
            request,
            Some(current_metadata),
        )));
    }

    let metadata = match backend
        .write_atomic(
            &request.path,
            request.bytes.clone(),
            request.opened_metadata.permissions,
            &request.operation_id,
        )
        .await
    {
        Ok(metadata) => metadata,
        Err(error)
            if write_failure_disposition(&error) == WriteFailureDisposition::OutcomeUnknown =>
        {
            return Ok(SaveOutcome::OutcomeUnknown(format!(
                "connection closed after the save started: {error:#}"
            )));
        }
        Err(error) => return Err(error),
    };
    Ok(SaveOutcome::Saved {
        revision: request.revision,
        metadata,
        original_hash: Sha256::digest(&request.bytes).into(),
    })
}

pub fn find_existing_document<'a>(
    mut documents: impl Iterator<Item = (&'a str, &'a RemoteDocumentKey)>,
    requested: &RemoteDocumentKey,
) -> Option<&'a str> {
    documents
        .find(|(_, key)| *key == requested)
        .map(|(id, _)| id)
}

pub async fn load_remote_document<B: RemoteFileBackend>(
    backend: &B,
    path: &str,
) -> Result<LoadedDocument> {
    let metadata = backend.stat(path).await?;
    let mode = mode_for_size(metadata.size);
    let bytes = match mode {
        DocumentMode::Editable => backend.read(path, None).await?,
        DocumentMode::PagedReadOnly => {
            backend
                .read(
                    path,
                    Some(ByteRange {
                        offset: 0,
                        length: PAGE_READ_BYTES,
                    }),
                )
                .await?
        }
        DocumentMode::MetadataOnly => Vec::new(),
    };
    if mode == DocumentMode::Editable && bytes.len() as u64 > EDITABLE_MAX_BYTES {
        return Err(anyhow!(
            "remote document grew beyond the editable size limit while loading"
        ));
    }

    let (inspected, large_file) = if mode == DocumentMode::PagedReadOnly {
        let binary = bytes.iter().take(1024).any(|byte| *byte == 0);
        if binary {
            let inspected = inspect_text(&bytes);
            let large_file = LargeFileState {
                offset: 0,
                start_offset: 0,
                end_offset: bytes.len() as u64,
                text: inspected.text.clone(),
                loading: false,
                operation_id: String::new(),
                error: None,
            };
            (inspected, Some(large_file))
        } else {
            match normalize_window(&bytes, 0) {
                Ok(page) => {
                    let inspected = inspect_text(page.text.as_bytes());
                    let large_file = LargeFileState {
                        offset: 0,
                        start_offset: page.start_offset,
                        end_offset: page.end_offset,
                        text: inspected.text.clone(),
                        loading: false,
                        operation_id: String::new(),
                        error: None,
                    };
                    (inspected, Some(large_file))
                }
                Err(_) => {
                    let inspected = inspect_text(&bytes);
                    let large_file = LargeFileState {
                        offset: 0,
                        start_offset: 0,
                        end_offset: bytes.len() as u64,
                        text: inspected.text.clone(),
                        loading: false,
                        operation_id: String::new(),
                        error: None,
                    };
                    (inspected, Some(large_file))
                }
            }
        }
    } else {
        (inspect_text(&bytes), None)
    };
    let first_line = inspected.text.lines().next();
    let language = detect_language(path, first_line).to_string();
    let original_hash = Sha256::digest(&bytes).into();
    let read_only_reason = match inspected.kind {
        TextKind::Binary => Some(ReadOnlyReason::Binary),
        TextKind::UnsupportedEncoding => Some(ReadOnlyReason::UnsupportedEncoding),
        TextKind::Utf8 => match mode {
            DocumentMode::Editable => None,
            DocumentMode::PagedReadOnly => Some(ReadOnlyReason::LargeFile),
            DocumentMode::MetadataOnly => Some(ReadOnlyReason::MetadataOnly),
        },
    };

    Ok(LoadedDocument {
        metadata,
        mode,
        language,
        text: inspected.text,
        original_hash,
        line_ending: inspected.line_ending,
        had_bom: inspected.had_bom,
        read_only_reason,
        large_file,
    })
}

impl Ashell {
    pub(crate) fn open_remote_document(
        &mut self,
        remote_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group_id) = self.active_group.clone() else {
            self.status = t!("document_no_active_ssh").to_string().into();
            cx.notify();
            return;
        };
        let Some(handle) = self.sftp_handles.get(&group_id).cloned() else {
            self.status = t!("document_sftp_unavailable").to_string().into();
            cx.notify();
            return;
        };
        let Some(session) = self
            .tab_groups
            .iter()
            .find(|group| group.id == group_id)
            .and_then(|group| {
                group.pane_root.tab_ids().iter().find_map(|tab_id| {
                    self.tabs
                        .iter()
                        .find(|tab| tab.id == *tab_id)
                        .and_then(|tab| tab.session.clone())
                })
            })
        else {
            self.status = t!("document_no_ssh_session").to_string().into();
            cx.notify();
            return;
        };

        let key = RemoteDocumentKey::new(group_id, &remote_path);
        if let Some(existing_id) = find_existing_document(
            self.documents
                .iter()
                .map(|(id, document)| (id.as_str(), &document.key)),
            &key,
        )
        .map(str::to_string)
        {
            self.focus_document_workspace(&existing_id, window, cx);
            return;
        }

        let document_id = Uuid::new_v4().to_string();
        let operation_id = Uuid::new_v4().to_string();
        let soft_wrap = self.config.editor_soft_wrap();
        let editor: Entity<InputState> = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("text")
                .line_number(true)
                .searchable(true)
                .soft_wrap(soft_wrap)
        });
        let large_file_viewer: Entity<InputState> = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .searchable(true)
                .soft_wrap(soft_wrap)
        });
        let large_file_search: Entity<InputState> = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("document_search_chunk").to_string())
        });
        let subscription_document_id = document_id.clone();
        let subscription = cx.subscribe(&editor, move |this, _, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let Some(document) = this.documents.get_mut(&subscription_document_id) else {
                return;
            };
            if document.suppress_changes || document.is_read_only() {
                return;
            }
            document.revisions.changed();
            if matches!(document.save_state, SaveState::Saved) {
                document.save_state = SaveState::Idle;
            }
            cx.notify();
        });
        let large_file_search_subscription =
            cx.subscribe(&large_file_search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
        let backend = SftpRemoteFileBackend::new(handle);
        let document = RemoteDocument {
            key: key.clone(),
            session,
            editor: editor.clone(),
            large_file_viewer,
            large_file_search,
            language: "text".into(),
            original_hash: Sha256::digest([]).into(),
            metadata: None,
            line_ending: LineEnding::Lf,
            had_bom: false,
            revisions: DocumentRevisions::default(),
            mode: DocumentMode::Editable,
            read_only_reason: None,
            load_state: LoadState::Loading,
            save_state: SaveState::Idle,
            connection_state: DocumentConnectionState::Online,
            operation_id: operation_id.clone(),
            backend: backend.clone(),
            suppress_changes: false,
            soft_wrap,
            close_after_save: false,
            pending_save: None,
            large_file: None,
            _subscription: subscription,
            _large_file_search_subscription: large_file_search_subscription,
        };
        self.documents.insert(document_id.clone(), document);
        let insert_path = self
            .pane_root
            .insert_right(
                &self.focused_pane_path,
                PaneLeaf::Document(document_id.clone()),
            )
            .or_else(|| {
                let anchor_path = self
                    .active_tab
                    .as_deref()
                    .and_then(|tab_id| self.pane_root.path_to_terminal(tab_id))?;
                self.pane_root
                    .insert_right(&anchor_path, PaneLeaf::Document(document_id.clone()))
            });
        self.workspace_tabs.push(WorkspaceTab::RemoteDocument {
            id: document_id.clone(),
            document_id: document_id.clone(),
        });
        if let Some(path) = insert_path {
            self.focus_pane_path(path);
        }
        self.active_workspace_tab = Some(document_id.clone());
        editor.focus_handle(cx).focus(window, cx);
        cx.notify();

        let path = key.remote_path;
        cx.spawn_in(window, async move |this, cx| {
            let event = match load_remote_document(&backend, &path).await {
                Ok(loaded) => DocumentEvent::Loaded {
                    document_id,
                    operation_id,
                    loaded,
                },
                Err(error) => {
                    let offline = crate::document::remote::is_connection_closed(&error);
                    DocumentEvent::LoadFailed {
                        document_id,
                        operation_id,
                        error: format!("{error:#}"),
                        offline,
                    }
                }
            };
            let _ = gpui::AsyncWindowContext::update(cx, |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.apply_document_event(event, window, cx);
                });
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn focus_document_workspace(
        &mut self,
        document_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self
            .workspace_tabs
            .iter()
            .find_map(|workspace| match workspace {
                WorkspaceTab::RemoteDocument {
                    id,
                    document_id: existing,
                } if existing == document_id => Some(id.clone()),
                _ => None,
            })
        else {
            return;
        };
        self.active_workspace_tab = Some(workspace_id);
        if self.activate_detached_document(document_id, cx) {
            cx.notify();
            return;
        }
        if let Some(group) = self
            .tab_groups
            .iter()
            .find(|group| group.pane_root.contains_document(document_id))
        {
            self.active_group = Some(group.id.clone());
            self.pane_root = group.pane_root.clone();
            self.focused_pane_path = group
                .pane_root
                .path_to_document(document_id)
                .unwrap_or_default();
            self.focus_pane_path(self.focused_pane_path.clone());
        }
        if let Some(document) = self.documents.get(document_id) {
            document.editor.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn apply_document_event(
        &mut self,
        event: DocumentEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DocumentEvent::Loaded {
                document_id,
                operation_id,
                loaded,
            } => {
                let Some(document) = self.documents.get_mut(&document_id) else {
                    return;
                };
                if document.operation_id != operation_id {
                    return;
                }
                document.suppress_changes = true;
                if loaded.mode == DocumentMode::PagedReadOnly {
                    let text = loaded
                        .large_file
                        .as_ref()
                        .map(|state| state.text.clone())
                        .unwrap_or_default();
                    document.large_file_viewer.update(cx, |viewer, cx| {
                        viewer.set_value(text, window, cx);
                    });
                } else {
                    document.editor.update(cx, |editor, cx| {
                        editor.set_highlighter(loaded.language.clone(), cx);
                        editor.set_value(loaded.text.clone(), window, cx);
                    });
                }
                document.suppress_changes = false;
                document.language = loaded.language;
                document.original_hash = loaded.original_hash;
                document.metadata = Some(loaded.metadata);
                document.line_ending = loaded.line_ending;
                document.had_bom = loaded.had_bom;
                document.mode = loaded.mode;
                document.read_only_reason = loaded.read_only_reason;
                document.large_file = loaded.large_file;
                document.revisions = DocumentRevisions::default();
                document.load_state = LoadState::Ready;
                document.save_state = SaveState::Idle;
                document.connection_state = DocumentConnectionState::Online;
            }
            DocumentEvent::LoadFailed {
                document_id,
                operation_id,
                error,
                offline,
            } => {
                let Some(document) = self.documents.get_mut(&document_id) else {
                    return;
                };
                if document.operation_id == operation_id {
                    document.load_state = LoadState::Failed(error.clone());
                    if offline {
                        document.connection_state = DocumentConnectionState::Offline(error);
                    }
                }
            }
            DocumentEvent::Saved {
                document_id,
                operation_id,
                revision,
                metadata,
                original_hash,
            } => {
                let (close_after_save, is_dirty) = {
                    let Some(document) = self.documents.get_mut(&document_id) else {
                        return;
                    };
                    if document.operation_id != operation_id {
                        return;
                    }
                    document.metadata = Some(metadata);
                    document.original_hash = original_hash;
                    document.revisions.finish_save(revision);
                    document.pending_save = None;
                    document.connection_state = DocumentConnectionState::Online;
                    let is_dirty = document.revisions.is_dirty();
                    document.save_state = save_state_after_completion(is_dirty);
                    (document.close_after_save, is_dirty)
                };
                if self.window_close_save_current.as_deref() == Some(document_id.as_str()) {
                    if is_dirty {
                        self.cancel_window_close_save();
                        cx.notify();
                    } else {
                        self.window_close_save_current = None;
                        self.window_close_save_queue
                            .retain(|queued| queued != &document_id);
                        self.save_next_document_for_window_close(window, cx);
                    }
                    return;
                }
                if close_after_save && !is_dirty {
                    self.close_document_now(&document_id, window, cx);
                    return;
                }
            }
            DocumentEvent::SaveFailed {
                document_id,
                operation_id,
                error,
                offline,
            } => {
                let Some(document) = self.documents.get_mut(&document_id) else {
                    return;
                };
                if document.operation_id == operation_id {
                    document.save_state = SaveState::Failed(error.clone());
                    document.close_after_save = false;
                    if offline {
                        document.connection_state = DocumentConnectionState::Offline(error);
                    }
                }
                if self.window_close_save_current.as_deref() == Some(document_id.as_str()) {
                    self.cancel_window_close_save();
                }
            }
            DocumentEvent::SaveOutcomeUnknown {
                document_id,
                operation_id,
                error,
            } => {
                let Some(document) = self.documents.get_mut(&document_id) else {
                    return;
                };
                if document.operation_id == operation_id {
                    document.save_state = SaveState::OutcomeUnknown(error.clone());
                    document.connection_state = DocumentConnectionState::Offline(error);
                    document.close_after_save = false;
                }
                if self.window_close_save_current.as_deref() == Some(document_id.as_str()) {
                    self.cancel_window_close_save();
                }
            }
            DocumentEvent::PageLoaded {
                document_id,
                operation_id,
                requested_offset,
                page,
            } => {
                let Some(document) = self.documents.get_mut(&document_id) else {
                    return;
                };
                let Some(state) = document.large_file.as_mut() else {
                    return;
                };
                if state.operation_id != operation_id {
                    return;
                }
                document.large_file_viewer.update(cx, |viewer, cx| {
                    viewer.set_value(page.text.clone(), window, cx);
                });
                state.offset = requested_offset;
                state.start_offset = page.start_offset;
                state.end_offset = page.end_offset;
                state.text = page.text;
                state.loading = false;
                state.error = None;
                document.connection_state = DocumentConnectionState::Online;
            }
            DocumentEvent::PageFailed {
                document_id,
                operation_id,
                error,
                offline,
            } => {
                let Some(document) = self.documents.get_mut(&document_id) else {
                    return;
                };
                let Some(state) = document.large_file.as_mut() else {
                    return;
                };
                if state.operation_id == operation_id {
                    state.loading = false;
                    state.error = Some(error.clone());
                    if offline {
                        document.connection_state = DocumentConnectionState::Offline(error);
                    }
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn active_document_id(&self) -> Option<String> {
        match self.pane_root.focused_leaf(&self.focused_pane_path) {
            Some(crate::PaneLeaf::Document(document_id))
                if self.documents.contains_key(document_id) =>
            {
                Some(document_id.clone())
            }
            _ => None,
        }
    }

    pub(crate) fn dirty_document_ids(&self) -> Vec<String> {
        crate::document::ordered_dirty_document_ids(&self.workspace_tabs, |document_id| {
            self.documents
                .get(document_id)
                .is_some_and(|document| document.revisions.is_dirty())
        })
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    pub(crate) fn has_dirty_documents(&self) -> bool {
        !self.dirty_document_ids().is_empty()
    }

    pub(crate) fn request_application_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_dirty_documents() {
            self.finish_window_close(window, cx);
        } else if !self.window_close_prompt_open {
            self.window_close_prompt_open = true;
            self.show_window_close_dialog(window, cx);
        }
    }

    pub(crate) fn cancel_application_close(&mut self, cx: &mut Context<Self>) {
        self.window_close_prompt_open = false;
        self.cancel_window_close_save();
        cx.notify();
    }

    pub(crate) fn discard_all_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_close_prompt_open = false;
        self.cancel_window_close_save();
        self.finish_window_close(window, cx);
    }

    pub(crate) fn save_all_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_close_prompt_open = false;
        self.window_close_save_queue = self.dirty_document_ids();
        self.window_close_save_current = None;
        self.save_next_document_for_window_close(window, cx);
    }

    fn save_next_document_for_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let Some(document_id) = self.window_close_save_queue.first().cloned() else {
                self.finish_window_close(window, cx);
                return;
            };
            let Some(document) = self.documents.get(&document_id) else {
                self.window_close_save_queue.remove(0);
                continue;
            };
            if !document.revisions.is_dirty() {
                self.window_close_save_queue.remove(0);
                continue;
            }
            if document.is_read_only()
                || !matches!(document.load_state, LoadState::Ready)
                || document.metadata.is_none()
            {
                self.cancel_window_close_save();
                cx.notify();
                return;
            }
            self.window_close_save_current = Some(document_id.clone());
            if !matches!(document.save_state, SaveState::Checking | SaveState::Saving) {
                self.save_document(document_id, window, cx);
            }
            return;
        }
    }

    fn cancel_window_close_save(&mut self) {
        self.window_close_save_queue.clear();
        self.window_close_save_current = None;
        self.allow_window_close = false;
    }

    fn finish_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_detached_windows_for_shutdown(cx);
        self.allow_window_close = true;
        self.save_layout_state(window, cx);
        window.remove_window();
    }

    pub(crate) fn save_document(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(&document_id) else {
            return;
        };
        if document.is_read_only() || !matches!(document.load_state, LoadState::Ready) {
            return;
        }
        if matches!(document.save_state, SaveState::Checking | SaveState::Saving) {
            return;
        }
        let Some(opened_metadata) = document.metadata.clone() else {
            return;
        };
        if !document.revisions.is_dirty() {
            document.save_state = SaveState::Saved;
            cx.notify();
            return;
        }

        let text = document.editor.read(cx).value().to_string();
        let mut bytes = document.line_ending.encode(&text);
        if document.had_bom {
            bytes.splice(0..0, [0xef, 0xbb, 0xbf]);
        }
        let revision = document.revisions.begin_save();
        let operation_id = Uuid::new_v4().to_string();
        document.operation_id = operation_id.clone();
        document.save_state = SaveState::Checking;
        document.pending_save = None;
        let request = SaveRequest::new(
            document.key.remote_path.clone(),
            bytes,
            opened_metadata,
            document.original_hash,
            revision,
            operation_id.clone(),
        );
        let backend = document.backend.clone();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = run_save_check(&backend, request).await;
            let _ = gpui::AsyncWindowContext::update(cx, |window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(SaveOutcome::Saved {
                        revision,
                        metadata,
                        original_hash,
                    }) => this.apply_document_event(
                        DocumentEvent::Saved {
                            document_id,
                            operation_id,
                            revision,
                            metadata,
                            original_hash,
                        },
                        window,
                        cx,
                    ),
                    Ok(SaveOutcome::Conflict(pending)) => {
                        let is_window_close_save =
                            this.window_close_save_current.as_deref() == Some(document_id.as_str());
                        if let Some(document) = this.documents.get_mut(&document_id)
                            && document.operation_id == operation_id
                        {
                            document.save_state = SaveState::Conflict;
                            document.pending_save = Some(pending);
                            document.connection_state = DocumentConnectionState::Online;
                            if is_window_close_save {
                                this.cancel_window_close_save();
                            }
                            cx.notify();
                            this.show_document_conflict_dialog(
                                document_id.clone(),
                                false,
                                window,
                                cx,
                            );
                        }
                    }
                    Ok(SaveOutcome::RemoteDeleted(pending)) => {
                        let is_window_close_save =
                            this.window_close_save_current.as_deref() == Some(document_id.as_str());
                        if let Some(document) = this.documents.get_mut(&document_id)
                            && document.operation_id == operation_id
                        {
                            document.save_state = SaveState::Conflict;
                            document.pending_save = Some(pending);
                            document.connection_state = DocumentConnectionState::Online;
                            if is_window_close_save {
                                this.cancel_window_close_save();
                            }
                            cx.notify();
                            this.show_document_conflict_dialog(
                                document_id.clone(),
                                true,
                                window,
                                cx,
                            );
                        }
                    }
                    Ok(SaveOutcome::OutcomeUnknown(error)) => this.apply_document_event(
                        DocumentEvent::SaveOutcomeUnknown {
                            document_id,
                            operation_id,
                            error,
                        },
                        window,
                        cx,
                    ),
                    Err(error) => {
                        let offline = crate::document::remote::is_connection_closed(&error);
                        this.apply_document_event(
                            DocumentEvent::SaveFailed {
                                document_id,
                                operation_id,
                                error: format!("{error:#}"),
                                offline,
                            },
                            window,
                            cx,
                        )
                    }
                });
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn force_overwrite_document(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(&document_id) else {
            return;
        };
        let Some(pending) = document.pending_save.take() else {
            return;
        };
        let backend = document.backend.clone();
        let operation_id = pending.operation_id.clone();
        let revision = pending.revision;
        let path = pending.path;
        let bytes = pending.bytes;
        let permissions = pending.opened_metadata.permissions;
        document.save_state = SaveState::Saving;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = backend
                .write_atomic(&path, bytes.clone(), permissions, &operation_id)
                .await;
            let event = match result {
                Ok(metadata) => DocumentEvent::Saved {
                    document_id,
                    operation_id,
                    revision,
                    metadata,
                    original_hash: Sha256::digest(&bytes).into(),
                },
                Err(error) => match write_failure_disposition(&error) {
                    WriteFailureDisposition::OutcomeUnknown => DocumentEvent::SaveOutcomeUnknown {
                        document_id,
                        operation_id,
                        error: format!("connection closed after the save started: {error:#}"),
                    },
                    WriteFailureDisposition::Failed => DocumentEvent::SaveFailed {
                        document_id,
                        operation_id,
                        error: format!("{error:#}"),
                        offline: false,
                    },
                },
            };
            let _ = gpui::AsyncWindowContext::update(cx, |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.apply_document_event(event, window, cx);
                });
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn reload_document(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(&document_id) else {
            return;
        };
        let operation_id = Uuid::new_v4().to_string();
        let backend = document.backend.clone();
        let path = document.key.remote_path.clone();
        document.operation_id = operation_id.clone();
        document.load_state = LoadState::Loading;
        document.save_state = SaveState::Idle;
        document.close_after_save = false;
        document.pending_save = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let event = match load_remote_document(&backend, &path).await {
                Ok(loaded) => DocumentEvent::Loaded {
                    document_id,
                    operation_id,
                    loaded,
                },
                Err(error) => {
                    let offline = crate::document::remote::is_connection_closed(&error);
                    DocumentEvent::LoadFailed {
                        document_id,
                        operation_id,
                        error: format!("{error:#}"),
                        offline,
                    }
                }
            };
            let _ = gpui::AsyncWindowContext::update(cx, |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.apply_document_event(event, window, cx);
                });
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn load_document_page(
        &mut self,
        document_id: String,
        requested_offset: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(&document_id) else {
            return;
        };
        if document.mode != DocumentMode::PagedReadOnly
            || document.read_only_reason != Some(ReadOnlyReason::LargeFile)
        {
            return;
        }
        let Some(metadata) = document.metadata.as_ref() else {
            return;
        };
        if requested_offset >= metadata.size {
            return;
        }
        let Some(state) = document.large_file.as_mut() else {
            return;
        };
        let operation_id = Uuid::new_v4().to_string();
        state.loading = true;
        state.operation_id = operation_id.clone();
        state.error = None;
        let backend = document.backend.clone();
        let path = document.key.remote_path.clone();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = backend
                .read(
                    &path,
                    Some(ByteRange {
                        offset: requested_offset,
                        length: PAGE_READ_BYTES,
                    }),
                )
                .await
                .and_then(|bytes| normalize_window(&bytes, requested_offset));
            let event = match result {
                Ok(page) => DocumentEvent::PageLoaded {
                    document_id,
                    operation_id,
                    requested_offset,
                    page,
                },
                Err(error) => {
                    let offline = crate::document::remote::is_connection_closed(&error);
                    DocumentEvent::PageFailed {
                        document_id,
                        operation_id,
                        error: format!("{error:#}"),
                        offline,
                    }
                }
            };
            let _ = gpui::AsyncWindowContext::update(cx, |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.apply_document_event(event, window, cx);
                });
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn load_previous_document_page(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self
            .documents
            .get(&document_id)
            .and_then(|document| document.large_file.as_ref())
            .map(|state| state.offset.saturating_sub(PAGE_BYTES as u64))
            .unwrap_or(0);
        self.load_document_page(document_id, offset, window, cx);
    }

    pub(crate) fn load_next_document_page(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self
            .documents
            .get(&document_id)
            .and_then(|document| document.large_file.as_ref())
            .map(|state| state.end_offset.saturating_sub(1));
        if let Some(offset) = offset {
            self.load_document_page(document_id, offset, window, cx);
        }
    }

    pub(crate) fn download_document(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get(&document_id) else {
            return;
        };
        let backend = document.backend.clone();
        let remote_path = document.key.remote_path.clone();
        let path_prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("document_select_download_folder").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            match path_prompt.await {
                Ok(Ok(Some(mut paths))) => {
                    if let Some(folder) = paths.pop() {
                        backend.download(remote_path, folder.to_string_lossy().to_string());
                        this.update(cx, |this, cx| {
                            this.show_transfers_dialog = true;
                            cx.notify();
                        })?;
                    }
                }
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.status =
                            t!("document_download_picker_failed", error = error.to_string())
                                .to_string()
                                .into();
                        cx.notify();
                    })?;
                }
                _ => {}
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn reconnect_document(
        &mut self,
        document_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection_id) = self
            .documents
            .get(&document_id)
            .map(|document| document.key.connection_id.clone())
        else {
            return;
        };
        let Some(handle) = self.sftp_handles.get(&connection_id).cloned() else {
            if let Some(document) = self.documents.get_mut(&document_id) {
                document.connection_state =
                    DocumentConnectionState::Offline(t!("document_sftp_unavailable").to_string());
            }
            cx.notify();
            return;
        };
        self.sftp_reconnect_after_ssh.remove(&connection_id);
        self.mark_sftp_cwd_follow_unavailable(&connection_id);
        let Some(document) = self.documents.get_mut(&document_id) else {
            return;
        };
        document.connection_state = DocumentConnectionState::Reconnecting;
        handle.reconnect_now();
        cx.notify();
    }

    pub(crate) fn cancel_document_conflict(&mut self, document_id: &str, cx: &mut Context<Self>) {
        if let Some(document) = self.documents.get_mut(document_id) {
            document.pending_save = None;
            document.save_state = SaveState::Idle;
            document.close_after_save = false;
            cx.notify();
        }
    }

    pub(crate) fn toggle_document_soft_wrap(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(soft_wrap) = self
            .documents
            .get(&document_id)
            .map(|document| !document.soft_wrap)
        else {
            return;
        };
        self.config.set_editor_soft_wrap(soft_wrap);
        self.save_preferences_background();

        let editors = self
            .documents
            .values_mut()
            .map(|document| {
                document.soft_wrap = soft_wrap;
                (document.editor.clone(), document.large_file_viewer.clone())
            })
            .collect::<Vec<_>>();
        for (editor, viewer) in editors {
            editor.update(cx, |editor, cx| {
                editor.set_soft_wrap(soft_wrap, window, cx);
            });
            viewer.update(cx, |viewer, cx| {
                viewer.set_soft_wrap(soft_wrap, window, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn request_close_document(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get(&document_id) else {
            return;
        };
        match close_decision(document.revisions) {
            CloseDecision::CloseNow => self.close_document_now(&document_id, window, cx),
            CloseDecision::Prompt => {
                self.show_document_close_dialog(document_id, window, cx);
            }
        }
    }

    pub(crate) fn save_and_close_document(
        &mut self,
        document_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(&document_id) else {
            return;
        };
        document.close_after_save = true;
        if matches!(document.save_state, SaveState::Checking | SaveState::Saving) {
            cx.notify();
            return;
        }
        self.save_document(document_id, window, cx);
    }

    pub(crate) fn close_document_now(
        &mut self,
        document_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_detached_document_window(document_id, cx);
        let updated_group = self
            .tab_groups
            .iter_mut()
            .find(|group| group.pane_root.contains_document(document_id))
            .map(|group| {
                let previous_focus = group
                    .pane_root
                    .focused_leaf(&group.focused_pane_path)
                    .cloned();
                let adjacent_path = group.pane_root.remove_document_and_focus(document_id);
                group.focused_pane_path = preserved_focus_path_after_document_removal(
                    &group.pane_root,
                    previous_focus.as_ref(),
                    document_id,
                )
                .or(adjacent_path)
                .or_else(|| group.pane_root.first_leaf_path())
                .unwrap_or_default();
                (
                    group.id.clone(),
                    group.pane_root.clone(),
                    group.focused_pane_path.clone(),
                )
            });
        if let Some((group_id, pane_root, focused_path)) = &updated_group
            && self.active_group.as_deref() == Some(group_id.as_str())
        {
            self.pane_root = pane_root.clone();
            self.focused_pane_path = focused_path.clone();
        }

        let workspace_index = self.workspace_tabs.iter().position(|workspace| {
            matches!(
                workspace,
                WorkspaceTab::RemoteDocument {
                    document_id: existing,
                    ..
                } if existing == document_id
            )
        });
        let removed_workspace_id = workspace_index
            .and_then(|index| self.workspace_tabs.get(index))
            .map(|workspace| workspace.id().to_string());
        if let Some(index) = workspace_index {
            self.workspace_tabs.remove(index);
        }
        self.documents.remove(document_id);

        let empty_group_id = updated_group.as_ref().and_then(|(group_id, pane_root, _)| {
            (pane_root.tab_ids().is_empty() && pane_root.document_ids().is_empty())
                .then(|| group_id.clone())
        });
        if let Some(group_id) = empty_group_id {
            self.sftp_handles.remove(&group_id);
            self.sftp_reconnect_after_ssh.remove(&group_id);
            self.tab_groups.retain(|group| group.id != group_id);
            self.workspace_tabs.retain(|workspace| {
                !matches!(
                    workspace,
                    WorkspaceTab::Session {
                        group_id: workspace_group_id,
                        ..
                    } if workspace_group_id == &group_id
                )
            });
            if self.active_group.as_deref() == Some(group_id.as_str()) {
                self.active_group = None;
                self.active_tab = None;
                self.pane_root = crate::PaneLayout::empty();
                self.focused_pane_path.clear();
                self.pending_sftp_path_sync = None;
            }
        }

        if self.active_workspace_tab == removed_workspace_id {
            match self.pane_root.focused_leaf(&self.focused_pane_path) {
                Some(PaneLeaf::Terminal(tab_id)) => {
                    self.active_tab = Some(tab_id.clone());
                    self.active_workspace_tab = self.active_group.clone();
                    self.focus_handle.focus(window, cx);
                }
                Some(PaneLeaf::Document(next_document_id)) => {
                    let next_document_id = next_document_id.clone();
                    self.active_workspace_tab =
                        self.workspace_tabs
                            .iter()
                            .find_map(|workspace| match workspace {
                                WorkspaceTab::RemoteDocument { id, document_id }
                                    if document_id == &next_document_id =>
                                {
                                    Some(id.clone())
                                }
                                _ => None,
                            });
                    if !self.activate_detached_document(&next_document_id, cx)
                        && let Some(document) = self.documents.get(&next_document_id)
                    {
                        document.editor.focus_handle(cx).focus(window, cx);
                    }
                }
                _ => {
                    let next_index = workspace_index
                        .unwrap_or(0)
                        .min(self.workspace_tabs.len().saturating_sub(1));
                    let next = self.workspace_tabs.get(next_index).cloned();
                    if let Some(next) = next {
                        self.activate_workspace(next.id().to_string(), window, cx);
                    } else {
                        self.active_workspace_tab = None;
                    }
                }
            }
        }
        self.sync_system_tab_to_active_group();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CloseDecision, close_decision, find_existing_document,
        preserved_focus_path_after_document_removal,
    };
    use crate::document::{DocumentMode, DocumentRevisions, LineEnding, SaveState, remote::*};
    use crate::{PaneLayout, PaneLeaf};
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use sha2::{Digest, Sha256};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn close_clean_document_immediately() {
        assert_eq!(
            close_decision(DocumentRevisions::default()),
            CloseDecision::CloseNow
        );
    }

    #[test]
    fn close_dirty_document_requires_confirmation() {
        let mut revisions = DocumentRevisions::default();
        revisions.changed();

        assert_eq!(close_decision(revisions), CloseDecision::Prompt);
    }

    #[test]
    fn closing_an_inactive_document_preserves_the_focused_pane() {
        let mut layout = PaneLayout::Vertical(
            vec![
                PaneLayout::terminal("left"),
                PaneLayout::Leaf(PaneLeaf::Document("document".into())),
                PaneLayout::terminal("right"),
            ],
            0.5,
        );
        let previous_focus = layout.focused_leaf(&[2]).cloned();
        let adjacent_path = layout.remove_document_and_focus("document");
        let focused_path = preserved_focus_path_after_document_removal(
            &layout,
            previous_focus.as_ref(),
            "document",
        )
        .or(adjacent_path)
        .expect("a terminal remains");

        assert_eq!(
            layout.focused_leaf(&focused_path),
            Some(&PaneLeaf::Terminal("right".into()))
        );
    }

    #[derive(Clone)]
    struct ReadBackend {
        metadata: RemoteMetadata,
        bytes: Vec<u8>,
        reads: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    enum StatBehavior {
        Metadata(RemoteMetadata),
        Missing,
    }

    #[derive(Clone)]
    enum WriteBehavior {
        Metadata(RemoteMetadata),
        Failure(String),
        ChannelClosed,
    }

    type WriteRecord = (String, Vec<u8>, Option<u32>);
    type WriteLog = Arc<Mutex<Vec<WriteRecord>>>;

    #[derive(Clone)]
    struct MemoryBackend {
        stat: StatBehavior,
        bytes: Vec<u8>,
        write: WriteBehavior,
        writes: WriteLog,
    }

    impl MemoryBackend {
        fn matching(metadata: RemoteMetadata) -> Self {
            Self {
                stat: StatBehavior::Metadata(metadata.clone()),
                bytes: b"alpha".to_vec(),
                write: WriteBehavior::Metadata(RemoteMetadata {
                    size: metadata.size,
                    mtime: metadata.mtime + 1,
                    permissions: metadata.permissions,
                }),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing_write(metadata: RemoteMetadata, message: &str) -> Self {
            Self {
                stat: StatBehavior::Metadata(metadata),
                bytes: b"alpha".to_vec(),
                write: WriteBehavior::Failure(message.into()),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn channel_closed_write(metadata: RemoteMetadata) -> Self {
            Self {
                stat: StatBehavior::Metadata(metadata),
                bytes: b"alpha".to_vec(),
                write: WriteBehavior::ChannelClosed,
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn missing() -> Self {
            Self {
                stat: StatBehavior::Missing,
                bytes: Vec::new(),
                write: WriteBehavior::Failure("write must not run".into()),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn writes(&self) -> Vec<(String, Vec<u8>, Option<u32>)> {
            self.writes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl RemoteFileBackend for MemoryBackend {
        async fn stat(&self, _path: &str) -> Result<RemoteMetadata> {
            match &self.stat {
                StatBehavior::Metadata(metadata) => Ok(metadata.clone()),
                StatBehavior::Missing => Err(RemoteFileError::NotFound.into()),
            }
        }

        async fn read(&self, _path: &str, _range: Option<ByteRange>) -> Result<Vec<u8>> {
            Ok(self.bytes.clone())
        }

        async fn write_atomic(
            &self,
            path: &str,
            bytes: Vec<u8>,
            permissions: Option<u32>,
            _operation_id: &str,
        ) -> Result<RemoteMetadata> {
            self.writes
                .lock()
                .unwrap()
                .push((path.into(), bytes, permissions));
            match &self.write {
                WriteBehavior::Metadata(metadata) => Ok(metadata.clone()),
                WriteBehavior::Failure(message) => Err(anyhow!(message.clone())),
                WriteBehavior::ChannelClosed => Err(RemoteFileError::ChannelClosed.into()),
            }
        }
    }

    fn opened_metadata() -> RemoteMetadata {
        RemoteMetadata {
            size: 5,
            mtime: 20,
            permissions: Some(0o100644),
        }
    }

    fn opened_hash() -> [u8; 32] {
        Sha256::digest(b"alpha").into()
    }

    #[async_trait]
    impl RemoteFileBackend for ReadBackend {
        async fn stat(&self, _path: &str) -> Result<RemoteMetadata> {
            Ok(self.metadata.clone())
        }

        async fn read(&self, _path: &str, _range: Option<ByteRange>) -> Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.bytes.clone())
        }

        async fn write_atomic(
            &self,
            _path: &str,
            _bytes: Vec<u8>,
            _permissions: Option<u32>,
            _operation_id: &str,
        ) -> Result<RemoteMetadata> {
            Err(anyhow!("not used by load tests"))
        }
    }

    #[test]
    fn finds_duplicate_only_on_the_same_connection_and_normalized_path() {
        let first = RemoteDocumentKey::new("connection-a", "/etc/./app.conf");
        let second = RemoteDocumentKey::new("connection-b", "/etc/app.conf");
        let documents = [("document-a", first), ("document-b", second)];
        let requested = RemoteDocumentKey::new("connection-a", "/etc/tmp/../app.conf");

        assert_eq!(
            find_existing_document(documents.iter().map(|(id, key)| (*id, key)), &requested,),
            Some("document-a")
        );
        assert_eq!(
            find_existing_document(
                documents.iter().map(|(id, key)| (*id, key)),
                &RemoteDocumentKey::new("connection-c", "/etc/app.conf"),
            ),
            None
        );
    }

    #[test]
    fn completed_save_only_reports_saved_when_revision_is_clean() {
        assert_eq!(super::save_state_after_completion(false), SaveState::Saved);
        assert_eq!(super::save_state_after_completion(true), SaveState::Idle);
    }

    #[test]
    fn classifies_closed_write_as_unknown_outcome() {
        let closed = anyhow::Error::new(RemoteFileError::ChannelClosed);
        assert_eq!(
            super::write_failure_disposition(&closed),
            super::WriteFailureDisposition::OutcomeUnknown
        );

        let failed = anyhow!("disk full");
        assert_eq!(
            super::write_failure_disposition(&failed),
            super::WriteFailureDisposition::Failed
        );
    }

    #[tokio::test]
    async fn loads_editable_bom_crlf_text_with_detected_language() {
        let reads = Arc::new(AtomicUsize::new(0));
        let backend = ReadBackend {
            metadata: RemoteMetadata {
                size: 16,
                mtime: 20,
                permissions: Some(0o100644),
            },
            bytes: b"\xef\xbb\xbfalpha\r\nbeta\r\n".to_vec(),
            reads: reads.clone(),
        };

        let loaded = super::load_remote_document(&backend, "/etc/app.yaml")
            .await
            .unwrap();

        assert_eq!(loaded.mode, DocumentMode::Editable);
        assert_eq!(loaded.text, "alpha\nbeta\n");
        assert_eq!(loaded.line_ending, LineEnding::CrLf);
        assert!(loaded.had_bom);
        assert_eq!(loaded.language, "yaml");
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn metadata_only_files_are_not_read() {
        let reads = Arc::new(AtomicUsize::new(0));
        let backend = ReadBackend {
            metadata: RemoteMetadata {
                size: 100 * crate::document::MIB + 1,
                mtime: 20,
                permissions: Some(0o100644),
            },
            bytes: Vec::new(),
            reads: reads.clone(),
        };

        let loaded = super::load_remote_document(&backend, "/var/log/huge.log")
            .await
            .unwrap();

        assert_eq!(loaded.mode, DocumentMode::MetadataOnly);
        assert!(loaded.text.is_empty());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn editable_document_growth_is_rejected_after_read() {
        let reads = Arc::new(AtomicUsize::new(0));
        let backend = ReadBackend {
            metadata: RemoteMetadata {
                size: 1,
                mtime: 20,
                permissions: Some(0o100644),
            },
            bytes: vec![b'x'; crate::document::EDITABLE_MAX_BYTES as usize + 1],
            reads: reads.clone(),
        };

        let error = super::load_remote_document(&backend, "/tmp/growing.txt")
            .await
            .expect_err("growth beyond the editable limit must fail");

        assert!(error.to_string().contains("editable size limit"));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn save_success_marks_only_the_saved_revision_clean() {
        let backend = MemoryBackend::matching(opened_metadata());
        let mut revisions = DocumentRevisions::default();
        revisions.changed();
        let saved_revision = revisions.begin_save();

        let outcome = super::run_save_check(
            &backend,
            super::SaveRequest::new(
                "/etc/app.conf",
                b"alpha".to_vec(),
                opened_metadata(),
                opened_hash(),
                saved_revision,
                "operation-1",
            ),
        )
        .await
        .unwrap();
        revisions.changed();
        let super::SaveOutcome::Saved {
            revision, metadata, ..
        } = outcome
        else {
            panic!("expected save")
        };
        revisions.finish_save(revision);

        assert_eq!(metadata.size, 5);
        assert!(
            revisions.is_dirty(),
            "typing after the snapshot must remain dirty"
        );
        assert_eq!(
            backend.writes(),
            vec![("/etc/app.conf".into(), b"alpha".to_vec(), Some(0o100644))]
        );
    }

    #[tokio::test]
    async fn remote_metadata_change_requires_a_conflict_choice() {
        let backend = MemoryBackend::matching(RemoteMetadata {
            size: 8,
            mtime: 21,
            permissions: Some(0o100644),
        });
        let outcome = super::run_save_check(
            &backend,
            super::SaveRequest::new(
                "/etc/app.conf",
                b"local".to_vec(),
                opened_metadata(),
                opened_hash(),
                1,
                "operation-1",
            ),
        )
        .await
        .unwrap();

        let super::SaveOutcome::Conflict(pending) = outcome else {
            panic!("expected conflict")
        };
        assert_eq!(pending.bytes, b"local");
        assert_eq!(pending.current_metadata.unwrap().mtime, 21);
        assert!(backend.writes().is_empty());
    }

    #[tokio::test]
    async fn same_size_and_mtime_content_change_requires_a_conflict_choice() {
        let mut backend = MemoryBackend::matching(opened_metadata());
        backend.bytes = b"bravo".to_vec();

        let outcome = super::run_save_check(
            &backend,
            super::SaveRequest::new(
                "/etc/app.conf",
                b"local".to_vec(),
                opened_metadata(),
                opened_hash(),
                1,
                "operation-1",
            ),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, super::SaveOutcome::Conflict(_)));
        assert!(backend.writes().is_empty());
    }

    #[tokio::test]
    async fn save_failure_preserves_text_and_dirty_state() {
        let backend = MemoryBackend::failing_write(opened_metadata(), "disk full");
        let request = super::SaveRequest::new(
            "/etc/app.conf",
            b"local".to_vec(),
            opened_metadata(),
            opened_hash(),
            1,
            "operation-1",
        );
        let error = super::run_save_check(&backend, request.clone())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("disk full"));
        assert_eq!(request.bytes, b"local");
        assert_eq!(request.revision, 1);
        assert_eq!(backend.writes().len(), 1);
    }

    #[tokio::test]
    async fn interrupted_write_reports_unknown_outcome_without_retrying() {
        let backend = MemoryBackend::channel_closed_write(opened_metadata());
        let outcome = super::run_save_check(
            &backend,
            super::SaveRequest::new(
                "/etc/app.conf",
                b"local".to_vec(),
                opened_metadata(),
                opened_hash(),
                1,
                "operation-1",
            ),
        )
        .await
        .unwrap();

        let super::SaveOutcome::OutcomeUnknown(error) = outcome else {
            panic!("expected unknown save outcome")
        };
        assert!(error.contains("connection"));
        assert_eq!(backend.writes().len(), 1);
    }

    #[tokio::test]
    async fn remote_delete_requires_recreate_confirmation() {
        let backend = MemoryBackend::missing();
        let outcome = super::run_save_check(
            &backend,
            super::SaveRequest::new(
                "/etc/app.conf",
                b"local".to_vec(),
                opened_metadata(),
                opened_hash(),
                1,
                "operation-1",
            ),
        )
        .await
        .unwrap();

        let super::SaveOutcome::RemoteDeleted(pending) = outcome else {
            panic!("expected deletion")
        };
        assert_eq!(pending.bytes, b"local");
        assert_eq!(pending.opened_metadata, opened_metadata());
        assert!(backend.writes().is_empty());
    }
}
