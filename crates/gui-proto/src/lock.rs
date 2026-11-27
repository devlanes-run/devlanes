//! The lock state of one window, on both ends.
//!
//! The host runs a [`HostLock`] per window and sends its state in the window
//! description and in [`crate::HostMessage::LockChanged`]. A viewer runs a
//! [`ViewerLock`] per window, which combines what the host said with what
//! only the viewer sees: its link to the host and its own resize, which ends
//! by sequence number. Each cause is tracked on its own and the most serious
//! one shows, so causes that overlap clear in any order. Every method takes
//! the time, so neither machine reads a clock.

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

/// How long Resizing lasts at most. It ends earlier on a frame after the
/// change; this covers an app that keeps its size on a host that sends frames
/// only when the picture changes (Windows), where no such frame comes.
pub const RESIZE_TIMEOUT: Duration = Duration::from_secs(3);

/// The lock state of one window as the host sees it: what it sends to viewers.
#[derive(Debug, Clone, Default)]
pub struct HostLock {
    resizing_since: Option<Instant>,
    waiting_since: Option<Instant>,
    frozen: bool,
    stopped: bool,
}

impl HostLock {
    /// An unlocked window.
    pub fn new() -> Self {
        Self::default()
    }

    /// The host applied a size change from a viewer or the app.
    pub fn resize_applied(&mut self, now: Instant) {
        self.resizing_since = Some(now);
    }

    /// The host captured a frame after applying the change. It ends Resizing
    /// whatever its size, because apps clamp to sizes of their own.
    pub fn frame_captured(&mut self) {
        self.resizing_since = None;
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

    /// The service stopped or failed. Final: nothing unlocks the window after
    /// this, and the host process ends with the unit.
    pub fn service_stopped(&mut self) {
        self.stopped = true;
    }

    /// The state at `now`, the most serious cause first: Closed, Frozen,
    /// Waiting, Resizing. Closed after [`WAITING_GRACE`] is not final: a new
    /// window of the service can still appear, and the host announces it as
    /// a new window.
    pub fn state(&self, now: Instant) -> LockState {
        let waited_out = self
            .waiting_since
            .is_some_and(|since| now >= since + WAITING_GRACE);
        let resizing = self
            .resizing_since
            .is_some_and(|since| now < since + RESIZE_TIMEOUT);
        if self.stopped || waited_out {
            LockState::Closed
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

    /// When [`HostLock::state`] changes next if nothing else happens.
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        [
            self.waiting_since.map(|since| since + WAITING_GRACE),
            self.resizing_since.map(|since| since + RESIZE_TIMEOUT),
        ]
        .into_iter()
        .flatten()
        .filter(|at| *at > now)
        .min()
    }
}

/// A resize the viewer asked for.
#[derive(Debug, Clone, Copy)]
struct LocalResize {
    since: Instant,
    /// From [`crate::HostMessage::ResizeApplied`]; frames before it show the old size.
    from_seq: Option<u64>,
}

/// The lock state of one window as a viewer shows it.
#[derive(Debug, Clone)]
pub struct ViewerLock {
    host: LockState,
    resize: Option<LocalResize>,
    /// When the link last carried data, or `None` while it is closed.
    link_alive: Option<Instant>,
}

impl ViewerLock {
    /// An unlocked window on a link that carried data at `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            host: LockState::Unlocked,
            resize: None,
            link_alive: Some(now),
        }
    }

    /// The host's state for the window, from its description or
    /// [`crate::HostMessage::LockChanged`].
    pub fn host_state(&mut self, state: LockState) {
        self.host = state;
    }

    /// The viewer sent a resize request for the window.
    pub fn resize_requested(&mut self, now: Instant) {
        self.resize = Some(LocalResize {
            since: now,
            from_seq: None,
        });
    }

    /// The host applied it; frames from `from_seq` on were captured after.
    pub fn resize_applied(&mut self, from_seq: u64) {
        if let Some(resize) = &mut self.resize {
            resize.from_seq = Some(from_seq);
        }
    }

