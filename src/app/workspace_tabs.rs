use crate::document::{DocumentConnectionState, LoadState, SaveState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabVisualKind {
    Terminal,
    RemoteDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabStatus {
    Normal,
    Attention,
    Error,
    Unavailable,
}

impl WorkspaceTabStatus {
    fn priority(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Unavailable => 1,
            Self::Attention => 2,
            Self::Error => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabColorRole {
    Success,
    Blue,
    Warning,
    Danger,
    MutedForeground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTabKeyboardAction {
    Current,
    Previous,
    Next,
    First,
    Last,
}

pub(crate) fn terminal_tab_status(
    connected: bool,
    disconnected_reason: Option<&str>,
) -> WorkspaceTabStatus {
    if connected {
        WorkspaceTabStatus::Normal
    } else if disconnected_reason.is_some() {
        WorkspaceTabStatus::Error
    } else {
        WorkspaceTabStatus::Attention
    }
}

pub(crate) fn aggregate_terminal_workspace_status(
    pane_statuses: &[Option<WorkspaceTabStatus>],
) -> WorkspaceTabStatus {
    if pane_statuses.is_empty() {
        return WorkspaceTabStatus::Unavailable;
    }

    pane_statuses
        .iter()
        .fold(WorkspaceTabStatus::Normal, |aggregate, pane_status| {
            let pane_status = pane_status.unwrap_or(WorkspaceTabStatus::Unavailable);
            if pane_status.priority() > aggregate.priority() {
                pane_status
            } else {
                aggregate
            }
        })
}

pub(crate) fn document_workspace_status(
    load_state: &LoadState,
    save_state: &SaveState,
    connection_state: &DocumentConnectionState,
    dirty: bool,
    large_file_loading: bool,
    large_file_error: bool,
) -> WorkspaceTabStatus {
    if matches!(load_state, LoadState::Failed(_))
        || matches!(
            save_state,
            SaveState::Failed(_) | SaveState::OutcomeUnknown(_) | SaveState::Conflict
        )
        || matches!(connection_state, DocumentConnectionState::Offline(_))
        || large_file_error
    {
        WorkspaceTabStatus::Error
    } else if matches!(load_state, LoadState::Loading)
        || matches!(save_state, SaveState::Checking | SaveState::Saving)
        || matches!(connection_state, DocumentConnectionState::Reconnecting)
        || large_file_loading
        || dirty
    {
        WorkspaceTabStatus::Attention
    } else {
        WorkspaceTabStatus::Normal
    }
}

pub(crate) fn workspace_tab_color_role(
    visual_kind: WorkspaceTabVisualKind,
    status: WorkspaceTabStatus,
) -> WorkspaceTabColorRole {
    match status {
        WorkspaceTabStatus::Normal => match visual_kind {
            WorkspaceTabVisualKind::Terminal => WorkspaceTabColorRole::Success,
            WorkspaceTabVisualKind::RemoteDocument => WorkspaceTabColorRole::Blue,
        },
        WorkspaceTabStatus::Attention => WorkspaceTabColorRole::Warning,
        WorkspaceTabStatus::Error => WorkspaceTabColorRole::Danger,
        WorkspaceTabStatus::Unavailable => WorkspaceTabColorRole::MutedForeground,
    }
}

pub(crate) fn workspace_tab_accessibility_label(
    label: &str,
    kind_label: &str,
    status_label: &str,
    dirty: bool,
    unsaved_label: &str,
) -> String {
    let base = format!("{label}, {kind_label}, {status_label}");
    if dirty {
        format!("{base}, {unsaved_label}")
    } else {
        base
    }
}

pub(crate) fn workspace_tab_keyboard_action(
    key: &str,
    has_modifiers: bool,
) -> Option<WorkspaceTabKeyboardAction> {
    if has_modifiers {
        return None;
    }

    match key {
        "enter" | "return" | "space" => Some(WorkspaceTabKeyboardAction::Current),
        "left" => Some(WorkspaceTabKeyboardAction::Previous),
        "right" => Some(WorkspaceTabKeyboardAction::Next),
        "home" => Some(WorkspaceTabKeyboardAction::First),
        "end" => Some(WorkspaceTabKeyboardAction::Last),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_tab_accessibility_label_includes_unsaved_state() {
        assert_eq!(
            workspace_tab_accessibility_label(
                "notes.rs",
                "Remote file",
                "Ready",
                false,
                "Unsaved changes"
            ),
            "notes.rs, Remote file, Ready"
        );
        assert_eq!(
            workspace_tab_accessibility_label(
                "notes.rs",
                "Remote file",
                "Needs attention",
                true,
                "Unsaved changes"
            ),
            "notes.rs, Remote file, Needs attention, Unsaved changes"
        );
    }

    #[test]
    fn workspace_tab_keyboard_actions_cover_standard_tab_keys() {
        use WorkspaceTabKeyboardAction::{Current, First, Last, Next, Previous};

        let cases = [
            ("enter", Some(Current)),
            ("return", Some(Current)),
            ("space", Some(Current)),
            ("left", Some(Previous)),
            ("right", Some(Next)),
            ("home", Some(First)),
            ("end", Some(Last)),
            ("escape", None),
        ];
        for (key, expected) in cases {
            assert_eq!(workspace_tab_keyboard_action(key, false), expected, "{key}");
        }
        assert_eq!(workspace_tab_keyboard_action("right", true), None);
    }

    #[test]
    fn terminal_workspace_status_aggregates_every_pane_by_priority() {
        use WorkspaceTabStatus::{Attention, Error, Normal, Unavailable};

        let terminal_cases = [
            ("connected", true, None, Normal),
            ("initial connection", false, None, Attention),
            ("reported disconnect", false, Some("closed"), Error),
            (
                "connected wins over stale reason",
                true,
                Some("stale"),
                Normal,
            ),
        ];
        for (name, connected, reason, expected) in terminal_cases {
            assert_eq!(terminal_tab_status(connected, reason), expected, "{name}");
        }

        let aggregate_cases = vec![
            (
                "all pane terminals connected",
                vec![Some(Normal), Some(Normal)],
                Normal,
            ),
            (
                "one pane still connecting",
                vec![Some(Normal), Some(Attention)],
                Attention,
            ),
            (
                "one pane reported disconnect",
                vec![Some(Normal), Some(Error)],
                Error,
            ),
            ("empty workspace", vec![], Unavailable),
            (
                "connected pane plus missing instance",
                vec![Some(Normal), None],
                Unavailable,
            ),
            (
                "attention outranks missing instance",
                vec![Some(Normal), None, Some(Attention)],
                Attention,
            ),
            (
                "error outranks attention and missing instance",
                vec![Some(Attention), None, Some(Error)],
                Error,
            ),
        ];
        for (name, pane_statuses, expected) in aggregate_cases {
            assert_eq!(
                aggregate_terminal_workspace_status(&pane_statuses),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn document_workspace_status_covers_all_sources_and_priority() {
        use WorkspaceTabStatus::{Attention, Error, Normal};

        struct Case {
            name: &'static str,
            load_state: LoadState,
            save_state: SaveState,
            connection_state: DocumentConnectionState,
            dirty: bool,
            large_file_loading: bool,
            large_file_error: bool,
            expected: WorkspaceTabStatus,
        }

        let cases = vec![
            Case {
                name: "ready idle clean",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Normal,
            },
            Case {
                name: "ready saved clean",
                load_state: LoadState::Ready,
                save_state: SaveState::Saved,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Normal,
            },
            Case {
                name: "unsaved edits",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "loading document",
                load_state: LoadState::Loading,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "checking save precondition",
                load_state: LoadState::Ready,
                save_state: SaveState::Checking,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "saving document",
                load_state: LoadState::Ready,
                save_state: SaveState::Saving,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "reconnecting document backend",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Reconnecting,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "loading large file page",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: true,
                large_file_error: false,
                expected: Attention,
            },
            Case {
                name: "load failed",
                load_state: LoadState::Failed("load failed".into()),
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "save failed",
                load_state: LoadState::Ready,
                save_state: SaveState::Failed("save failed".into()),
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "save conflict",
                load_state: LoadState::Ready,
                save_state: SaveState::Conflict,
                connection_state: DocumentConnectionState::Online,
                dirty: true,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "connection offline",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Offline("offline".into()),
                dirty: false,
                large_file_loading: false,
                large_file_error: false,
                expected: Error,
            },
            Case {
                name: "large file page failed",
                load_state: LoadState::Ready,
                save_state: SaveState::Idle,
                connection_state: DocumentConnectionState::Online,
                dirty: false,
                large_file_loading: false,
                large_file_error: true,
                expected: Error,
            },
            Case {
                name: "error outranks every processing source",
                load_state: LoadState::Failed("load failed".into()),
                save_state: SaveState::Saving,
                connection_state: DocumentConnectionState::Reconnecting,
                dirty: true,
                large_file_loading: true,
                large_file_error: true,
                expected: Error,
            },
        ];

        for case in cases {
            assert_eq!(
                document_workspace_status(
                    &case.load_state,
                    &case.save_state,
                    &case.connection_state,
                    case.dirty,
                    case.large_file_loading,
                    case.large_file_error,
                ),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn workspace_tab_color_roles_preserve_green_blue_yellow_red_and_gray_semantics() {
        use WorkspaceTabColorRole::{Blue, Danger, MutedForeground, Success, Warning};
        use WorkspaceTabStatus::{Attention, Error, Normal, Unavailable};
        use WorkspaceTabVisualKind::{RemoteDocument, Terminal};

        let cases = [
            (Terminal, Normal, Success),
            (RemoteDocument, Normal, Blue),
            (Terminal, Attention, Warning),
            (RemoteDocument, Attention, Warning),
            (Terminal, Error, Danger),
            (RemoteDocument, Error, Danger),
            (Terminal, Unavailable, MutedForeground),
            (RemoteDocument, Unavailable, MutedForeground),
        ];

        for (visual_kind, status, expected) in cases {
            assert_eq!(workspace_tab_color_role(visual_kind, status), expected);
        }
    }
}
