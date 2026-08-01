use gpui::{Context, Entity, SharedString};
use gpui_component::input::InputState;
use rust_i18n::t;

use crate::{
    Ashell,
    session::config::SyncConnectionSnapshot,
    sync::{
        self, DecodedSyncPayload, SyncBackendCredentials, SyncCredentials, SyncError,
        SyncOperationSnapshot, SyncPayloadV2, SyncPreview, SyncResult, SyncTargetId,
    },
    terminal::BackendEvent,
};

pub(crate) struct PendingSyncDownload {
    pub(crate) payload: DecodedSyncPayload,
    pub(crate) preview: SyncPreview,
    pub(crate) etag: Option<String>,
    pub(crate) target_id: SyncTargetId,
    pub(crate) credentials: SyncCredentials,
    pub(crate) connection: SyncConnectionSnapshot,
}

pub(crate) struct PendingSyncUpload {
    pub(crate) credentials: SyncCredentials,
    pub(crate) payload: SyncPayloadV2,
    pub(crate) target_id: SyncTargetId,
    pub(crate) connection: SyncConnectionSnapshot,
}

#[derive(Default)]
pub(crate) struct SyncUiState {
    pub(crate) pending_sync_download: Option<PendingSyncDownload>,
    pub(crate) pending_sync_upload_conflict: Option<PendingSyncUpload>,
}

#[cfg(test)]
pub(crate) fn reduce_download_result(
    state: &mut SyncUiState,
    operation: SyncOperationSnapshot,
    result: Result<(DecodedSyncPayload, Option<String>), crate::sync::SyncError>,
) {
    reduce_download_result_with_preview(state, operation, result, DecodedSyncPayload::preview);
}

fn reduce_download_result_against(
    state: &mut SyncUiState,
    operation: SyncOperationSnapshot,
    result: Result<(DecodedSyncPayload, Option<String>), crate::sync::SyncError>,
    local_config: &crate::session::config::ConfigFile,
) {
    reduce_download_result_with_preview(state, operation, result, |payload| {
        payload.preview_against(local_config)
    });
}

fn reduce_download_result_with_preview(
    state: &mut SyncUiState,
    operation: SyncOperationSnapshot,
    result: Result<(DecodedSyncPayload, Option<String>), crate::sync::SyncError>,
    preview: impl FnOnce(&DecodedSyncPayload) -> SyncPreview,
) {
    state.pending_sync_download = result.ok().map(|(payload, etag)| PendingSyncDownload {
        preview: preview(&payload),
        payload,
        etag,
        target_id: operation.target_id,
        credentials: operation.credentials,
        connection: operation.connection,
    });
}

pub(crate) fn cancel_sync_preview(state: &mut SyncUiState) {
    state.pending_sync_download = None;
}

pub(crate) fn cancel_sync_conflict(state: &mut SyncUiState) {
    state.pending_sync_upload_conflict = None;
}

pub(crate) fn invalidate_pending_sync_actions(state: &mut SyncUiState) {
    state.pending_sync_download = None;
    state.pending_sync_upload_conflict = None;
}

pub(crate) fn pending_sync_conflict_download_snapshot(
    state: &SyncUiState,
) -> Option<SyncOperationSnapshot> {
    state
        .pending_sync_upload_conflict
        .as_ref()
        .map(|pending| SyncOperationSnapshot {
            credentials: pending.credentials.clone(),
            target_id: pending.target_id.clone(),
            connection: pending.connection.clone(),
        })
}

pub(crate) fn pending_sync_conflict_upload_snapshot(
    state: &SyncUiState,
    current_operation: &SyncOperationSnapshot,
) -> Result<SyncOperationSnapshot, SyncError> {
    let pending = state
        .pending_sync_upload_conflict
        .as_ref()
        .ok_or_else(|| SyncError::InvalidInput("no pending upload conflict".to_string()))?;
    if pending.target_id != current_operation.target_id
        || !pending
            .connection
            .matches_remote(&current_operation.connection)
    {
        return Err(SyncError::InvalidInput(
            "sync conflict target changed".to_string(),
        ));
    }

    Ok(SyncOperationSnapshot {
        credentials: pending.credentials.clone(),
        target_id: pending.target_id.clone(),
        connection: current_operation.connection.clone(),
    })
}

struct PreviousSyncCredentials {
    encryption_password: Option<crate::sync::SecretString>,
    r2_secret: Option<crate::sync::SecretString>,
}

fn load_previous_sync_credentials<S: crate::sync::SyncCredentialStore + ?Sized>(
    store: &S,
    target_id: &SyncTargetId,
    credentials: &SyncCredentials,
) -> Result<PreviousSyncCredentials, SyncError> {
    let encryption_password = store.load_encryption_password(target_id)?;
    let r2_secret = if matches!(&credentials.backend, SyncBackendCredentials::R2 { .. }) {
        store.load_r2_secret(target_id)?
    } else {
        None
    };
    Ok(PreviousSyncCredentials {
        encryption_password,
        r2_secret,
    })
}

fn restore_previous_sync_credentials<S: crate::sync::SyncCredentialStore + ?Sized>(
    store: &S,
    target_id: &SyncTargetId,
    credentials: &SyncCredentials,
    previous: &PreviousSyncCredentials,
) -> Result<(), SyncError> {
    match &previous.encryption_password {
        Some(password) => store.store_encryption_password(target_id, password)?,
        None => store.delete_encryption_password(target_id)?,
    }
    if matches!(&credentials.backend, SyncBackendCredentials::R2 { .. }) {
        match &previous.r2_secret {
            Some(secret) => store.store_r2_secret(target_id, secret)?,
            None => store.delete_r2_secret(target_id)?,
        }
    }
    Ok(())
}

pub(crate) fn persist_success_credentials<S: crate::sync::SyncCredentialStore + ?Sized>(
    store: &S,
    target_id: &SyncTargetId,
    credentials: &SyncCredentials,
    remember_encryption_password: bool,
) -> Result<(), crate::sync::SyncError> {
    let previous_encryption_password = store.load_encryption_password(target_id)?;
    if remember_encryption_password {
        store.store_encryption_password(
            target_id,
            &crate::sync::SecretString::new(credentials.encryption_password.clone()),
        )?;
    } else {
        store.delete_encryption_password(target_id)?;
    }

    if let SyncBackendCredentials::R2 {
        secret_access_key, ..
    } = &credentials.backend
        && let Err(error) = store.store_r2_secret(target_id, secret_access_key)
    {
        match previous_encryption_password {
            Some(previous) => {
                let _ = store.store_encryption_password(target_id, &previous);
            }
            None => {
                let _ = store.delete_encryption_password(target_id);
            }
        }
        return Err(error);
    }

    Ok(())
}

pub(crate) fn forget_encryption_passwords<S: crate::sync::SyncCredentialStore + ?Sized>(
    store: &S,
    saved_target: Option<&SyncTargetId>,
    current_target: &SyncTargetId,
) -> Result<(), crate::sync::SyncError> {
    if let Some(saved_target) = saved_target {
        store.delete_encryption_password(saved_target)?;
    }
    if saved_target != Some(current_target) {
        store.delete_encryption_password(current_target)?;
    }
    Ok(())
}

