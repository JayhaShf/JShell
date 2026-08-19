use std::time::{Duration, Instant};

use gpui::Context;
use rust_i18n::t;

use crate::{Ashell, session::config::SftpCwdSyncMode, terminal::TabKind};

pub(crate) const SFTP_CWD_REALTIME_LIMIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CwdFollowTrigger {
    TerminalSwitch,
    Realtime,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingCwdFollow {
    pub(crate) path: String,
    pub(crate) trigger: CwdFollowTrigger,
}

#[derive(Clone, Debug)]
pub(crate) struct InFlightCwdFollow {
    pub(crate) request_id: u64,
    pub(crate) path: String,
    pub(crate) trigger: CwdFollowTrigger,
    pub(crate) started_at: Instant,
    pub(crate) accept_result: bool,
    pub(crate) superseded: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CwdFollowCompletion {
    pub(crate) matched: bool,
    pub(crate) accept_result: bool,
    pub(crate) next: Option<(u64, String)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SftpCwdFollowState {
    next_request_id: u64,
    ready: bool,
    realtime_paused: bool,
    in_flight: Option<InFlightCwdFollow>,
    pending: Option<PendingCwdFollow>,
}

#[derive(Debug)]
struct CwdFollowDispatch {
    group_id: String,
    request_id: u64,
    path: String,
}

#[derive(Debug)]
enum CwdFollowAction {
    AlreadyCurrent(String),
    Dispatch(CwdFollowDispatch),
}

impl SftpCwdFollowState {
    pub(crate) fn enqueue(
        &mut self,
        path: String,
        trigger: CwdFollowTrigger,
        configured_mode: SftpCwdSyncMode,
        now: Instant,
    ) -> Option<(u64, String)> {
        if !trigger_is_allowed(configured_mode, self.realtime_paused, trigger) {
            return None;
        }

        if let Some(in_flight) = self
            .in_flight
            .as_mut()
            .filter(|in_flight| in_flight.accept_result && in_flight.path == path)
        {
            in_flight.trigger = stronger_trigger(in_flight.trigger, trigger);
            in_flight.superseded = false;
            self.pending = None;
            return None;
        }
        if let Some(pending) = self.pending.as_mut().filter(|pending| pending.path == path) {
            pending.trigger = stronger_trigger(pending.trigger, trigger);
            return None;
        }

        if let Some(in_flight) = self.in_flight.as_mut() {
            in_flight.superseded = true;
        }
        self.pending = Some(PendingCwdFollow { path, trigger });
        self.start_pending(configured_mode, now)
    }

    pub(crate) fn mark_ready(
        &mut self,
        configured_mode: SftpCwdSyncMode,
        now: Instant,
    ) -> Option<(u64, String)> {
        self.ready = true;
        self.start_pending(configured_mode, now)
    }

    pub(crate) fn mark_unavailable(&mut self) {
        self.ready = false;
        let Some(in_flight) = self.in_flight.take() else {
            return;
        };
        if in_flight.accept_result && !in_flight.superseded && self.pending.is_none() {
            self.pending = Some(PendingCwdFollow {
                path: in_flight.path,
                trigger: in_flight.trigger,
            });
        }
    }

    pub(crate) fn complete(
        &mut self,
        request_id: u64,
        configured_mode: SftpCwdSyncMode,
        now: Instant,
    ) -> CwdFollowCompletion {
        let Some(in_flight) = self.in_flight.as_ref() else {
            return CwdFollowCompletion::default();
        };
        if in_flight.request_id != request_id {
            return CwdFollowCompletion::default();
        }

        let accept_result = self
            .in_flight
            .take()
            .is_some_and(|request| request.accept_result && !request.superseded);
        let next = self.start_pending(configured_mode, now);
        CwdFollowCompletion {
            matched: true,
            accept_result,
            next,
        }
    }

    pub(crate) fn expire_realtime(
        &mut self,
        configured_mode: SftpCwdSyncMode,
        now: Instant,
    ) -> bool {
        if configured_mode != SftpCwdSyncMode::Realtime || self.realtime_paused {
            return false;
        }

        let Some(in_flight) = self.in_flight.as_ref() else {
            return false;
        };
        if !in_flight.accept_result
            || now.saturating_duration_since(in_flight.started_at) < SFTP_CWD_REALTIME_LIMIT
        {
            return false;
        }

        self.realtime_paused = true;
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.trigger == CwdFollowTrigger::Realtime)
        {
            self.pending = None;
        }
        true
    }

    pub(crate) fn cancel_for_manual_navigation(&mut self) {
        self.cancel_automatic_intent();
    }

    fn cancel_automatic_intent(&mut self) {
        self.pending = None;
        if let Some(in_flight) = self.in_flight.as_mut() {
            in_flight.accept_result = false;
        }
    }

    pub(crate) fn resume_realtime(&mut self, now: Instant) {
        self.realtime_paused = false;
        if let Some(in_flight) = self
            .in_flight
            .as_mut()
            .filter(|request| request.accept_result)
        {
            in_flight.started_at = now;
        }
    }

    pub(crate) fn apply_configured_mode(&mut self, mode: SftpCwdSyncMode) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| !trigger_is_allowed(mode, self.realtime_paused, pending.trigger))
        {
            self.pending = None;
        }
        if let Some(in_flight) = self.in_flight.as_mut()
            && !trigger_is_allowed(mode, self.realtime_paused, in_flight.trigger)
        {
            in_flight.accept_result = false;
        }
    }

    pub(crate) fn realtime_paused(&self) -> bool {
        self.realtime_paused
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    fn correction_trigger(&self) -> Option<CwdFollowTrigger> {
        self.pending
            .as_ref()
            .map(|pending| pending.trigger)
            .or_else(|| {
                self.in_flight
                    .as_ref()
                    .filter(|request| request.accept_result && !request.superseded)
                    .map(|request| request.trigger)
            })
    }

    fn start_pending(
        &mut self,
        configured_mode: SftpCwdSyncMode,
        now: Instant,
    ) -> Option<(u64, String)> {
        if !self.ready || self.in_flight.is_some() {
            return None;
        }

        let pending = self.pending.take()?;
        if !trigger_is_allowed(configured_mode, self.realtime_paused, pending.trigger) {
            return None;
        }

        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }
        let request_id = self.next_request_id;
        let path = pending.path;
        self.in_flight = Some(InFlightCwdFollow {
            request_id,
            path: path.clone(),
            trigger: pending.trigger,
            started_at: now,
            accept_result: true,
            superseded: false,
        });
        Some((request_id, path))
    }
}

