//! Application lifecycle plumbing.
//!
//! Owns the UI event pump plus the window-visibility transitions driven by the system
//! tray, the single-instance handshake and termination signals. Kept out of
//! `app/mod.rs` so the "close to tray vs really quit" state machine stays in one
//! readable place instead of being buried in the main view module.

use std::time::Duration;

use gpui::{Context, Window};
use rust_i18n::t;

use crate::Ashell;
use crate::app::tray::TrayRequest;

/// Fast cadence while work is flowing: keeps terminal output, SFTP progress and the
/// cursor blink responsive.
const ACTIVE_INTERVAL: Duration = Duration::from_millis(16);
/// Slow cadence after an idle tick. A window parked in the tray then wakes the CPU 20
/// times per second instead of 60, without affecting latency the user can perceive
/// (tray clicks, activation requests and signals all wait < 50 ms, and the 600 ms cursor
/// blink still lands well within its interval).
const IDLE_INTERVAL: Duration = Duration::from_millis(50);

fn pump_interval(idle: bool) -> Duration {
    if idle { IDLE_INTERVAL } else { ACTIVE_INTERVAL }
}

impl Ashell {
    pub(crate) fn start_event_pump(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let mut last_blink_time = std::time::Instant::now();
            let mut idle = false;
            loop {
                cx.background_executor().timer(pump_interval(idle)).await;
                let outcome = this.update_in(cx, |this, window, cx| {
                    let now = std::time::Instant::now();
                    let changed = this.drain_backend_events();
                    let cwd_follow_changed = this.expire_sftp_cwd_follow_requests(now);
                    let system_sampled = this.sample_system_if_due();
                    this.sync_theme_if_due(cx);
                    this.poll_tray_requests(window, cx);
                    let instance_activated = this
                        .instance_events
                        .as_ref()
                        .is_some_and(|events| events.try_recv().is_ok());
                    if instance_activated {
                        // Another launch asked this instance to show its window.
                        this.show_from_tray(window, cx);
                    }
                    let mut signal_received = false;
                    if !this.closing_application
                        && this
                            .signal_events
                            .as_ref()
                            .is_some_and(|events| events.try_recv().is_ok())
                    {
                        // A termination signal was received: restore the window and
                        // go through the regular quit flow, bypassing close-to-tray.
                        signal_received = true;
                        this.closing_application = true;
                        this.show_from_tray(window, cx);
                        this.request_application_close(window, cx);
                    }
                    let is_blinking = matches!(
                        this.cursor_style,
                        crate::session::config::CursorStyle::Blink
                            | crate::session::config::CursorStyle::BeamBlink
                    );
                    let blink_due = is_blinking
                        && now.duration_since(last_blink_time)
                            >= std::time::Duration::from_millis(600);
                    if changed || system_sampled || cwd_follow_changed || blink_due {
                        cx.notify();
                        if blink_due {
                            last_blink_time = now;
                        }
                    }
                    changed
                        || system_sampled
                        || cwd_follow_changed
                        || blink_due
                        || instance_activated
                        || signal_received
                });
                match outcome {
                    Ok(active) => idle = !active,
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    pub(crate) fn hide_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray.is_none() {
            return;
        }
        self.tray_window_visible = false;
        // Fully hide the window (no taskbar entry, no visible surface) while
        // keeping the process and its sessions alive, then activate the tray
        // so the window cannot be reached from alt-tab either.
        window.set_visible(false);
        self.status = t!("minimized_to_tray").into();
        cx.notify();
    }

    pub(crate) fn show_from_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tray_window_visible = true;
        window.set_visible(true);
        window.activate_window();
        cx.notify();
    }

    pub(crate) fn poll_tray_requests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        let requests: Vec<TrayRequest> = std::iter::from_fn(|| tray.try_recv()).collect();
        for request in requests {
            match request {
                TrayRequest::ToggleWindow => {
                    if self.tray_window_visible {
                        self.hide_to_tray(window, cx);
                    } else {
                        self.show_from_tray(window, cx);
                    }
                }
                TrayRequest::ShowWindow => {
                    self.show_from_tray(window, cx);
                }
                TrayRequest::Quit => {
                    // Restore the window first so the unsaved-document prompt is visible.
                    self.show_from_tray(window, cx);
                    self.request_application_close(window, cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_pump_backs_off_but_stays_fast_enough_for_blinking_and_tray_clicks() {
        assert_eq!(pump_interval(false), Duration::from_millis(16));
        assert_eq!(pump_interval(true), Duration::from_millis(50));
        assert!(
            pump_interval(true) < Duration::from_millis(600),
            "the cursor blink interval is 600 ms, so an idle tick must stay below it"
        );
        assert!(
            pump_interval(true) > pump_interval(false),
            "idle ticks must actually reduce the wake-up rate"
        );
    }
}