fn disable_remember_encryption_password<S: crate::sync::SyncCredentialStore + ?Sized>(
    store: &S,
    config: &mut crate::session::config::ConfigStore,
    saved_target: Option<&SyncTargetId>,
    current_target: &SyncTargetId,
) -> Result<(), crate::sync::SyncError> {
    forget_encryption_passwords(store, saved_target, current_target)?;
    config.set_sync_remember_encryption_password(false);
    Ok(())
}

pub(crate) fn apply_pending_sync_download<S: crate::sync::SyncCredentialStore + ?Sized>(
    state: &mut SyncUiState,
    current_connection: &SyncConnectionSnapshot,
    config: &mut crate::session::config::ConfigStore,
    credential_store: &S,
) -> Result<(), crate::sync::SyncError> {
    let pending = state
        .pending_sync_download
        .as_ref()
        .ok_or_else(|| crate::sync::SyncError::InvalidInput("no pending download".to_string()))?;
    if pending.target_id != current_connection.target_id()
        || !pending.connection.matches_remote(current_connection)
    {
        return Err(crate::sync::SyncError::InvalidInput(
            "sync preview target changed".to_string(),
        ));
    }

    let pending = state.pending_sync_download.take().unwrap();
    let connection = current_connection.clone();
    let previous_credentials = match load_previous_sync_credentials(
        credential_store,
        &pending.target_id,
        &pending.credentials,
    ) {
        Ok(previous) => previous,
        Err(error) => {
            state.pending_sync_download = Some(pending);
            return Err(error);
        }
    };
    if let Err(error) = persist_success_credentials(
        credential_store,
        &pending.target_id,
        &pending.credentials,
        connection.remember_encryption_password(),
    ) {
        state.pending_sync_download = Some(pending);
        return Err(error);
    }
    if let Err(error) = config.apply_decoded_sync_payload_with_connection(
        &pending.payload,
        &connection,
        pending.etag.clone(),
    ) {
        let rollback_result = restore_previous_sync_credentials(
            credential_store,
            &pending.target_id,
            &pending.credentials,
            &previous_credentials,
        );
        state.pending_sync_download = Some(pending);
        rollback_result?;
        return Err(crate::sync::SyncError::LocalSave(format!("{error:#}")));
    }
    state.pending_sync_upload_conflict = None;
    Ok(())
}

pub(crate) fn reduce_upload_result(
    state: &mut SyncUiState,
    operation: SyncOperationSnapshot,
    payload: SyncPayloadV2,
    result: Result<Option<String>, crate::sync::SyncError>,
) -> Option<SyncOperationSnapshot> {
    match result {
        Ok(_) => {
            state.pending_sync_upload_conflict = None;
            Some(operation)
        }
        Err(crate::sync::SyncError::Conflict) => {
            state.pending_sync_upload_conflict = Some(PendingSyncUpload {
                credentials: operation.credentials,
                payload,
                target_id: operation.target_id,
                connection: operation.connection,
            });
            None
        }
        Err(_) => {
            state.pending_sync_upload_conflict = None;
            None
        }
    }
}

pub(crate) enum SyncCompletionStatus {
    ConnectionTested(crate::sync::RemoteObjectState),
    UploadComplete,
    DownloadPending,
    UploadConflict,
    Failed(crate::sync::SyncError),
    LocalStateSaveFailed,
    CredentialStoreFailed,
}

pub(crate) fn sync_error_message(error: &SyncError) -> String {
    match error {
        SyncError::InvalidInput(_) => t!("sync_error_invalid_input").to_string(),
        SyncError::Network(_) => t!("sync_error_network").to_string(),
        SyncError::Timeout => t!("sync_error_timeout").to_string(),
        SyncError::Unauthorized => t!("sync_error_unauthorized").to_string(),
        SyncError::NotFound => t!("sync_error_not_found").to_string(),
        SyncError::Conflict => t!("sync_error_conflict").to_string(),
        SyncError::PayloadTooLarge { .. } => t!("sync_error_payload_too_large").to_string(),
        SyncError::DecryptFailed => t!("sync_error_decrypt").to_string(),
        SyncError::InvalidPayload(_) => t!("sync_error_invalid_payload").to_string(),
        SyncError::CredentialStore(_) => t!("sync_error_credential_store").to_string(),
        SyncError::LocalSave(_) => t!("sync_error_local_save").to_string(),
    }
}

pub(crate) fn handle_sync_result<S: crate::sync::SyncCredentialStore + ?Sized>(
    state: &mut SyncUiState,
    config: &mut crate::session::config::ConfigStore,
    credential_store: &S,
    completion: SyncResult,
) -> SyncCompletionStatus {
    match completion {
        SyncResult::Tested { result, .. } => match result {
            Ok(remote_state) => SyncCompletionStatus::ConnectionTested(remote_state),
            Err(error) => SyncCompletionStatus::Failed(error),
        },
        SyncResult::Uploaded {
            operation,
            payload,
            result,
        } => match result {
            Ok(etag) => {
                state.pending_sync_upload_conflict = None;
                if state
                    .pending_sync_download
                    .as_ref()
                    .is_some_and(|pending| pending.target_id == operation.target_id)
                {
                    state.pending_sync_download = None;
                }
                let previous_credentials = match load_previous_sync_credentials(
                    credential_store,
                    &operation.target_id,
                    &operation.credentials,
                ) {
                    Ok(previous) => previous,
                    Err(_) => return SyncCompletionStatus::CredentialStoreFailed,
                };
                if persist_success_credentials(
                    credential_store,
                    &operation.target_id,
                    &operation.credentials,
                    operation.connection.remember_encryption_password(),
                )
                .is_err()
                {
                    return SyncCompletionStatus::CredentialStoreFailed;
                }
                if config
                    .persist_sync_connection(&operation.connection, etag)
                    .is_err()
                {
                    if restore_previous_sync_credentials(
                        credential_store,
                        &operation.target_id,
                        &operation.credentials,
                        &previous_credentials,
                    )
                    .is_err()
                    {
                        SyncCompletionStatus::CredentialStoreFailed
                    } else {
                        SyncCompletionStatus::LocalStateSaveFailed
                    }
                } else {
                    SyncCompletionStatus::UploadComplete
                }
            }
            Err(crate::sync::SyncError::Conflict) => {
                let _ = reduce_upload_result(
                    state,
                    operation,
                    *payload,
                    Err(crate::sync::SyncError::Conflict),
                );
                SyncCompletionStatus::UploadConflict
            }
            Err(error) => {
                state.pending_sync_upload_conflict = None;
                SyncCompletionStatus::Failed(error)
            }
        },
        SyncResult::Downloaded { operation, result } => match result {
            Ok(downloaded) => {
                reduce_download_result_against(state, operation, Ok(downloaded), &config.cache);
                SyncCompletionStatus::DownloadPending
            }
            Err(error) => {
                reduce_download_result_against(state, operation, Err(error.clone()), &config.cache);
                SyncCompletionStatus::Failed(error)
            }
        },
    }
}