impl Ashell {
    pub(crate) fn handle_terminal_title_changed(&mut self, tab_id: String, title: String) {
        let home_dir = self
            .tab_groups
            .iter()
            .find(|group| group.pane_root.contains(&tab_id))
            .and_then(|group| group.sftp.as_ref())
            .map(|sftp| sftp.home_dir.as_str())
            .unwrap_or("/");
        let parsed_cwd = parse_terminal_cwd(&title, home_dir);

        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.dynamic_title = title;
        let Some(parsed_cwd) = parsed_cwd else {
            return;
        };
        let cwd_changed = tab.remote_cwd.as_deref() != Some(parsed_cwd.as_str());
        let trigger = if tab.cwd_follow_on_next_title {
            CwdFollowTrigger::TerminalSwitch
        } else {
            CwdFollowTrigger::Realtime
        };
        tab.cwd_follow_on_next_title = false;
        tab.remote_cwd = Some(parsed_cwd);

        if cwd_changed && self.active_tab.as_deref() == Some(tab_id.as_str()) {
            self.follow_active_terminal_cwd(trigger);
        }
    }

    pub(crate) fn handle_sftp_home(&mut self, group_id: String, generation: u64, home: String) {
        let configured_mode = self.config.sftp_cwd_sync_mode();
        let Some((
            terminal_ids,
            focused_terminal_id,
            current_path,
            effective_mode,
            correction_trigger,
        )) = self
            .tab_groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .and_then(|group| {
                let terminal_ids = group
                    .pane_root
                    .terminal_ids()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let focused_terminal_id = group
                    .pane_root
                    .focused_tab_id(&group.focused_pane_path)
                    .or_else(|| group.pane_root.terminal_ids().first().copied())
                    .map(str::to_string);
                let sftp = group.sftp.as_mut()?;
                if !sftp.accepts_generation(generation) {
                    return None;
                }
                sftp.apply_home(home.clone());
                Some((
                    terminal_ids,
                    focused_terminal_id,
                    sftp.current_path.clone(),
                    effective_cwd_sync_mode(configured_mode, sftp.cwd_follow.realtime_paused()),
                    sftp.cwd_follow.correction_trigger(),
                ))
            })
        else {
            return;
        };

        let mut corrected_focused_cwd = None;
        let mut focused_deferred_switch = false;
        for tab in self
            .tabs
            .iter_mut()
            .filter(|tab| terminal_ids.iter().any(|tab_id| tab_id == &tab.id))
        {
            let deferred_switch = tab.cwd_follow_on_next_title;
            let (reparsed_cwd, follow_on_next_title) = reparse_terminal_cwd_after_home(
                &tab.dynamic_title,
                tab.remote_cwd.as_deref(),
                deferred_switch,
                &home,
            );
            tab.cwd_follow_on_next_title = follow_on_next_title;
            if let Some(path) = reparsed_cwd {
                let cwd_changed = tab.remote_cwd.as_deref() != Some(path.as_str());
                let is_focused = focused_terminal_id.as_deref() == Some(tab.id.as_str());
                if is_focused {
                    focused_deferred_switch = deferred_switch;
                }
                if is_focused && (cwd_changed || deferred_switch || correction_trigger.is_some()) {
                    corrected_focused_cwd = Some(path.clone());
                }
                tab.remote_cwd = Some(path);
            }
        }

        let active_group = self.active_group.as_deref() == Some(group_id.as_str());
        if active_group {
            self.pending_sftp_path_sync = Some(current_path);
        }
        let correction_trigger = if focused_deferred_switch {
            Some(CwdFollowTrigger::TerminalSwitch)
        } else {
            correction_trigger
        };
        let trigger = match effective_mode {
            SftpCwdSyncMode::Off => None,
            SftpCwdSyncMode::OnTerminalSwitch | SftpCwdSyncMode::Realtime => correction_trigger,
        };
        if let (Some(tab_id), Some(path), Some(trigger)) =
            (focused_terminal_id, corrected_focused_cwd, trigger)
        {
            let action = self.enqueue_sftp_cwd_follow(group_id, &tab_id, path, trigger);
            self.apply_sftp_cwd_follow_action(action, active_group);
        }
    }

