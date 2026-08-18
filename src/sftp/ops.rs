use gpui::{Context, PathPromptOptions, Pixels, Point, Window};
use std::collections::HashSet;

use crate::{
    Ashell, SftpContextMenuState,
    sftp::{RemoteEntry, SftpHandle, cwd_follow::parse_terminal_cwd},
    terminal,
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SftpEntryGesture {
    SingleClick,
    DoubleClick,
    Checkbox,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SftpEntryAction {
    Focus,
    OpenFile,
    NavigateDirectory,
    ToggleSelection,
}

pub(crate) fn sftp_entry_action(is_dir: bool, gesture: SftpEntryGesture) -> SftpEntryAction {
    match gesture {
        SftpEntryGesture::SingleClick => SftpEntryAction::Focus,
        SftpEntryGesture::DoubleClick if is_dir => SftpEntryAction::NavigateDirectory,
        SftpEntryGesture::DoubleClick => SftpEntryAction::OpenFile,
        SftpEntryGesture::Checkbox => SftpEntryAction::ToggleSelection,
    }
}

pub(crate) fn begin_sftp_delete(pending: &mut HashSet<String>, paths: &[String]) -> bool {
    if paths.is_empty() || paths.iter().any(|path| pending.contains(path)) {
        return false;
    }
    pending.extend(paths.iter().cloned());
    true
}

pub(crate) fn finish_sftp_delete(pending: &mut HashSet<String>, paths: &[String]) {
    for path in paths {
        pending.remove(path);
    }
}

pub(crate) fn apply_sftp_delete_result(
    pending: &mut HashSet<String>,
    selected: &mut HashSet<String>,
    attempted: &[String],
    deleted: &[String],
) {
    finish_sftp_delete(pending, attempted);
    for path in deleted {
        selected.remove(path);
    }
}

impl Ashell {
    pub(crate) fn delete_sftp_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) -> bool {
        let Some(handle) = self.active_sftp_handle().cloned() else {
            return false;
        };
        let Some(sftp) = self.active_sftp_mut() else {
            return false;
        };
        if !begin_sftp_delete(&mut sftp.deleting_entries, &paths) {
            return false;
        }
        if !handle.send(crate::sftp::SftpCommand::DeletePaths(paths.clone())) {
            finish_sftp_delete(&mut sftp.deleting_entries, &paths);
            sftp.set_status(rust_i18n::t!("sftp_command_channel_closed").to_string());
            cx.notify();
            return false;
        }
        cx.notify();
        true
    }

    pub(crate) fn active_sftp(&self) -> Option<&terminal::SftpUiState> {
        self.active_group
            .as_ref()
            .and_then(|id| self.tab_groups.iter().find(|g| &g.id == id))
            .and_then(|g| g.sftp.as_ref())
    }

    pub(crate) fn active_sftp_mut(&mut self) -> Option<&mut terminal::SftpUiState> {
        let active_id = self.active_group.clone()?;
        self.tab_groups
            .iter_mut()
            .find(|g| g.id == active_id)
            .and_then(|g| g.sftp.as_mut())
    }

    pub(crate) fn active_sftp_handle(&self) -> Option<&SftpHandle> {
        self.active_group
            .as_ref()
            .and_then(|id| self.sftp_handles.get(id))
    }

    pub(crate) fn reconnect_active_sftp(&mut self, cx: &mut Context<Self>) {
        if let Some(group_id) = self.active_group.clone()
            && let Some(handle) = self.sftp_handles.get(&group_id).cloned()
        {
            self.sftp_reconnect_after_ssh.remove(&group_id);
            self.mark_sftp_cwd_follow_unavailable(&group_id);
            handle.reconnect_now();
            if let Some(sftp) = self.active_sftp_mut() {
                sftp.set_status(rust_i18n::t!("sftp_reconnecting").to_string());
            }
            cx.notify();
        }
    }

    pub(crate) fn navigate_sftp(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(active_tab) = self.active_tab.as_deref()
            && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == active_tab)
        {
            tab.cwd_follow_on_next_title = false;
        }
        let (ready, generation) = if let Some(sftp) = self.active_sftp_mut() {
            sftp.cwd_follow.cancel_for_manual_navigation();
            (sftp.cwd_follow.is_ready(), sftp.generation)
        } else {
            return;
        };

        tracing::info!("[sftp] navigating to directory: '{}'", path);
        if ready && let Some(handle) = self.active_sftp_handle().cloned() {
            handle.list_dir(path.clone(), generation);
        }
        let path_for_input = self.active_sftp_mut().map(|sftp| {
            sftp.current_path = path;
            sftp.path_initialized = true;
            sftp.selected_path = None;
            sftp.preview = None;
            sftp.selected_entries.clear();
            sftp.current_path.clone()
        });
        if let Some(path) = path_for_input {
            self.pending_sftp_path_sync = Some(path);
        }
        cx.notify();
    }

    pub(crate) fn sync_cwd_from_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_id) = self.active_tab.clone() else {
            return;
        };
        let Some(home_dir) = self
            .tab_groups
            .iter()
            .find(|group| group.pane_root.contains(&active_id))
            .and_then(|group| group.sftp.as_ref())
            .map(|sftp| sftp.home_dir.as_str())
        else {
            return;
        };
        let path = self
            .tabs
            .iter()
            .find(|tab| tab.id == active_id)
            .and_then(|tab| {
                tab.remote_cwd
                    .clone()
                    .or_else(|| parse_terminal_cwd(&tab.dynamic_title, home_dir))
            });

        if let Some(path) = path {
            self.navigate_sftp(path, cx);
        }
    }

    pub(crate) fn focus_sftp_entry(&mut self, entry: &RemoteEntry, cx: &mut Context<Self>) {
        self.mark_sftp_entry_selected(&entry.full_path, cx);
        if let Some(handle) = self.active_sftp_handle() {
            handle.preview(entry.full_path.clone());
        }
    }

    pub(crate) fn mark_sftp_entry_selected(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(sftp) = self.active_sftp_mut() {
            sftp.selected_path = Some(path.to_string());
        }
        cx.notify();
    }

    pub(crate) fn sftp_parent_path(path: &str) -> String {
        if path == "/" {
            return "/".to_string();
        }
        path.trim_end_matches('/')
            .rsplit_once('/')
            .map(|(parent, _)| {
                if parent.is_empty() {
                    "/".to_string()
                } else {
                    parent.to_string()
                }
            })
            .unwrap_or_else(|| "/".to_string())
    }

    pub(crate) fn refresh_sftp(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.active_sftp().map(|sftp| sftp.current_path.clone()) {
            self.navigate_sftp(path, cx);
        }
    }

    pub(crate) fn sync_sftp_path_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.pending_sftp_path_sync.take() else {
            return;
        };
        self.sftp_path_input.update(cx, |state, cx| {
            state.set_value(path, window, cx);
        });
    }

    pub(crate) fn open_sftp_context_menu(
        &mut self,
        remote_path: String,
        is_dir: bool,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.sftp_context_menu = Some(SftpContextMenuState {
            remote_path,
            is_dir,
            position,
        });
        cx.notify();
    }

    pub(crate) fn dismiss_sftp_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.sftp_context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn trigger_sftp_context_download(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.sftp_context_menu.take() else {
            return;
        };
        self.download_sftp_entry(menu.remote_path, window, cx);
        cx.notify();
    }

    pub(crate) fn trigger_sftp_context_open_document(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.sftp_context_menu.take() else {
            return;
        };
        if !menu.is_dir {
            self.open_remote_document(menu.remote_path, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn trigger_sftp_context_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.sftp_context_menu.take() else {
            return;
        };
        if let Some(sftp) = self.active_sftp_mut() {
            sftp.selected_entries.clear();
            sftp.selected_entries.insert(menu.remote_path);
        }
        self.show_delete_confirm_dialog(window, cx);
        cx.notify();
    }

    pub(crate) fn download_sftp_entry(
        &mut self,
        remote_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.active_sftp_handle().cloned() else {
            return;
        };
        let path_prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                rust_i18n::t!("document_select_download_folder")
                    .to_string()
                    .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            match path_prompt.await {
                Ok(Ok(Some(mut paths))) => {
                    if let Some(folder) = paths.pop() {
                        let local_path = folder.to_string_lossy().to_string();
                        tracing::info!(
                            "[sftp] initiating download of '{}' to '{}'",
                            remote_path,
                            local_path
                        );
                        handle.download(remote_path, local_path);
                        this.update(cx, |this, cx| {
                            this.show_transfers_dialog = true;
                            cx.notify();
                        })?;
                    }
                }
                Ok(Err(err)) => {
                    this.update(cx, |this, cx| {
                        this.status = format!("download picker failed: {err}").into();
                        cx.notify();
                    })?;
                }
                _ => {}
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn upload_sftp_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.active_sftp_handle().cloned() else {
            return;
        };
        let remote_dir = self
            .active_sftp()
            .map(|sftp| sftp.current_path.clone())
            .unwrap_or_else(|| "/".into());
        let path_prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select File to Upload".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            match path_prompt.await {
                Ok(Ok(Some(mut paths))) => {
                    if let Some(file) = paths.pop() {
                        let local_path = file.to_string_lossy().to_string();
                        tracing::info!(
                            "[sftp] initiating upload of file '{}' to '{}'",
                            local_path,
                            remote_dir
                        );
                        handle.upload_paths(vec![local_path], remote_dir);
                        this.update(cx, |this, cx| {
                            this.show_transfers_dialog = true;
                            cx.notify();
                        })?;
                    }
                }
                Ok(Err(err)) => {
                    this.update(cx, |this, cx| {
                        this.status = format!("upload picker failed: {err}").into();
                        cx.notify();
                    })?;
                }
                _ => {}
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn upload_sftp_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.active_sftp_handle().cloned() else {
            return;
        };
        let remote_dir = self
            .active_sftp()
            .map(|sftp| sftp.current_path.clone())
            .unwrap_or_else(|| "/".into());
        let path_prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Folder to Upload".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            match path_prompt.await {
                Ok(Ok(Some(mut paths))) => {
                    if let Some(folder) = paths.pop() {
                        let local_path = folder.to_string_lossy().to_string();
                        tracing::info!(
                            "[sftp] initiating upload of folder '{}' to '{}'",
                            local_path,
                            remote_dir
                        );
                        handle.upload_paths(vec![local_path], remote_dir);
                        this.update(cx, |this, cx| {
                            this.show_transfers_dialog = true;
                            cx.notify();
                        })?;
                    }
                }
                Ok(Err(err)) => {
                    this.update(cx, |this, cx| {
                        this.status = format!("upload picker failed: {err}").into();
                        cx.notify();
                    })?;
                }
                _ => {}
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn toggle_sftp_entry(
        &mut self,
        path: String,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(sftp) = self.active_sftp_mut() {
            if checked {
                sftp.selected_entries.insert(path);
            } else {
                sftp.selected_entries.remove(&path);
            }
            cx.notify();
        }
    }

    pub(crate) fn toggle_all_sftp_entries(&mut self, checked: bool, cx: &mut Context<Self>) {
        let show_hidden_files = self.show_hidden_files;
        if let Some(sftp) = self.active_sftp_mut() {
            if checked {
                let paths: Vec<String> = sftp
                    .entries
                    .iter()
                    .filter(|entry| show_hidden_files || !entry.name.starts_with('.'))
                    .map(|entry| entry.full_path.clone())
                    .collect();
                for path in paths {
                    sftp.selected_entries.insert(path);
                }
            } else {
                sftp.selected_entries.clear();
            }
            cx.notify();
        }
    }

    pub(crate) fn download_selected_sftp_entries(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sftp) = self.active_sftp() else {
            return;
        };
        let selected: Vec<String> = sftp.selected_entries.iter().cloned().collect();
        if selected.is_empty() {
            return;
        }

        let Some(handle) = self.active_sftp_handle().cloned() else {
            return;
        };

        let path_prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                rust_i18n::t!("document_select_download_folder")
                    .to_string()
                    .into(),
            ),
        });

        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = path_prompt.await
                && let Some(folder) = paths.pop()
            {
                let local_dir = folder.to_string_lossy().to_string();
                tracing::info!(
                    "[sftp] initiating batch download of {} entries to '{}'",
                    selected.len(),
                    local_dir
                );
                for remote in selected {
                    handle.send(crate::sftp::SftpCommand::Download {
                        remote,
                        local_dir: local_dir.clone(),
                    });
                }

                let _ = this.update(cx, |this, cx| {
                    if let Some(sftp_mut) = this.active_sftp_mut() {
                        sftp_mut.selected_entries.clear();
                    }
                    this.show_transfers_dialog = true;
                    cx.notify();
                });
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn upload_sftp_files_batch(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        if let Some(sftp) = self.active_sftp()
            && let Some(handle) = self.active_sftp_handle()
        {
            tracing::info!(
                "[sftp] initiating batch upload of {} files to '{}'",
                paths.len(),
                sftp.current_path
            );
            handle.send(crate::sftp::SftpCommand::UploadPaths {
                locals: paths,
                remote_dir: sftp.current_path.clone(),
            });
            self.show_transfers_dialog = true;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        SftpEntryAction, SftpEntryGesture, apply_sftp_delete_result, begin_sftp_delete,
        finish_sftp_delete, sftp_entry_action,
    };

    #[test]
    fn single_click_only_focuses_entries() {
        assert_eq!(
            sftp_entry_action(false, SftpEntryGesture::SingleClick),
            SftpEntryAction::Focus
        );
        assert_eq!(
            sftp_entry_action(true, SftpEntryGesture::SingleClick),
            SftpEntryAction::Focus
        );
    }

    #[test]
    fn double_click_opens_files_and_navigates_directories() {
        assert_eq!(
            sftp_entry_action(false, SftpEntryGesture::DoubleClick),
            SftpEntryAction::OpenFile
        );
        assert_eq!(
            sftp_entry_action(true, SftpEntryGesture::DoubleClick),
            SftpEntryAction::NavigateDirectory
        );
    }

    #[test]
    fn checkbox_only_toggles_selection() {
        assert_eq!(
            sftp_entry_action(false, SftpEntryGesture::Checkbox),
            SftpEntryAction::ToggleSelection
        );
        assert_eq!(
            sftp_entry_action(true, SftpEntryGesture::Checkbox),
            SftpEntryAction::ToggleSelection
        );
    }

    #[test]
    fn delete_submission_rejects_pending_paths() {
        let mut pending = HashSet::new();
        let paths = vec!["/tmp/a".to_string(), "/tmp/b".to_string()];

        assert!(begin_sftp_delete(&mut pending, &paths));
        assert!(!begin_sftp_delete(&mut pending, &paths));
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn delete_completion_releases_all_pending_paths() {
        let mut pending = HashSet::from([
            "/tmp/a".to_string(),
            "/tmp/b".to_string(),
            "/tmp/c".to_string(),
        ]);

        finish_sftp_delete(&mut pending, &["/tmp/a".to_string(), "/tmp/b".to_string()]);

        assert_eq!(pending, HashSet::from(["/tmp/c".to_string()]));
    }

    #[test]
    fn failed_delete_stays_selected_for_retry() {
        let attempted = vec!["/tmp/a".to_string(), "/tmp/b".to_string()];
        let deleted = vec!["/tmp/a".to_string()];
        let mut pending = HashSet::from_iter(attempted.clone());
        let mut selected = HashSet::from_iter(attempted.clone());

        apply_sftp_delete_result(&mut pending, &mut selected, &attempted, &deleted);

        assert!(pending.is_empty());
        assert_eq!(selected, HashSet::from(["/tmp/b".to_string()]));
    }
}