pub(crate) fn handle_cancel_sync_preview<S: crate::sync::SyncCredentialStore + ?Sized>(
    state: &mut SyncUiState,
    _credential_store: &S,
) {
    cancel_sync_preview(state);
}

impl Ashell {
    pub(crate) fn is_sync_input(&self, input: &Entity<InputState>) -> bool {
        input == &self.sync_endpoint_input
            || input == &self.sync_username_input
            || input == &self.sync_webdav_password_input
            || input == &self.sync_s3_endpoint_input
            || input == &self.sync_s3_region_input
            || input == &self.sync_s3_bucket_input
            || input == &self.sync_s3_object_key_input
            || input == &self.sync_s3_access_key_input
            || input == &self.sync_s3_secret_key_input
            || input == &self.sync_s3_session_token_input
            || input == &self.sync_r2_account_id_input
            || input == &self.sync_r2_bucket_input
            || input == &self.sync_r2_object_key_input
            || input == &self.sync_r2_access_key_id_input
            || input == &self.sync_r2_secret_access_key_input
            || input == &self.sync_encryption_password_input
    }

    fn sync_input_value(input: &Entity<InputState>, cx: &Context<Self>) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn sync_operation_snapshot(&self, cx: &Context<Self>) -> SyncOperationSnapshot {
        let remember = self.sync_remember_encryption_password;
        let (backend, connection) = match self.sync_provider.as_str() {
            "s3" => (
                SyncBackendCredentials::S3 {
                    endpoint: Self::sync_input_value(&self.sync_s3_endpoint_input, cx),
                    region: Self::sync_input_value(&self.sync_s3_region_input, cx),
                    bucket: Self::sync_input_value(&self.sync_s3_bucket_input, cx),
                    object_key: Self::sync_input_value(&self.sync_s3_object_key_input, cx),
                    access_key: Self::sync_input_value(&self.sync_s3_access_key_input, cx),
                    secret_key: self.sync_s3_secret_key_input.read(cx).value().to_string(),
                    session_token: self
                        .sync_s3_session_token_input
                        .read(cx)
                        .value()
                        .to_string(),
                },
                SyncConnectionSnapshot::s3(
                    Self::sync_input_value(&self.sync_s3_endpoint_input, cx),
                    Self::sync_input_value(&self.sync_s3_region_input, cx),
                    Self::sync_input_value(&self.sync_s3_bucket_input, cx),
                    Self::sync_input_value(&self.sync_s3_object_key_input, cx),
                    remember,
                ),
            ),
            "r2" => (
                SyncBackendCredentials::R2 {
                    account_id: Self::sync_input_value(&self.sync_r2_account_id_input, cx),
                    bucket: Self::sync_input_value(&self.sync_r2_bucket_input, cx),
                    object_key: Self::sync_input_value(&self.sync_r2_object_key_input, cx),
                    access_key_id: Self::sync_input_value(&self.sync_r2_access_key_id_input, cx),
                    secret_access_key: crate::sync::SecretString::new(
                        self.sync_r2_secret_access_key_input
                            .read(cx)
                            .value()
                            .to_string(),
                    ),
                },
                SyncConnectionSnapshot::r2(
                    Self::sync_input_value(&self.sync_r2_account_id_input, cx),
                    Self::sync_input_value(&self.sync_r2_bucket_input, cx),
                    Self::sync_input_value(&self.sync_r2_object_key_input, cx),
                    Self::sync_input_value(&self.sync_r2_access_key_id_input, cx),
                    remember,
                ),
            ),
            _ => (
                SyncBackendCredentials::WebDav {
                    endpoint: Self::sync_input_value(&self.sync_endpoint_input, cx),
                    username: Self::sync_input_value(&self.sync_username_input, cx),
                    password: self.sync_webdav_password_input.read(cx).value().to_string(),
                },
                SyncConnectionSnapshot::webdav(
                    Self::sync_input_value(&self.sync_endpoint_input, cx),
                    Self::sync_input_value(&self.sync_username_input, cx),
                    remember,
                ),
            ),
        };
        let credentials = SyncCredentials {
            backend,
            encryption_password: self
                .sync_encryption_password_input
                .read(cx)
                .value()
                .to_string(),
        };
        SyncOperationSnapshot::new(credentials, connection)
    }

    fn begin_sync(
        &mut self,
        status: SharedString,
        cx: &mut Context<Self>,
    ) -> Option<SyncOperationSnapshot> {
        if self.sync_in_progress {
            return None;
        }
        let operation = self.sync_operation_snapshot(cx);
        if let Err(error) = sync::validate_credentials(&operation.credentials) {
            self.sync_status =
                format!("{}: {}", t!("sync_failed"), sync_error_message(&error)).into();
            cx.notify();
            return None;
        }
        self.sync_in_progress = true;
        self.sync_status = status;
        cx.notify();
        Some(operation)
    }

    pub(crate) fn set_sync_backend(&mut self, backend: &str, cx: &mut Context<Self>) {
        if self.sync_in_progress {
            return;
        }
        self.sync_provider = match backend {
            "s3" => "s3",
            "r2" => "r2",
            _ => "webdav",
        }
        .to_string();
        invalidate_pending_sync_actions(&mut self.sync_ui_state);
        self.sync_status = t!("sync_not_run").into();
        cx.notify();
    }

    pub(crate) fn set_sync_remember_encryption_password(
        &mut self,
        remember: bool,
        cx: &mut Context<Self>,
    ) {
        if self.sync_in_progress {
            return;
        }

        if remember {
            self.config.set_sync_remember_encryption_password(true);
        } else {
            let saved_target = self.config.sync_target_id();
            let current_target = self.sync_operation_snapshot(cx).target_id;
            let credential_store = crate::sync::PlatformSyncCredentialStore::new();
            if let Err(error) = disable_remember_encryption_password(
                &credential_store,
                &mut self.config,
                saved_target.as_ref(),
                &current_target,
            ) {
                self.sync_status =
                    format!("{}: {}", t!("sync_failed"), sync_error_message(&error)).into();
                cx.notify();
                return;
            }
        }

        self.sync_remember_encryption_password = remember;
        self.save_preferences_background();
        cx.notify();
    }

    pub(crate) fn test_sync_connection(&mut self, cx: &mut Context<Self>) {
        let Some(operation) = self.begin_sync(t!("sync_testing_connection").into(), cx) else {
            return;
        };
        let credentials = operation.credentials.clone();
        let events = self.events_tx.clone();
        self.runtime.spawn(async move {
            let result = sync::test_connection(credentials).await;
            let _ = events.send(BackendEvent::SyncFinished(Box::new(SyncResult::Tested {
                operation,
                result,
            })));
        });
    }