    pub(crate) fn follow_active_terminal_cwd(&mut self, trigger: CwdFollowTrigger) {
        let Some(group_id) = self.active_group.clone() else {
            return;
        };
        let Some(tab_id) = self.active_tab.clone() else {
            return;
        };
        let configured_mode = self.config.sftp_cwd_sync_mode();
        let trigger_allowed = self
            .tab_groups
            .iter()
            .find(|group| group.id == group_id && group.pane_root.contains(&tab_id))
            .and_then(|group| group.sftp.as_ref())
            .is_some_and(|sftp| {
                trigger_is_allowed(configured_mode, sftp.cwd_follow.realtime_paused(), trigger)
            });
        if !trigger_allowed {
            return;
        }
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id && tab.kind == TabKind::Ssh)
        else {
            return;
        };
        let Some(path) = tab.remote_cwd.clone() else {
            if trigger == CwdFollowTrigger::TerminalSwitch {
                tab.cwd_follow_on_next_title = true;
            }
            return;
        };
        tab.cwd_follow_on_next_title = false;

        let action = self.enqueue_sftp_cwd_follow(group_id, &tab_id, path, trigger);
        self.apply_sftp_cwd_follow_action(action, true);
    }

    fn enqueue_sftp_cwd_follow(
        &mut self,
        group_id: String,
        tab_id: &str,
        path: String,
        trigger: CwdFollowTrigger,
    ) -> Option<CwdFollowAction> {
        let configured_mode = self.config.sftp_cwd_sync_mode();
        self.tab_groups
            .iter_mut()
            .find(|group| group.id == group_id && group.pane_root.contains(tab_id))
            .and_then(|group| group.sftp.as_mut())
            .and_then(|sftp| {
                if !trigger_is_allowed(configured_mode, sftp.cwd_follow.realtime_paused(), trigger)
                {
                    return None;
                }
                if sftp.current_path == path && sftp.path_initialized {
                    sftp.cwd_follow.cancel_automatic_intent();
                    return Some(CwdFollowAction::AlreadyCurrent(path));
                }
                sftp.cwd_follow
                    .enqueue(path, trigger, configured_mode, Instant::now())
                    .map(|(request_id, path)| {
                        CwdFollowAction::Dispatch(CwdFollowDispatch {
                            group_id: group_id.clone(),
                            request_id,
                            path,
                        })
                    })
            })
    }

    fn apply_sftp_cwd_follow_action(
        &mut self,
        action: Option<CwdFollowAction>,
        sync_active_path: bool,
    ) {
        match action {
            Some(CwdFollowAction::AlreadyCurrent(path)) if sync_active_path => {
                self.pending_sftp_path_sync = Some(path);
            }
            Some(CwdFollowAction::AlreadyCurrent(_)) => {}
            Some(CwdFollowAction::Dispatch(dispatch)) => {
                self.dispatch_sftp_cwd_follow(dispatch);
            }
            None => {}
        }
    }

    pub(crate) fn dispatch_pending_sftp_cwd_follow(
        &mut self,
        group_id: String,
        request: Option<(u64, String)>,
    ) {
        let Some((request_id, path)) = request else {
            return;
        };
        self.dispatch_sftp_cwd_follow(CwdFollowDispatch {
            group_id,
            request_id,
            path,
        });
    }

    fn dispatch_sftp_cwd_follow(&mut self, dispatch: CwdFollowDispatch) {
        let handle = self.sftp_handles.get(&dispatch.group_id);
        let generation = self
            .tab_groups
            .iter()
            .find(|group| group.id == dispatch.group_id)
            .and_then(|group| group.sftp.as_ref())
            .map(|sftp| sftp.generation);
        let sent = handle.zip(generation).is_some_and(|(handle, generation)| {
            handle.follow_dir(dispatch.path, dispatch.request_id, generation)
        });
        if !sent {
            self.mark_sftp_cwd_follow_unavailable(&dispatch.group_id);
        }
    }

    pub(crate) fn mark_sftp_cwd_follow_unavailable(&mut self, group_id: &str) {
        if let Some(sftp) = self
            .tab_groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .and_then(|group| group.sftp.as_mut())
        {
            sftp.cwd_follow.mark_unavailable();
        }
    }

    pub(crate) fn expire_sftp_cwd_follow_requests(&mut self, now: Instant) -> bool {
        let configured_mode = self.config.sftp_cwd_sync_mode();
        let active_group = self.active_group.as_deref();
        let paused_status = t!("sftp_cwd_sync_realtime_paused").to_string();
        let mut active_paused = false;
        let mut changed = false;

        for group in &mut self.tab_groups {
            let Some(sftp) = group.sftp.as_mut() else {
                continue;
            };
            if sftp.cwd_follow.expire_realtime(configured_mode, now)
                && realtime_pause_is_active(configured_mode, sftp.cwd_follow.realtime_paused())
            {
                sftp.set_cwd_follow_paused_status(paused_status.clone());
                active_paused |= active_group == Some(group.id.as_str());
                changed = true;
            }
        }

        if active_paused {
            self.status = paused_status.into();
        }
        changed
    }

    pub(crate) fn set_sftp_cwd_sync_mode(&mut self, mode: SftpCwdSyncMode, cx: &mut Context<Self>) {
        self.config.set_sftp_cwd_sync_mode(mode);
        if mode == SftpCwdSyncMode::Off {
            for tab in &mut self.tabs {
                tab.cwd_follow_on_next_title = false;
            }
        }
        let active_group = self.active_group.clone();
        let paused_status = t!("sftp_cwd_sync_realtime_paused").to_string();
        let connected_status = t!("sftp_connected").to_string();
        let mut active_pause_transition = None;
        let now = Instant::now();

        for group in &mut self.tab_groups {
            let Some(sftp) = group.sftp.as_mut() else {
                continue;
            };
            let is_active = active_group.as_deref() == Some(group.id.as_str());
            let was_pause_displayed = sftp.is_cwd_follow_pause_status_displayed();
            sftp.cwd_follow.apply_configured_mode(mode);
            if mode == SftpCwdSyncMode::Realtime && is_active {
                sftp.cwd_follow.resume_realtime(now);
            }
            let pause_active = realtime_pause_is_active(mode, sftp.cwd_follow.realtime_paused());
            if pause_active {
                sftp.set_cwd_follow_paused_status(paused_status.clone());
            } else if was_pause_displayed {
                sftp.set_status(connected_status.clone());
            }
            if is_active {
                active_pause_transition = Some((pause_active, was_pause_displayed));
            }
        }

        if let Some((pause_active, was_pause_displayed)) = active_pause_transition {
            if pause_active {
                self.status = paused_status.into();
            } else if was_pause_displayed {
                self.status = connected_status.into();
            }
        }
        self.save_preferences_background();
        if mode == SftpCwdSyncMode::Realtime {
            self.follow_active_terminal_cwd(CwdFollowTrigger::Realtime);
        }
        cx.notify();
    }
}