    /// A frame of the window arrived. A frame at or after the resize's
    /// sequence number ends Resizing whatever its size; earlier ones were
    /// captured before the change.
    pub fn frame(&mut self, seq: u64, now: Instant) {
        let done = self
            .resize
            .is_some_and(|resize| resize.from_seq.is_some_and(|from| seq >= from));
        if done {
            self.resize = None;
        }
        self.data(now);
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

    /// A new connection to the host is up. The host's state comes again with
    /// its welcome, so a resize the old connection had under way is dropped.
    pub fn link_opened(&mut self, now: Instant) {
        self.link_alive = Some(now);
        self.resize = None;
    }

    /// The state at `now`. Closed from the host wins, since a host that
    /// stops closes the link too; then Reconnecting, then the host's other
    /// states, then the viewer's own resize.
    pub fn state(&self, now: Instant) -> LockState {
        let silent = self
            .link_alive
            .is_none_or(|alive| now >= alive + RECONNECT_AFTER);
        let resizing = self
            .resize
            .is_some_and(|resize| now < resize.since + RESIZE_TIMEOUT);
        if self.host == LockState::Closed {
            LockState::Closed
        } else if silent {
            LockState::Reconnecting
        } else if self.host.is_locked() {
            self.host
        } else if resizing {
            LockState::Resizing
        } else {
            LockState::Unlocked
        }
    }

    /// When [`ViewerLock::state`] changes next if nothing else happens.
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        [
            self.link_alive.map(|alive| alive + RECONNECT_AFTER),
            self.resize.map(|resize| resize.since + RESIZE_TIMEOUT),
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

    /// `n` milliseconds.
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_host_ends_resizing_on_the_first_capture_or_the_timeout() {
        let t0 = Instant::now();
        let mut lock = HostLock::new();
        lock.resize_applied(t0);
        assert_eq!(lock.state(t0), LockState::Resizing);
        lock.frame_captured();
        assert_eq!(lock.state(t0), LockState::Unlocked);

        lock.resize_applied(t0);
        assert_eq!(lock.next_change(t0), Some(t0 + RESIZE_TIMEOUT));
        assert_eq!(lock.state(t0 + RESIZE_TIMEOUT), LockState::Unlocked);
    }

    #[test]
    fn a_closed_window_waits_for_the_app_then_counts_as_closed() {
        let t0 = Instant::now();
        let mut lock = HostLock::new();
        lock.window_gone(t0);
        assert_eq!(lock.state(t0 + ms(4000)), LockState::Waiting);
        lock.window_back();
        assert_eq!(lock.state(t0 + ms(4000)), LockState::Unlocked);

        lock.window_gone(t0 + ms(4000));
        assert_eq!(lock.state(t0 + ms(4000) + WAITING_GRACE), LockState::Closed);
        lock.window_back();
        assert_eq!(lock.state(t0 + ms(9000)), LockState::Unlocked, "not final");
        lock.service_stopped();
        lock.window_back();
        assert_eq!(lock.state(t0 + ms(9000)), LockState::Closed, "final");
    }

    #[test]
    fn frames_sent_before_the_resize_do_not_end_it_on_the_viewer() {
        let t0 = Instant::now();
        let mut lock = ViewerLock::new(t0);
        lock.resize_requested(t0);
        lock.frame(10, t0 + ms(10));
        lock.resize_applied(12);
        lock.frame(11, t0 + ms(20));
        assert_eq!(lock.state(t0 + ms(20)), LockState::Resizing);
        lock.frame(12, t0 + ms(30));
        assert_eq!(lock.state(t0 + ms(30)), LockState::Unlocked);
    }

    #[test]
    fn a_resize_without_a_new_frame_ends_on_the_timeout() {
        let t0 = Instant::now();
        let mut lock = ViewerLock::new(t0);
        lock.resize_requested(t0);
        lock.resize_applied(5);
        lock.data(t0 + ms(2000));
        assert_eq!(lock.state(t0 + ms(2000)), LockState::Resizing);
        assert_eq!(lock.next_change(t0 + ms(2000)), Some(t0 + RESIZE_TIMEOUT));
        assert_eq!(lock.state(t0 + RESIZE_TIMEOUT), LockState::Unlocked);
    }

    #[test]
    fn a_pause_of_a_few_seconds_is_not_reconnecting() {
        let t0 = Instant::now();
        let mut lock = ViewerLock::new(t0);
        assert_eq!(lock.state(t0 + ms(5000)), LockState::Unlocked);
        assert_eq!(lock.state(t0 + RECONNECT_AFTER), LockState::Reconnecting);
        lock.data(t0 + RECONNECT_AFTER);
        assert_eq!(lock.state(t0 + RECONNECT_AFTER), LockState::Unlocked);
    }

    #[test]
    fn the_viewer_combines_the_host_state_with_its_link() {
        let t0 = Instant::now();
        let mut lock = ViewerLock::new(t0);
        lock.host_state(LockState::Frozen);
        lock.resize_requested(t0);
        assert_eq!(lock.state(t0), LockState::Frozen);
        lock.link_closed();
        lock.data(t0);
        assert_eq!(
            lock.state(t0),
            LockState::Reconnecting,
            "data on a closed link counts for nothing"
        );
        lock.host_state(LockState::Unlocked);
        lock.link_opened(t0);
        assert_eq!(
            lock.state(t0),
            LockState::Unlocked,
            "the old connection's resize is gone"
        );
        lock.host_state(LockState::Locked);
        assert_eq!(lock.state(t0), LockState::Locked);
        lock.host_state(LockState::Closed);
        lock.link_closed();
        assert_eq!(lock.state(t0), LockState::Closed);
    }
}