    pub(crate) fn upload_sync_config(&mut self, cx: &mut Context<Self>) {
        let Some(operation) = self.begin_sync(t!("sync_uploading").into(), cx) else {
            return;
        };
        let credentials = operation.credentials.clone();
        let payload = SyncPayloadV2::new(self.config.portable_config());
        let expected_etag = self
            .config
            .sync_etag_for_target(&operation.target_id)
            .map(str::to_string);
        let events = self.events_tx.clone();
        self.runtime.spawn(async move {
            let result = sync::upload(credentials, payload.clone(), expected_etag).await;
            let _ = events.send(BackendEvent::SyncFinished(Box::new(SyncResult::Uploaded {
                operation,
                payload: Box::new(payload),
                result,
            })));
        });
    }

    pub(crate) fn confirm_overwrite_sync_config(&mut self, cx: &mut Context<Self>) {
        if self.sync_in_progress {
            return;
        }
        let current_operation = self.sync_operation_snapshot(cx);
        let operation =
            match pending_sync_conflict_upload_snapshot(&self.sync_ui_state, &current_operation) {
                Ok(operation) => operation,
                Err(error) => {
                    cancel_sync_conflict(&mut self.sync_ui_state);
                    self.sync_status =
                        format!("{}: {}", t!("sync_failed"), sync_error_message(&error)).into();
                    cx.notify();
                    return;
                }
            };
        let Some(pending) = self.sync_ui_state.pending_sync_upload_conflict.as_ref() else {
            return;
        };
        let credentials = operation.credentials.clone();
        let payload = pending.payload.clone();
        self.sync_in_progress = true;
        self.sync_status = t!("sync_uploading").into();
        cx.notify();

        let events = self.events_tx.clone();
        self.runtime.spawn(async move {
            let result = sync::confirm_overwrite(credentials, payload.clone()).await;
            let _ = events.send(BackendEvent::SyncFinished(Box::new(SyncResult::Uploaded {
                operation,
                payload: Box::new(payload),
                result,
            })));
        });
    }

    pub(crate) fn download_sync_config(&mut self, cx: &mut Context<Self>) {
        let Some(operation) = self.begin_sync(t!("sync_downloading").into(), cx) else {
            return;
        };
        self.spawn_sync_download(operation);
    }

    pub(crate) fn download_pending_sync_conflict(&mut self, cx: &mut Context<Self>) {
        if self.sync_in_progress {
            return;
        }
        let Some(operation) = pending_sync_conflict_download_snapshot(&self.sync_ui_state) else {
            return;
        };
        self.sync_in_progress = true;
        self.sync_status = t!("sync_downloading").into();
        cx.notify();
        self.spawn_sync_download(operation);
    }

    fn spawn_sync_download(&self, operation: SyncOperationSnapshot) {
        let credentials = operation.credentials.clone();
        let events = self.events_tx.clone();
        self.runtime.spawn(async move {
            let result = sync::download(credentials).await;
            let _ = events.send(BackendEvent::SyncFinished(Box::new(
                SyncResult::Downloaded { operation, result },
            )));
        });
    }

    pub(crate) fn cancel_sync_preview(&mut self, cx: &mut Context<Self>) {
        let credential_store = crate::sync::PlatformSyncCredentialStore::new();
        handle_cancel_sync_preview(&mut self.sync_ui_state, &credential_store);
        self.sync_status = t!("sync_not_run").into();
        cx.notify();
    }

    pub(crate) fn cancel_sync_conflict(&mut self, cx: &mut Context<Self>) {
        cancel_sync_conflict(&mut self.sync_ui_state);
        self.sync_status = t!("sync_not_run").into();
        cx.notify();
    }

    pub(crate) fn apply_sync_preview(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let current_connection = self.sync_operation_snapshot(cx).connection;
        let credential_store = crate::sync::PlatformSyncCredentialStore::new();
        let coordinator = self.config_writes.clone();
        let result = coordinator.run_exclusive(|| {
            apply_pending_sync_download(
                &mut self.sync_ui_state,
                &current_connection,
                &mut self.config,
                &credential_store,
            )
        });
        match result {
            Ok(()) => {
                self.sync_status = t!("sync_download_complete").into();
                self.apply_loaded_config(window, cx);
            }
            Err(_) if self.sync_ui_state.pending_sync_download.is_none() => {
                self.sync_status = t!("sync_local_applied_credential_store_failed").into();
                self.apply_loaded_config(window, cx);
            }
            Err(error) => {
                self.sync_status =
                    format!("{}: {}", t!("sync_failed"), sync_error_message(&error)).into();
                cx.notify();
            }
        }
    }

    pub(crate) fn handle_sync_finished(&mut self, result: SyncResult) {
        self.sync_in_progress = false;
        let credential_store = crate::sync::PlatformSyncCredentialStore::new();
        let completion = if matches!(&result, SyncResult::Uploaded { result: Ok(_), .. }) {
            let coordinator = self.config_writes.clone();
            coordinator.run_exclusive(|| {
                handle_sync_result(
                    &mut self.sync_ui_state,
                    &mut self.config,
                    &credential_store,
                    result,
                )
            })
        } else {
            handle_sync_result(
                &mut self.sync_ui_state,
                &mut self.config,
                &credential_store,
                result,
            )
        };
        self.sync_status = match completion {
            SyncCompletionStatus::ConnectionTested(remote_state) => match remote_state {
                crate::sync::RemoteObjectState::Exists { .. } => format!(
                    "{}: {}",
                    t!("sync_connection_succeeded"),
                    t!("sync_remote_exists")
                )
                .into(),
                crate::sync::RemoteObjectState::Missing => format!(
                    "{}: {}",
                    t!("sync_connection_succeeded"),
                    t!("sync_remote_missing")
                )
                .into(),
            },
            SyncCompletionStatus::UploadComplete => t!("sync_upload_complete").into(),
            SyncCompletionStatus::DownloadPending => t!("sync_preview_ready").into(),
            SyncCompletionStatus::UploadConflict => t!("sync_conflict_title").into(),
            SyncCompletionStatus::Failed(error) => {
                format!("{}: {}", t!("sync_failed"), sync_error_message(&error)).into()
            }
            SyncCompletionStatus::LocalStateSaveFailed => {
                t!("sync_remote_succeeded_local_state_failed").into()
            }
            SyncCompletionStatus::CredentialStoreFailed => {
                t!("sync_remote_succeeded_credential_store_failed").into()
            }
        };
    }

