//! The lock state of one window.
//!
//! Each cause of a lock is tracked on its own and the state shown is the most
//! serious one, so causes that overlap (a resize while the link drops) clear
//! in any order. The host tracks resizing, waiting and freezing and sends the
//! result; a viewer feeds the same machine what the host said and adds what
//! only it sees, the link.

use std::time::{Duration, Instant};

use crate::LockState;

/// How long a closed window waits for the app to open a new one, such as
/// during an Electron main-process restart, before it counts as closed.
pub const WAITING_GRACE: Duration = Duration::from_secs(5);

/// How long the link may stay silent before the window shows Reconnecting.
/// Hosts send a heartbeat every [`crate::HEARTBEAT_INTERVAL`], and a P2P link
/// can pause for about four seconds after a network switch without anything
/// being wrong, so this sits above both.
pub const RECONNECT_AFTER: Duration = Duration::from_secs(8);

/// How long Resizing lasts when no frame comes after the request, such as an
/// app that keeps its size and a host that sends frames only on change.
pub const RESIZE_TIMEOUT: Duration = Duration::from_secs(3);

/// The lock state machine of one window. Every method takes the time, so the
/// machine never reads a clock.
#[derive(Debug, Clone)]
pub struct WindowLock {
    resizing_since: Option<Instant>,
    waiting_since: Option<Instant>,
    frozen: bool,
    /// When the link last carried data, or `None` while it is closed.
    link_alive: Option<Instant>,
    closed: bool,
}

impl WindowLock {
    /// An unlocked window on a link that carried data at `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            resizing_since: None,
            waiting_since: None,
            frozen: false,
            link_alive: Some(now),
            closed: false,
        }
    }

    /// A viewer or the app asked for a new size.
    pub fn resize_requested(&mut self, now: Instant) {
        self.resizing_since = Some(now);
    }

    /// A frame of the window arrived or was captured. It ends Resizing whatever
    /// its size, because apps clamp to sizes of their own.
    pub fn frame(&mut self, now: Instant) {
        self.resizing_since = None;
        self.data(now);
    }

    /// The window closed while the service still runs.
    pub fn window_gone(&mut self, now: Instant) {
        self.waiting_since.get_or_insert(now);
    }

    /// A window of the service appeared again in its place.
    pub fn window_back(&mut self) {
        self.waiting_since = None;
    }

    /// The lane was frozen or thawed.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Anything arrived on the link, a heartbeat included.
    pub fn data(&mut self, now: Instant) {
        if self.link_alive.is_some() {
            self.link_alive = Some(now);
        }
    }

    /// The connection to the host closed.
    pub fn link_closed(&mut self) {
        self.link_alive = None;
    }

    /// A new connection to the host is up.
    pub fn link_opened(&mut self, now: Instant) {
        self.link_alive = Some(now);
    }

    /// The service stopped or failed. Nothing unlocks the window after this.
    pub fn service_stopped(&mut self) {
        self.closed = true;
    }

    /// The state at `now`, the most serious cause first: Closed, Reconnecting,
    /// Frozen, Waiting, Resizing.
    pub fn state(&self, now: Instant) -> LockState {
        let waited_out = self
            .waiting_since
            .is_some_and(|since| now >= since + WAITING_GRACE);
        let silent = self
            .link_alive
            .is_none_or(|alive| now >= alive + RECONNECT_AFTER);
        let resizing = self
            .resizing_since
            .is_some_and(|since| now < since + RESIZE_TIMEOUT);
        if self.closed || waited_out {
            LockState::Closed
        } else if silent {
            LockState::Reconnecting
        } else if self.frozen {
            LockState::Frozen
        } else if self.waiting_since.is_some() {
            LockState::Waiting
        } else if resizing {
            LockState::Resizing
        } else {
            LockState::Unlocked
        }
    }

    /// When [`WindowLock::state`] changes next if nothing else happens.
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        [
            self.waiting_since.map(|since| since + WAITING_GRACE),
            self.link_alive.map(|alive| alive + RECONNECT_AFTER),
            self.resizing_since.map(|since| since + RESIZE_TIMEOUT),
        ]
        .into_iter()
        .flatten()
        .filter(|at| *at > now)
        .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` seconds.
    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn resizing_ends_on_the_first_frame_after_the_request() {
        let t0 = Instant::now();
        let mut lock = WindowLock::new(t0);
        lock.resize_requested(t0);
        assert_eq!(lock.state(t0), LockState::Resizing);
        lock.frame(t0 + Duration::from_millis(40));
        assert_eq!(
            lock.state(t0 + Duration::from_millis(40)),
            LockState::Unlocked
        );
    }

    #[test]
    fn resizing_ends_by_itself_when_no_frame_comes() {
        let t0 = Instant::now();
        let mut lock = WindowLock::new(t0);
        lock.resize_requested(t0);
        lock.data(t0 + s(2));
        assert_eq!(lock.next_change(t0 + s(2)), Some(t0 + RESIZE_TIMEOUT));
        assert_eq!(lock.state(t0 + RESIZE_TIMEOUT), LockState::Unlocked);
    }

    #[test]
    fn a_closed_window_waits_for_the_app_then_counts_as_closed() {
        let t0 = Instant::now();
        let mut lock = WindowLock::new(t0);
        lock.window_gone(t0);
        lock.data(t0 + s(4));
        assert_eq!(lock.state(t0 + s(4)), LockState::Waiting);
        lock.window_back();
        assert_eq!(lock.state(t0 + s(4)), LockState::Unlocked);

        lock.window_gone(t0 + s(4));
        lock.data(t0 + s(8));
        assert_eq!(lock.state(t0 + s(4) + WAITING_GRACE), LockState::Closed);
    }

    #[test]
    fn a_pause_of_a_few_seconds_is_not_reconnecting() {
        let t0 = Instant::now();
        let mut lock = WindowLock::new(t0);
        assert_eq!(lock.state(t0 + s(5)), LockState::Unlocked);
        assert_eq!(lock.state(t0 + RECONNECT_AFTER), LockState::Reconnecting);
        lock.data(t0 + RECONNECT_AFTER);
        assert_eq!(lock.state(t0 + RECONNECT_AFTER), LockState::Unlocked);
    }

    #[test]
    fn overlapping_causes_show_the_most_serious_and_clear_in_any_order() {
        let t0 = Instant::now();
        let mut lock = WindowLock::new(t0);
        lock.set_frozen(true);
        lock.resize_requested(t0);
        assert_eq!(lock.state(t0), LockState::Frozen);
        lock.link_closed();
        lock.data(t0);
        assert_eq!(
            lock.state(t0),
            LockState::Reconnecting,
            "data on a closed link counts for nothing"
        );
        lock.set_frozen(false);
        lock.link_opened(t0);
        assert_eq!(lock.state(t0), LockState::Resizing);
        lock.service_stopped();
        lock.frame(t0);
        assert_eq!(lock.state(t0), LockState::Closed);
    }
}
