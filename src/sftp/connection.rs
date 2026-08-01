use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SftpGeneration(pub u64);

impl SftpGeneration {
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

const RECONNECT_DELAY_SECONDS: [u64; 7] = [1, 2, 4, 8, 16, 30, 30];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ReconnectBackoff {
    next_index: usize,
    stopped: bool,
}

impl ReconnectBackoff {
    pub const fn new() -> Self {
        Self {
            next_index: 0,
            stopped: false,
        }
    }

    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.stopped {
            return None;
        }

        let delay = Duration::from_secs(RECONNECT_DELAY_SECONDS[self.next_index]);
        self.next_index = (self.next_index + 1).min(RECONNECT_DELAY_SECONDS.len() - 1);
        Some(delay)
    }

    pub fn reset(&mut self) {
        if !self.stopped {
            self.next_index = 0;
        }
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    #[cfg(test)]
    const fn is_stopped(&self) -> bool {
        self.stopped
    }
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Blocked,
    Closed,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DisconnectOutcome {
    pub generation: SftpGeneration,
    pub retry_after: Option<Duration>,
}

#[derive(Debug)]
pub struct ConnectionSupervisor {
    state: ConnectionState,
    generation: SftpGeneration,
    backoff: ReconnectBackoff,
}

impl ConnectionSupervisor {
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Connecting,
            generation: SftpGeneration::default(),
            backoff: ReconnectBackoff::new(),
        }
    }

    #[cfg(test)]
    const fn state(&self) -> ConnectionState {
        self.state
    }

    pub const fn generation(&self) -> SftpGeneration {
        self.generation
    }

    #[cfg(test)]
    const fn backoff(&self) -> &ReconnectBackoff {
        &self.backoff
    }

    pub fn begin_connecting(&mut self) -> Option<SftpGeneration> {
        if matches!(
            self.state,
            ConnectionState::Blocked | ConnectionState::Closed
        ) {
            return None;
        }

        self.state = ConnectionState::Connecting;
        Some(self.generation)
    }

    pub fn mark_connected(&mut self, generation: SftpGeneration) -> bool {
        if matches!(
            self.state,
            ConnectionState::Blocked | ConnectionState::Closed
        ) || self.generation != generation
        {
            return false;
        }

        self.state = ConnectionState::Connected;
        self.backoff.reset();
        true
    }

    pub fn disconnect(&mut self, generation: SftpGeneration) -> Option<DisconnectOutcome> {
        if matches!(
            self.state,
            ConnectionState::Blocked | ConnectionState::Closed | ConnectionState::Reconnecting
        ) || self.generation != generation
        {
            return None;
        }

        self.state = ConnectionState::Reconnecting;
        self.generation = self.generation.next();
        let next_generation = self.generation;

        Some(DisconnectOutcome {
            generation: next_generation,
            retry_after: self.backoff.next_delay(),
        })
    }

    pub fn block(&mut self, generation: SftpGeneration) -> bool {
        if matches!(
            self.state,
            ConnectionState::Blocked | ConnectionState::Closed
        ) || self.generation != generation
        {
            return false;
        }

        self.state = ConnectionState::Blocked;
        self.generation = self.generation.next();
        true
    }

    pub fn manual_reconnect(&mut self) -> Option<SftpGeneration> {
        if self.state == ConnectionState::Closed {
            return None;
        }

        self.state = ConnectionState::Connecting;
        self.backoff = ReconnectBackoff::new();
        Some(self.generation)
    }

    pub fn is_blocked(&self) -> bool {
        self.state == ConnectionState::Blocked
    }

    pub fn close(&mut self) {
        if self.state == ConnectionState::Closed {
            return;
        }

        self.state = ConnectionState::Closed;
        self.backoff.stop();
    }
}

impl Default for ConnectionSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn reconnect_backoff_is_capped_resettable_and_stoppable() {
        let mut backoff = ReconnectBackoff::new();
        let delays: Vec<_> = (0..8).map(|_| backoff.next_delay()).collect();

        assert_eq!(
            delays,
            vec![
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(4)),
                Some(Duration::from_secs(8)),
                Some(Duration::from_secs(16)),
                Some(Duration::from_secs(30)),
                Some(Duration::from_secs(30)),
                Some(Duration::from_secs(30)),
            ]
        );

        backoff.reset();
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));

        backoff.stop();
        assert_eq!(backoff.next_delay(), None);
        backoff.reset();
        assert_eq!(backoff.next_delay(), None);
    }

    #[test]
    fn disconnect_advances_generation_and_uses_backoff() {
        let mut supervisor = ConnectionSupervisor::new();
        let old_generation = supervisor.generation();
        assert!(supervisor.mark_connected(old_generation));

        let outcome = supervisor
            .disconnect(old_generation)
            .expect("current connection");
        assert_eq!(outcome.generation, SftpGeneration(1));
        assert_eq!(outcome.retry_after, Some(Duration::from_secs(1)));
        assert_eq!(supervisor.state(), ConnectionState::Reconnecting);
        assert!(!supervisor.mark_connected(old_generation));
        assert_eq!(supervisor.begin_connecting(), Some(outcome.generation));
        assert_eq!(supervisor.state(), ConnectionState::Connecting);
        assert!(supervisor.mark_connected(outcome.generation));
        assert_eq!(supervisor.state(), ConnectionState::Connected);
    }

    #[test]
    fn disconnect_is_idempotent_while_reconnecting() {
        let mut supervisor = ConnectionSupervisor::new();
        let connected_generation = supervisor.generation();
        assert!(supervisor.mark_connected(connected_generation));

        let first = supervisor
            .disconnect(connected_generation)
            .expect("first disconnect starts reconnecting");

        assert_eq!(supervisor.state(), ConnectionState::Reconnecting);
        assert_eq!(supervisor.generation(), first.generation);
        assert!(supervisor.disconnect(first.generation).is_none());
        assert_eq!(supervisor.generation(), first.generation);
    }

    #[test]
    fn closing_stops_reconnects() {
        let mut supervisor = ConnectionSupervisor::new();
        let generation = supervisor.generation();
        assert!(supervisor.mark_connected(generation));
        assert!(supervisor.disconnect(generation).is_some());

        supervisor.close();
        assert_eq!(supervisor.state(), ConnectionState::Closed);
        assert!(supervisor.backoff().is_stopped());
        assert_eq!(supervisor.begin_connecting(), None);
        assert!(!supervisor.mark_connected(supervisor.generation()));
    }

    #[test]
    fn permanent_failure_waits_for_manual_reconnect() {
        let mut supervisor = ConnectionSupervisor::new();
        let generation = supervisor.generation();

        assert!(supervisor.block(generation));
        assert_eq!(supervisor.state(), ConnectionState::Blocked);
        assert_eq!(supervisor.generation(), generation.next());
        assert_eq!(supervisor.begin_connecting(), None);
        assert!(supervisor.disconnect(supervisor.generation()).is_none());
    }

    #[test]
    fn manual_reconnect_resumes_blocked_connection() {
        let mut supervisor = ConnectionSupervisor::new();
        let generation = supervisor.generation();
        assert!(supervisor.block(generation));

        let resumed = supervisor
            .manual_reconnect()
            .expect("blocked connection can be manually retried");

        assert_eq!(supervisor.state(), ConnectionState::Connecting);
        assert_eq!(resumed, generation.next());
        assert_eq!(supervisor.begin_connecting(), Some(resumed));
    }
}