    pub(crate) fn export_local_config(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let local_config = self.config.cache.clone();
        let file_dialog = rfd::AsyncFileDialog::new()
            .set_file_name("jshell-config.json")
            .add_filter("JSON", &["json"])
            .save_file();

        cx.spawn_in(window, async move |_this, cx| {
            if let Some(file_handle) = file_dialog.await {
                let path = file_handle.path().to_path_buf();
                if let Ok(json_str) = serde_json::to_string_pretty(&local_config) {
                    let _ = cx
                        .background_executor()
                        .spawn(async move {
                            if let Err(err) = std::fs::write(path, json_str) {
                                tracing::error!("failed to export local config: {err:#}");
                            }
                        })
                        .await;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn import_local_config(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let file_dialog = rfd::AsyncFileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file();

        cx.spawn_in(window, async move |this, cx| {
            if let Some(file_handle) = file_dialog.await {
                let path = file_handle.path().to_path_buf();
                let read_result = cx
                    .background_executor()
                    .spawn(async move { std::fs::read_to_string(path) })
                    .await;

                if let Ok(json_str) = read_result
                    && let Ok(config_file) =
                        serde_json::from_str::<crate::session::config::ConfigFile>(&json_str)
                {
                    let _ = gpui::AsyncWindowContext::update(cx, |window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.config.cache = config_file;
                            if let Err(err) = this.save_config_now() {
                                tracing::error!("failed to save imported config: {err:#}");
                            } else {
                                this.apply_loaded_config(window, cx);
                            }
                        });
                    });
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
    };

    use super::*;
    use crate::{
        session::config::{ConfigStore, Session},
        sync::{RemoteObjectState, SecretString, SyncCredentialStore, SyncError},
    };

    #[derive(Default)]
    struct MemoryCredentialStore {
        r2_secrets: RefCell<HashMap<String, SecretString>>,
        encryption_passwords: RefCell<HashMap<String, SecretString>>,
        mutations: RefCell<Vec<&'static str>>,
        fail_writes: bool,
        fail_on_write: Option<usize>,
        write_attempts: Cell<usize>,
    }

    impl MemoryCredentialStore {
        fn failing() -> Self {
            Self {
                fail_writes: true,
                ..Self::default()
            }
        }

        fn failing_on_write(write_number: usize) -> Self {
            Self {
                fail_on_write: Some(write_number),
                ..Self::default()
            }
        }

        fn check_write(&self, operation: &'static str) -> Result<(), SyncError> {
            let write_number = self.write_attempts.get() + 1;
            self.write_attempts.set(write_number);
            if self.fail_writes || self.fail_on_write == Some(write_number) {
                Err(SyncError::CredentialStore(operation))
            } else {
                Ok(())
            }
        }
    }

    impl SyncCredentialStore for MemoryCredentialStore {
        fn load_r2_secret(&self, target: &SyncTargetId) -> Result<Option<SecretString>, SyncError> {
            Ok(self.r2_secrets.borrow().get(target.as_str()).cloned())
        }

        fn store_r2_secret(
            &self,
            target: &SyncTargetId,
            secret: &SecretString,
        ) -> Result<(), SyncError> {
            self.check_write("store-r2-secret")?;
            self.mutations.borrow_mut().push("store-r2-secret");
            self.r2_secrets
                .borrow_mut()
                .insert(target.to_string(), secret.clone());
            Ok(())
        }

        fn delete_r2_secret(&self, target: &SyncTargetId) -> Result<(), SyncError> {
            self.check_write("delete-r2-secret")?;
            self.mutations.borrow_mut().push("delete-r2-secret");
            self.r2_secrets.borrow_mut().remove(target.as_str());
            Ok(())
        }

        fn load_encryption_password(
            &self,
            target: &SyncTargetId,
        ) -> Result<Option<SecretString>, SyncError> {
            Ok(self
                .encryption_passwords
                .borrow()
                .get(target.as_str())
                .cloned())
        }

        fn store_encryption_password(
            &self,
            target: &SyncTargetId,
            password: &SecretString,
        ) -> Result<(), SyncError> {
            self.check_write("store-encryption-password")?;
            self.mutations
                .borrow_mut()
                .push("store-encryption-password");
            self.encryption_passwords
                .borrow_mut()
                .insert(target.to_string(), password.clone());
            Ok(())
        }

        fn delete_encryption_password(&self, target: &SyncTargetId) -> Result<(), SyncError> {
            self.check_write("delete-encryption-password")?;
            self.mutations
                .borrow_mut()
                .push("delete-encryption-password");
            self.encryption_passwords
                .borrow_mut()
                .remove(target.as_str());
            Ok(())
        }
    }

    fn credentials(bucket: &str) -> SyncCredentials {
        SyncCredentials {
            backend: SyncBackendCredentials::R2 {
                account_id: "0123456789abcdef0123456789abcdef".to_string(),
                bucket: bucket.to_string(),
                object_key: "jshell-sync.json".to_string(),
                access_key_id: "access-key-id".to_string(),
                secret_access_key: SecretString::new("secret-access-key".to_string()),
            },
            encryption_password: "encryption-password".to_string(),
        }
    }

    fn target(bucket: &str) -> SyncTargetId {
        SyncTargetId::for_r2(
            "0123456789abcdef0123456789abcdef",
            bucket,
            "jshell-sync.json",
        )
    }

    fn operation(bucket: &str, remember_encryption_password: bool) -> SyncOperationSnapshot {
        SyncOperationSnapshot::new(
            credentials(bucket),
            SyncConnectionSnapshot::r2(
                "0123456789abcdef0123456789abcdef".to_string(),
                bucket.to_string(),
                "jshell-sync.json".to_string(),
                "access-key-id".to_string(),
                remember_encryption_password,
            ),
        )
    }

    fn session(id: &str, host: &str) -> Session {
        let mut session = Session::password(
            host.to_string(),
            22,
            "test-user".to_string(),
            "session-password".to_string(),
        );
        session.id = id.to_string();
        session
    }

    fn decoded_payload(id: &str) -> DecodedSyncPayload {
        let mut remote = ConfigStore::in_memory();
        remote.replace_sessions(vec![session(id, "remote.test")]);
        DecodedSyncPayload::V2(Box::new(SyncPayloadV2::new(remote.portable_config())))
    }

    fn upload_payload() -> SyncPayloadV2 {
        SyncPayloadV2::new(ConfigStore::in_memory().portable_config())
    }

    fn config_snapshot(config: &ConfigStore) -> serde_json::Value {
        serde_json::to_value(&config.cache).unwrap()
    }

    #[test]
    fn failed_download_never_creates_preview_or_changes_config() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let before = config_snapshot(&config);

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Downloaded {
                operation: operation("failed", true),
                result: Err(SyncError::Network("download failed".to_string())),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::Failed(_)));
        assert!(state.pending_sync_download.is_none());
        assert_eq!(config_snapshot(&config), before);
    }

    #[test]
    fn successful_download_creates_preview_without_applying_it() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let before = config_snapshot(&config);

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Downloaded {
                operation: operation("download", true),
                result: Ok((
                    decoded_payload("remote-session"),
                    Some("remote-etag".to_string()),
                )),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::DownloadPending));
        let pending = state.pending_sync_download.as_ref().unwrap();
        assert_eq!(pending.preview.session_count, 1);
        assert_eq!(pending.preview.preference_category_count, 0);
        assert_eq!(pending.etag.as_deref(), Some("remote-etag"));
        assert_eq!(pending.target_id, target("download"));
        assert_eq!(config_snapshot(&config), before);
    }

    #[test]
    fn successful_upload_invalidates_an_older_download_preview() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let sync_operation = operation("same-target", true);
        reduce_download_result(
            &mut state,
            sync_operation.clone(),
            Ok((
                decoded_payload("remote-session"),
                Some("old-etag".to_string()),
            )),
        );
        assert!(state.pending_sync_download.is_some());

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: sync_operation,
                payload: Box::new(upload_payload()),
                result: Ok(Some("new-etag".to_string())),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::UploadComplete));
        assert!(state.pending_sync_download.is_none());
    }

    #[test]
    fn pending_conflict_download_uses_original_operation_snapshot() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let pending_operation = operation("pending", true);
        let mut changed_form_operation = operation("changed-form", false);
        changed_form_operation.credentials.encryption_password = "changed-password".to_string();

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: pending_operation.clone(),
                payload: Box::new(upload_payload()),
                result: Err(SyncError::Conflict),
            },
        );
        assert!(matches!(status, SyncCompletionStatus::UploadConflict));

        let snapshot = pending_sync_conflict_download_snapshot(&state)
            .expect("a pending conflict provides its original download snapshot");

        assert_eq!(snapshot.target_id, pending_operation.target_id);
        assert_ne!(snapshot.target_id, changed_form_operation.target_id);
        assert!(snapshot.connection == pending_operation.connection);
        assert!(snapshot.connection != changed_form_operation.connection);
        assert_eq!(
            snapshot.credentials.encryption_password,
            pending_operation.credentials.encryption_password
        );
        assert_ne!(
            snapshot.credentials.encryption_password,
            changed_form_operation.credentials.encryption_password
        );
        let SyncBackendCredentials::R2 { bucket, .. } = snapshot.credentials.backend else {
            panic!("expected pending R2 credentials");
        };
        assert_eq!(bucket, "pending");
    }

    #[test]
    fn conflict_confirmation_rejects_a_changed_form_target() {
        let mut state = SyncUiState::default();
        let _ = reduce_upload_result(
            &mut state,
            operation("pending", true),
            upload_payload(),
            Err(SyncError::Conflict),
        );

        let error = pending_sync_conflict_upload_snapshot(&state, &operation("changed", false))
            .expect_err("a conflict is bound to the form target that created it");

        assert!(matches!(error, SyncError::InvalidInput(_)));
        assert!(state.pending_sync_upload_conflict.is_some());
    }

    #[test]
    fn cancel_preview_discards_payload_without_changes() {
        let mut state = SyncUiState::default();
        let config = ConfigStore::in_memory();
        let before = config_snapshot(&config);
        reduce_download_result(
            &mut state,
            operation("download", true),
            Ok((
                decoded_payload("remote-session"),
                Some("remote-etag".to_string()),
            )),
        );

        cancel_sync_preview(&mut state);

        assert!(state.pending_sync_download.is_none());
        assert_eq!(config_snapshot(&config), before);
    }

    #[test]
    fn sync_form_change_invalidates_preview_and_conflict_actions() {
        let mut state = SyncUiState::default();
        let pending_operation = operation("pending", true);
        let _ = reduce_upload_result(
            &mut state,
            pending_operation.clone(),
            upload_payload(),
            Err(SyncError::Conflict),
        );
        reduce_download_result(
            &mut state,
            pending_operation,
            Ok((decoded_payload("remote-session"), None)),
        );

        invalidate_pending_sync_actions(&mut state);

        assert!(state.pending_sync_download.is_none());
        assert!(state.pending_sync_upload_conflict.is_none());
    }

    #[test]
    fn preview_from_another_target_cannot_be_applied() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        config.replace_sessions(vec![session("local-session", "local.test")]);
        let before = config_snapshot(&config);
        let _ = reduce_upload_result(
            &mut state,
            operation("upload-conflict", true),
            upload_payload(),
            Err(SyncError::Conflict),
        );
        reduce_download_result(
            &mut state,
            operation("download-a", true),
            Ok((
                decoded_payload("remote-session"),
                Some("remote-etag".to_string()),
            )),
        );
        let current_connection = operation("download-b", false).connection;

        let error = apply_pending_sync_download(
            &mut state,
            &current_connection,
            &mut config,
            &credential_store,
        )
        .expect_err("a preview is bound to the target that downloaded it");

        assert!(matches!(error, SyncError::InvalidInput(_)));
        assert!(state.pending_sync_download.is_some());
        assert!(state.pending_sync_upload_conflict.is_some());
        assert_eq!(config_snapshot(&config), before);
        assert!(credential_store.mutations.borrow().is_empty());
    }

    #[test]
    fn preview_with_changed_r2_access_key_id_cannot_be_applied() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let pending_operation = operation("same-target", true);
        reduce_download_result(
            &mut state,
            pending_operation.clone(),
            Ok((decoded_payload("remote-session"), None)),
        );
        let changed_connection = SyncConnectionSnapshot::r2(
            "0123456789abcdef0123456789abcdef".to_string(),
            "same-target".to_string(),
            "jshell-sync.json".to_string(),
            "different-access-key-id".to_string(),
            false,
        );
        assert_eq!(changed_connection.target_id(), pending_operation.target_id);
        let before = config_snapshot(&config);

        let error = apply_pending_sync_download(
            &mut state,
            &changed_connection,
            &mut config,
            &credential_store,
        )
        .expect_err("a preview is bound to the full remote connection snapshot");

        assert!(matches!(error, SyncError::InvalidInput(_)));
        assert!(state.pending_sync_download.is_some());
        assert_eq!(config_snapshot(&config), before);
        assert!(credential_store.mutations.borrow().is_empty());
    }

    #[test]
    fn successful_pending_download_apply_clears_stale_upload_conflict() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let download_operation = operation("downloaded", true);
        let _ = reduce_upload_result(
            &mut state,
            download_operation.clone(),
            upload_payload(),
            Err(SyncError::Conflict),
        );
        reduce_download_result(
            &mut state,
            download_operation.clone(),
            Ok((decoded_payload("remote-session"), None)),
        );

        apply_pending_sync_download(
            &mut state,
            &download_operation.connection,
            &mut config,
            &credential_store,
        )
        .unwrap();

        assert!(state.pending_sync_download.is_none());
        assert!(state.pending_sync_upload_conflict.is_none());
    }

    #[test]
    fn pending_download_apply_uses_the_current_remember_choice() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let download_operation = operation("downloaded", true);
        credential_store
            .store_encryption_password(
                &download_operation.target_id,
                &SecretString::new("previous-password".to_string()),
            )
            .unwrap();
        credential_store.mutations.borrow_mut().clear();
        reduce_download_result(
            &mut state,
            download_operation.clone(),
            Ok((decoded_payload("remote-session"), None)),
        );
        config.set_sync_remember_encryption_password(false);
        let current_connection = download_operation
            .connection
            .with_remember_encryption_password(false);

        apply_pending_sync_download(
            &mut state,
            &current_connection,
            &mut config,
            &credential_store,
        )
        .unwrap();

        assert!(!config.sync_remember_encryption_password());
        assert_eq!(
            credential_store
                .load_encryption_password(&download_operation.target_id)
                .unwrap(),
            None
        );
    }

    #[test]
    fn local_apply_failure_preserves_download_and_upload_conflict() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let download_operation = operation("downloaded", true);
        let _ = reduce_upload_result(
            &mut state,
            download_operation.clone(),
            upload_payload(),
            Err(SyncError::Conflict),
        );
        reduce_download_result(
            &mut state,
            download_operation.clone(),
            Ok((decoded_payload("remote-session"), None)),
        );
        config.path = std::env::temp_dir()
            .join(format!(
                "jshell-config-sync-missing-{}",
                uuid::Uuid::new_v4()
            ))
            .join("sessions.json");
        let before = config_snapshot(&config);

        let error = apply_pending_sync_download(
            &mut state,
            &download_operation.connection,
            &mut config,
            &credential_store,
        )
        .expect_err("a local persistence failure must keep both pending actions");

        assert!(matches!(error, SyncError::LocalSave(_)));
        assert!(state.pending_sync_download.is_some());
        assert!(state.pending_sync_upload_conflict.is_some());
        assert_eq!(config_snapshot(&config), before);
        assert_eq!(
            credential_store
                .load_r2_secret(&target("downloaded"))
                .unwrap(),
            None
        );
        assert_eq!(
            credential_store
                .load_encryption_password(&target("downloaded"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn upload_conflict_requires_an_explicit_follow_up_action() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();
        let before = config_snapshot(&config);
        let upload_target = target("upload");

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: operation("upload", true),
                payload: Box::new(upload_payload()),
                result: Err(SyncError::Conflict),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::UploadConflict));
        let pending = state.pending_sync_upload_conflict.as_ref().unwrap();
        assert_eq!(pending.target_id, upload_target);
        assert_eq!(config_snapshot(&config), before);
        assert!(credential_store.mutations.borrow().is_empty());

        cancel_sync_conflict(&mut state);

        assert!(state.pending_sync_upload_conflict.is_none());
        assert_eq!(config_snapshot(&config), before);
        assert!(credential_store.mutations.borrow().is_empty());
    }

    #[test]
    fn test_failure_and_cancel_never_write_credentials() {
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let credential_store = MemoryCredentialStore::default();

        let tested = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Tested {
                operation: operation("tested", true),
                result: Ok(RemoteObjectState::Missing),
            },
        );
        assert!(matches!(
            tested,
            SyncCompletionStatus::ConnectionTested(RemoteObjectState::Missing)
        ));
        let failed = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Downloaded {
                operation: operation("failed", true),
                result: Err(SyncError::Network("download failed".to_string())),
            },
        );
        assert!(matches!(failed, SyncCompletionStatus::Failed(_)));
        let pending = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Downloaded {
                operation: operation("cancelled", true),
                result: Ok((decoded_payload("remote-session"), None)),
            },
        );
        assert!(matches!(pending, SyncCompletionStatus::DownloadPending));
        handle_cancel_sync_preview(&mut state, &credential_store);

        assert!(state.pending_sync_download.is_none());
        assert!(credential_store.mutations.borrow().is_empty());
    }

    #[test]
    fn upload_success_and_confirmed_apply_write_credentials() {
        let credential_store = MemoryCredentialStore::default();
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let uploaded = operation("uploaded", true);
        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: uploaded.clone(),
                payload: Box::new(upload_payload()),
                result: Ok(Some("uploaded-etag".to_string())),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::UploadComplete));
        assert_eq!(config.sync_target_id(), Some(uploaded.target_id));
        assert_eq!(config.sync_etag(), Some("uploaded-etag"));
        assert_eq!(credential_store.mutations.borrow().len(), 2);

        credential_store.mutations.borrow_mut().clear();
        let download_operation = operation("downloaded", true);
        reduce_download_result(
            &mut state,
            download_operation.clone(),
            Ok((decoded_payload("remote-session"), None)),
        );
        apply_pending_sync_download(
            &mut state,
            &download_operation.connection,
            &mut config,
            &credential_store,
        )
        .unwrap();

        assert_eq!(config.sessions()[0].id, "remote-session");
        assert_eq!(credential_store.mutations.borrow().len(), 2);
        assert!(state.pending_sync_download.is_none());
    }

    #[test]
    fn upload_credential_failure_keeps_local_config_unchanged() {
        let credential_store = MemoryCredentialStore::failing();
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let uploaded = operation("uploaded", true);
        let before = config_snapshot(&config);

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: uploaded.clone(),
                payload: Box::new(upload_payload()),
                result: Ok(Some("uploaded-etag".to_string())),
            },
        );

        assert!(matches!(
            status,
            SyncCompletionStatus::CredentialStoreFailed
        ));
        assert_eq!(config_snapshot(&config), before);
    }

    #[test]
    fn second_credential_write_failure_restores_previous_password_and_r2_secret() {
        let credential_store = MemoryCredentialStore::failing_on_write(2);
        let target = target("rollback");
        let previous_r2_secret = SecretString::new("previous-r2-secret".to_string());
        let previous_password = SecretString::new("previous-password".to_string());
        credential_store
            .r2_secrets
            .borrow_mut()
            .insert(target.to_string(), previous_r2_secret.clone());
        credential_store
            .encryption_passwords
            .borrow_mut()
            .insert(target.to_string(), previous_password.clone());

        let error =
            persist_success_credentials(&credential_store, &target, &credentials("rollback"), true)
                .expect_err("the second keyring write is injected to fail");

        assert!(matches!(error, SyncError::CredentialStore(_)));
        assert_eq!(
            credential_store.load_r2_secret(&target).unwrap(),
            Some(previous_r2_secret)
        );
        assert_eq!(
            credential_store.load_encryption_password(&target).unwrap(),
            Some(previous_password)
        );
    }

    #[test]
    fn upload_success_persists_connection_and_etag_to_disk() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sessions.json");
        let credential_store = MemoryCredentialStore::default();
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        config.path = path.clone();
        config.save().unwrap();
        let before = std::fs::read(&path).unwrap();
        let uploaded = operation("persisted", true);

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: uploaded.clone(),
                payload: Box::new(upload_payload()),
                result: Ok(Some("persisted-etag".to_string())),
            },
        );

        let after = std::fs::read(&path).unwrap();
        assert!(matches!(status, SyncCompletionStatus::UploadComplete));
        assert_ne!(after, before);
        assert_eq!(config.sync_target_id(), Some(uploaded.target_id));
        assert_eq!(config.sync_etag(), Some("persisted-etag"));
    }

    #[test]
    fn upload_local_save_failure_restores_previous_credentials() {
        let credential_store = MemoryCredentialStore::default();
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        config.path = std::env::temp_dir()
            .join(format!(
                "jshell-config-sync-missing-{}",
                uuid::Uuid::new_v4()
            ))
            .join("sessions.json");
        let uploaded = operation("rollback-local-save", true);
        let previous_r2_secret = SecretString::new("previous-r2-secret".to_string());
        let previous_password = SecretString::new("previous-password".to_string());
        credential_store
            .r2_secrets
            .borrow_mut()
            .insert(uploaded.target_id.to_string(), previous_r2_secret.clone());
        credential_store
            .encryption_passwords
            .borrow_mut()
            .insert(uploaded.target_id.to_string(), previous_password.clone());
        let before = config_snapshot(&config);

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: uploaded.clone(),
                payload: Box::new(upload_payload()),
                result: Ok(Some("uploaded-etag".to_string())),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::LocalStateSaveFailed));
        assert_eq!(config_snapshot(&config), before);
        assert_eq!(
            credential_store
                .load_r2_secret(&uploaded.target_id)
                .unwrap(),
            Some(previous_r2_secret)
        );
        assert_eq!(
            credential_store
                .load_encryption_password(&uploaded.target_id)
                .unwrap(),
            Some(previous_password)
        );
    }

    #[test]
    fn upload_local_save_failure_removes_new_credentials_when_none_existed() {
        let credential_store = MemoryCredentialStore::default();
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        config.path = std::env::temp_dir()
            .join(format!(
                "jshell-config-sync-missing-{}",
                uuid::Uuid::new_v4()
            ))
            .join("sessions.json");
        let uploaded = operation("rollback-new-credentials", true);

        let status = handle_sync_result(
            &mut state,
            &mut config,
            &credential_store,
            SyncResult::Uploaded {
                operation: uploaded.clone(),
                payload: Box::new(upload_payload()),
                result: Ok(Some("uploaded-etag".to_string())),
            },
        );

        assert!(matches!(status, SyncCompletionStatus::LocalStateSaveFailed));
        assert_eq!(
            credential_store
                .load_r2_secret(&uploaded.target_id)
                .unwrap(),
            None
        );
        assert_eq!(
            credential_store
                .load_encryption_password(&uploaded.target_id)
                .unwrap(),
            None
        );
    }

    #[test]
    fn download_credential_failure_keeps_pending_and_config_unchanged() {
        let credential_store = MemoryCredentialStore::failing();
        let mut state = SyncUiState::default();
        let mut config = ConfigStore::in_memory();
        let download_operation = operation("downloaded", true);
        let _ = reduce_upload_result(
            &mut state,
            download_operation.clone(),
            upload_payload(),
            Err(SyncError::Conflict),
        );
        reduce_download_result(
            &mut state,
            download_operation.clone(),
            Ok((decoded_payload("remote-session"), None)),
        );
        let before = config_snapshot(&config);

        let error = apply_pending_sync_download(
            &mut state,
            &download_operation.connection,
            &mut config,
            &credential_store,
        )
        .expect_err("credential failure must abort the local apply");

        assert!(matches!(error, SyncError::CredentialStore(_)));
        assert_eq!(config_snapshot(&config), before);
        assert!(state.pending_sync_download.is_some());
        assert!(state.pending_sync_upload_conflict.is_some());
    }

    #[test]
    fn remember_false_deletes_encryption_password_idempotently() {
        let credential_store = MemoryCredentialStore::default();
        let credentials = credentials("forgotten");
        let target = target("forgotten");
        credential_store
            .store_encryption_password(&target, &SecretString::new("old-password".to_string()))
            .unwrap();
        credential_store.mutations.borrow_mut().clear();

        persist_success_credentials(&credential_store, &target, &credentials, false).unwrap();
        persist_success_credentials(&credential_store, &target, &credentials, false).unwrap();

        assert_eq!(
            credential_store.load_encryption_password(&target).unwrap(),
            None
        );
        assert_eq!(
            credential_store
                .mutations
                .borrow()
                .iter()
                .filter(|operation| **operation == "delete-encryption-password")
                .count(),
            2
        );
    }

    #[test]
    fn disabling_remember_deletes_saved_and_current_targets_and_updates_config_flag() {
        let credential_store = MemoryCredentialStore::default();
        let saved_target = target("saved-target");
        let current_target = target("current-target");
        for target in [&saved_target, &current_target] {
            credential_store
                .store_encryption_password(target, &SecretString::new("old-password".to_string()))
                .unwrap();
        }
        credential_store.mutations.borrow_mut().clear();
        let mut config = ConfigStore::in_memory();
        config.set_sync_remember_encryption_password(true);

        disable_remember_encryption_password(
            &credential_store,
            &mut config,
            Some(&saved_target),
            &current_target,
        )
        .unwrap();

        assert!(!config.sync_remember_encryption_password());
        assert_eq!(
            credential_store
                .load_encryption_password(&saved_target)
                .unwrap(),
            None
        );
        assert_eq!(
            credential_store
                .load_encryption_password(&current_target)
                .unwrap(),
            None
        );
        assert_eq!(
            credential_store
                .mutations
                .borrow()
                .iter()
                .filter(|operation| **operation == "delete-encryption-password")
                .count(),
            2
        );
    }

    #[test]
    fn disabling_remember_deletes_a_shared_saved_and_current_target_once() {
        let credential_store = MemoryCredentialStore::default();
        let shared_target = target("shared-target");
        credential_store
            .store_encryption_password(
                &shared_target,
                &SecretString::new("old-password".to_string()),
            )
            .unwrap();
        credential_store.mutations.borrow_mut().clear();
        let mut config = ConfigStore::in_memory();
        config.set_sync_remember_encryption_password(true);

        disable_remember_encryption_password(
            &credential_store,
            &mut config,
            Some(&shared_target),
            &shared_target,
        )
        .unwrap();

        assert!(!config.sync_remember_encryption_password());
        assert_eq!(
            credential_store
                .mutations
                .borrow()
                .iter()
                .filter(|operation| **operation == "delete-encryption-password")
                .count(),
            1
        );
    }

    #[test]
    fn failed_password_cleanup_keeps_remember_enabled() {
        let credential_store = MemoryCredentialStore::failing();
        let saved_target = target("saved-target");
        let current_target = target("current-target");
        let mut config = ConfigStore::in_memory();
        config.set_sync_remember_encryption_password(true);

        let error = disable_remember_encryption_password(
            &credential_store,
            &mut config,
            Some(&saved_target),
            &current_target,
        )
        .expect_err("credential deletion is injected to fail");

        assert!(matches!(error, SyncError::CredentialStore(_)));
        assert!(config.sync_remember_encryption_password());
    }

    #[test]
    fn operation_snapshot_binds_connection_target_and_redacts_credentials() {
        let connection = crate::session::config::SyncConnectionSnapshot::r2(
            "0123456789abcdef0123456789abcdef".to_string(),
            "snapshot-bucket".to_string(),
            "snapshot.json".to_string(),
            "snapshot-access-key".to_string(),
            true,
        );
        let snapshot = SyncOperationSnapshot::new(credentials("snapshot-bucket"), connection);

        assert_eq!(
            snapshot.target_id,
            SyncTargetId::for_r2(
                "0123456789abcdef0123456789abcdef",
                "snapshot-bucket",
                "snapshot.json",
            )
        );
        let debug = format!("{snapshot:?}");
        assert!(!debug.contains("secret-access-key"));
        assert!(!debug.contains("encryption-password"));
    }

    #[test]
    fn sync_error_messages_never_include_internal_details() {
        let errors = [
            SyncError::InvalidInput("invalid-input-sentinel".to_string()),
            SyncError::Network("network-sentinel".to_string()),
            SyncError::InvalidPayload("payload-sentinel".to_string()),
            SyncError::CredentialStore("credential-store-sentinel"),
            SyncError::LocalSave("local-save-sentinel".to_string()),
        ];

        for error in errors {
            let message = sync_error_message(&error);
            assert!(!message.contains("sentinel"));
        }
    }
}