pub(crate) fn parse_terminal_cwd(title: &str, home_dir: &str) -> Option<String> {
    let trimmed = title.trim();
    let path = if let Some(path) = trimmed.strip_prefix("ASHELL_CWD:") {
        path.trim()
    } else if is_supported_cwd(trimmed) {
        trimmed
    } else {
        conventional_title_path(trimmed)?
    };

    if path.starts_with('/') {
        Some(path.to_string())
    } else if path == "~" {
        Some(home_dir.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        let home = home_dir.trim_end_matches('/');
        Some(format!("{home}/{rest}"))
    } else {
        None
    }
}

fn reparse_terminal_cwd_after_home(
    title: &str,
    remote_cwd: Option<&str>,
    cwd_follow_on_next_title: bool,
    home_dir: &str,
) -> (Option<String>, bool) {
    if remote_cwd.is_none() {
        return (None, cwd_follow_on_next_title);
    }
    match parse_terminal_cwd(title, home_dir) {
        Some(path) => (Some(path), false),
        None => (None, cwd_follow_on_next_title),
    }
}

fn conventional_title_path(value: &str) -> Option<&str> {
    if has_uri_scheme(value) {
        return None;
    }

    value
        .match_indices(':')
        .map(|(separator, _)| value[separator + 1..].trim())
        .find(|path| is_supported_cwd(path) && !path.starts_with("//"))
}

fn has_uri_scheme(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
        && rest.starts_with("//")
}

pub(crate) fn effective_cwd_sync_mode(
    configured: SftpCwdSyncMode,
    realtime_paused: bool,
) -> SftpCwdSyncMode {
    if configured == SftpCwdSyncMode::Realtime && realtime_paused {
        SftpCwdSyncMode::OnTerminalSwitch
    } else {
        configured
    }
}

pub(crate) fn realtime_pause_is_active(
    configured_mode: SftpCwdSyncMode,
    realtime_paused: bool,
) -> bool {
    configured_mode == SftpCwdSyncMode::Realtime && realtime_paused
}

fn trigger_is_allowed(
    configured_mode: SftpCwdSyncMode,
    realtime_paused: bool,
    trigger: CwdFollowTrigger,
) -> bool {
    match effective_cwd_sync_mode(configured_mode, realtime_paused) {
        SftpCwdSyncMode::Off => false,
        SftpCwdSyncMode::OnTerminalSwitch => trigger == CwdFollowTrigger::TerminalSwitch,
        SftpCwdSyncMode::Realtime => true,
    }
}

fn stronger_trigger(current: CwdFollowTrigger, incoming: CwdFollowTrigger) -> CwdFollowTrigger {
    if current == CwdFollowTrigger::TerminalSwitch || incoming == CwdFollowTrigger::TerminalSwitch {
        CwdFollowTrigger::TerminalSwitch
    } else {
        CwdFollowTrigger::Realtime
    }
}

fn is_supported_cwd(value: &str) -> bool {
    value.starts_with('/') || value == "~" || value.starts_with("~/")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::session::config::SftpCwdSyncMode;

    use super::{
        CwdFollowTrigger, SFTP_CWD_REALTIME_LIMIT, SftpCwdFollowState, parse_terminal_cwd,
        realtime_pause_is_active, reparse_terminal_cwd_after_home,
    };

    #[test]
    fn realtime_pause_display_requires_realtime_mode_and_latch() {
        assert!(realtime_pause_is_active(SftpCwdSyncMode::Realtime, true));
        assert!(!realtime_pause_is_active(SftpCwdSyncMode::Off, true));
        assert!(!realtime_pause_is_active(
            SftpCwdSyncMode::OnTerminalSwitch,
            true,
        ));
        assert!(!realtime_pause_is_active(SftpCwdSyncMode::Realtime, false,));
    }

    #[test]
    fn home_reparse_ignores_titles_until_terminal_reports_a_cwd() {
        for title in ["/srv", "prod:/srv"] {
            assert_eq!(
                reparse_terminal_cwd_after_home(title, None, true, "/home/alice"),
                (None, true),
                "title: {title}",
            );
        }
    }

    #[test]
    fn home_reparse_corrects_reported_tilde_paths() {
        assert_eq!(
            reparse_terminal_cwd_after_home("~", Some("/"), false, "/home/alice"),
            (Some("/home/alice".to_string()), false),
        );
        assert_eq!(
            reparse_terminal_cwd_after_home("~/project", Some("/project"), true, "/home/alice",),
            (Some("/home/alice/project".to_string()), false),
        );
    }

    #[test]
    fn parses_supported_terminal_cwd_titles() {
        let home = "/home/tester";
        let cases = [
            ("ASHELL_CWD: /srv/ashell", "/srv/ashell"),
            (" /srv/plain ", "/srv/plain"),
            ("tester@example.test:/var/www", "/var/www"),
            ("tester@example.test: /srv/app", "/srv/app"),
            ("tester@example.test: ~/project", "/home/tester/project"),
            ("tester@[2001:db8::1]:/srv/ipv6", "/srv/ipv6"),
            ("~", "/home/tester"),
            ("~/project", "/home/tester/project"),
            ("/srv/name:with-colon", "/srv/name:with-colon"),
        ];

        for (title, expected) in cases {
            assert_eq!(
                parse_terminal_cwd(title, home).as_deref(),
                Some(expected),
                "title: {title}",
            );
        }
    }

    #[test]
    fn rejects_relative_and_uri_terminal_cwd_titles() {
        for title in [
            "relative/path",
            "ASHELL_CWD: relative/path",
            "https://example.test/path",
            "file:///tmp/path",
        ] {
            assert_eq!(
                parse_terminal_cwd(title, "/home/tester"),
                None,
                "title: {title}",
            );
        }
    }

    #[test]
    fn single_flight_keeps_only_latest_pending_path() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        assert_eq!(state.mark_ready(SftpCwdSyncMode::Realtime, start), None);

        let first = state
            .enqueue(
                "/first".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("the first path should start immediately");
        assert_eq!(
            state.enqueue(
                "/second".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start + Duration::from_millis(1),
            ),
            None,
        );
        assert_eq!(
            state.enqueue(
                "/latest".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start + Duration::from_millis(2),
            ),
            None,
        );

        let completion = state.complete(
            first.0,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(3),
        );

        assert!(completion.matched);
        assert_eq!(completion.next, Some((first.0 + 1, "/latest".to_string())));
    }

    #[test]
    fn newer_pending_path_invalidates_visible_in_flight_result() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        let first = state
            .enqueue(
                "/a".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("the first path should start immediately");
        state.enqueue(
            "/b".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );

        let completion = state.complete(
            first.0,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(2),
        );

        assert!(completion.matched);
        assert!(!completion.accept_result);
        assert_eq!(completion.next, Some((first.0 + 1, "/b".to_string())));
    }

    #[test]
    fn expired_pending_path_does_not_restore_superseded_result() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        let first = state
            .enqueue(
                "/a".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("the first path should start immediately");
        state.enqueue(
            "/b".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );
        assert!(state.expire_realtime(SftpCwdSyncMode::Realtime, start + SFTP_CWD_REALTIME_LIMIT,));

        let completion = state.complete(
            first.0,
            SftpCwdSyncMode::Realtime,
            start + SFTP_CWD_REALTIME_LIMIT,
        );

        assert!(completion.matched);
        assert!(!completion.accept_result);
        assert_eq!(completion.next, None);
    }

    #[test]
    fn reconnect_does_not_restore_superseded_in_flight_path() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        state.enqueue(
            "/a".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start,
        );
        state.enqueue(
            "/b".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );
        assert!(state.expire_realtime(SftpCwdSyncMode::Realtime, start + SFTP_CWD_REALTIME_LIMIT,));

        state.mark_unavailable();

        assert!(state.pending.is_none());
        assert_eq!(
            state.mark_ready(
                SftpCwdSyncMode::Realtime,
                start + SFTP_CWD_REALTIME_LIMIT + Duration::from_millis(1),
            ),
            None,
        );
        assert!(state.in_flight.is_none());
    }

    #[test]
    fn correction_trigger_ignores_superseded_in_flight_request() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        state.enqueue(
            "/a".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start,
        );
        state.enqueue(
            "/b".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );
        assert!(state.expire_realtime(SftpCwdSyncMode::Realtime, start + SFTP_CWD_REALTIME_LIMIT,));

        assert_eq!(state.correction_trigger(), None);
    }

    #[test]
    fn returning_to_in_flight_path_accepts_result_without_followup() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        let first = state
            .enqueue(
                "/a".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("the first path should start immediately");
        state.enqueue(
            "/b".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );
        state.enqueue(
            "/a".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(2),
        );

        let completion = state.complete(
            first.0,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(3),
        );

        assert!(completion.matched);
        assert!(completion.accept_result);
        assert_eq!(completion.next, None);
    }

    #[test]
    fn same_path_preserves_terminal_switch_trigger() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        state.enqueue(
            "/shared".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start,
        );

        assert_eq!(
            state.enqueue(
                "/shared".to_string(),
                CwdFollowTrigger::TerminalSwitch,
                SftpCwdSyncMode::Realtime,
                start + Duration::from_millis(1),
            ),
            None,
        );
        assert_eq!(
            state.in_flight.as_ref().map(|request| request.trigger),
            Some(CwdFollowTrigger::TerminalSwitch),
        );
    }

    #[test]
    fn configured_modes_and_realtime_pause_gate_triggers() {
        let start = Instant::now();

        let mut off = SftpCwdFollowState::default();
        off.mark_ready(SftpCwdSyncMode::Off, start);
        assert_eq!(
            off.enqueue(
                "/off-switch".to_string(),
                CwdFollowTrigger::TerminalSwitch,
                SftpCwdSyncMode::Off,
                start,
            ),
            None,
        );
        assert_eq!(
            off.enqueue(
                "/off-realtime".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Off,
                start,
            ),
            None,
        );
        assert!(off.in_flight.is_none());
        assert!(off.pending.is_none());

        let mut on_switch = SftpCwdFollowState::default();
        on_switch.mark_ready(SftpCwdSyncMode::OnTerminalSwitch, start);
        assert_eq!(
            on_switch.enqueue(
                "/ignored".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::OnTerminalSwitch,
                start,
            ),
            None,
        );
        assert!(
            on_switch
                .enqueue(
                    "/switched".to_string(),
                    CwdFollowTrigger::TerminalSwitch,
                    SftpCwdSyncMode::OnTerminalSwitch,
                    start,
                )
                .is_some()
        );

        let mut paused = SftpCwdFollowState::default();
        paused.mark_ready(SftpCwdSyncMode::Realtime, start);
        let slow = paused
            .enqueue(
                "/slow".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("realtime request should start before the latch is set");
        assert!(
            paused.expire_realtime(SftpCwdSyncMode::Realtime, start + SFTP_CWD_REALTIME_LIMIT,)
        );
        paused.complete(
            slow.0,
            SftpCwdSyncMode::Realtime,
            start + SFTP_CWD_REALTIME_LIMIT,
        );
        assert_eq!(
            paused.enqueue(
                "/paused-realtime".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start + SFTP_CWD_REALTIME_LIMIT,
            ),
            None,
        );
        assert!(
            paused
                .enqueue(
                    "/paused-switch".to_string(),
                    CwdFollowTrigger::TerminalSwitch,
                    SftpCwdSyncMode::Realtime,
                    start + SFTP_CWD_REALTIME_LIMIT,
                )
                .is_some()
        );
        paused.mark_unavailable();
        assert!(paused.realtime_paused());
        paused.mark_ready(
            SftpCwdSyncMode::Realtime,
            start + SFTP_CWD_REALTIME_LIMIT + Duration::from_millis(1),
        );
        assert!(paused.realtime_paused());
        paused.resume_realtime(start + SFTP_CWD_REALTIME_LIMIT + Duration::from_millis(2));
        assert!(!paused.realtime_paused());
    }

    #[test]
    fn manual_navigation_invalidates_in_flight_result() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        let request = state
            .enqueue(
                "/automatic".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("automatic navigation should start immediately");

        state.cancel_for_manual_navigation();
        let completion = state.complete(
            request.0,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );

        assert!(completion.matched);
        assert!(!completion.accept_result);
        assert_eq!(completion.next, None);
    }

    #[test]
    fn stale_request_ids_do_not_consume_the_active_request() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        let request = state
            .enqueue(
                "/current".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("automatic navigation should start immediately");

        let stale = state.complete(
            request.0 + 1,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );
        assert!(!stale.matched);
        assert!(!stale.accept_result);

        let current = state.complete(
            request.0,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(2),
        );
        assert!(current.matched);
        assert!(current.accept_result);
    }

    #[test]
    fn realtime_pauses_at_three_seconds_and_resume_resets_deadline() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        state.enqueue(
            "/slow".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start,
        );

        assert!(!state.expire_realtime(
            SftpCwdSyncMode::Realtime,
            start + SFTP_CWD_REALTIME_LIMIT - Duration::from_millis(1),
        ));
        assert!(state.expire_realtime(SftpCwdSyncMode::Realtime, start + SFTP_CWD_REALTIME_LIMIT,));
        assert!(state.realtime_paused());

        let resumed_at = start + Duration::from_secs(10);
        state.resume_realtime(resumed_at);

        assert!(!state.realtime_paused());
        assert!(!state.expire_realtime(
            SftpCwdSyncMode::Realtime,
            resumed_at + SFTP_CWD_REALTIME_LIMIT - Duration::from_millis(1),
        ));
        assert!(state.expire_realtime(
            SftpCwdSyncMode::Realtime,
            resumed_at + SFTP_CWD_REALTIME_LIMIT,
        ));
    }

    #[test]
    fn reconnect_resends_latest_pending_path() {
        let start = Instant::now();
        let mut state = SftpCwdFollowState::default();
        state.mark_ready(SftpCwdSyncMode::Realtime, start);
        let first = state
            .enqueue(
                "/first".to_string(),
                CwdFollowTrigger::Realtime,
                SftpCwdSyncMode::Realtime,
                start,
            )
            .expect("the first path should start immediately");
        state.enqueue(
            "/latest".to_string(),
            CwdFollowTrigger::Realtime,
            SftpCwdSyncMode::Realtime,
            start + Duration::from_millis(1),
        );

        state.mark_unavailable();

        assert!(!state.is_ready());
        assert_eq!(
            state.mark_ready(SftpCwdSyncMode::Realtime, start + Duration::from_millis(2),),
            Some((first.0 + 1, "/latest".to_string())),
        );
    }
}
