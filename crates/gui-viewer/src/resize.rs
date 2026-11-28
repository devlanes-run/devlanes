//! The resize of one local window: OSes report no end of an edge drag, so
//! the final size goes to the host once the local size stopped changing for
//! [`SETTLE`], and the window shows Resizing from the first change until the
//! client takes over with its own Resizing, which ends on the first frame
//! after the change.
//!
//! Sizes cannot fight: the first size the window reports after the viewer
//! gave it one is the OS's answer, whatever it is (an OS may round it to its
//! size steps, a tiling desktop may refuse it), and a size the host answered
//! with another size is not sent again.

use std::time::{Duration, Instant};

use devlanes_gui_proto::{RESIZE_TIMEOUT, Size};

/// How long the local size must stay the same before it is sent. Long enough
/// to span the gaps between the size events of one drag, short enough not to
/// feel like a wait after letting go.
pub const SETTLE: Duration = Duration::from_millis(250);

/// The resize state of one local window.
#[derive(Debug, Clone, Default)]
pub struct Resize {
    /// The viewer gave the window a size and its answer has not come yet.
    awaiting_os: bool,
    /// The size the window has, as last reported or given.
    current: Option<Size>,
    /// The size the host has for the window.
    remote: Option<Size>,
    /// A local size not sent yet, and when it last changed.
    pending: Option<(Size, Instant)>,
    /// When the last size was sent, until the client reports a lock state.
    sent_at: Option<Instant>,
    /// The last size sent, until the host answers with a size.
    last_sent: Option<Size>,
    /// A size the host answered with another one, which is not sent again.
    refused: Option<Size>,
}

impl Resize {
    /// A window opened at `size`, the host's size for it.
    pub fn new(size: Size) -> Self {
        Self {
            awaiting_os: true,
            current: Some(size),
            remote: Some(size),
            ..Self::default()
        }
    }

    /// The host's size for the window changed to `size`. Returns the size to
    /// give the local window, or `None` while the user resizes it, which
    /// wins: its size goes to the host when it settles.
    pub fn remote(&mut self, size: Size) -> Option<Size> {
        if let Some(sent) = self.last_sent.take()
            && sent != size
        {
            self.refused = Some(sent);
        }
        self.remote = Some(size);
        if self.pending.is_some() || self.current == Some(size) {
            return None;
        }
        self.awaiting_os = true;
        Some(size)
    }

    /// A new connection's host has `size` for the window. A local size the
    /// user set while the link was down, or one sent that the old connection
    /// lost, goes to the host after the reconnect; otherwise the window takes
    /// the host's size, which this returns.
    pub fn reconnected(&mut self, size: Size, now: Instant) -> Option<Size> {
        self.remote = Some(size);
        self.sent_at = None;
        self.refused = None;
        let unanswered = self.last_sent.take();
        if self.pending.is_some() || !size.is_nonzero() || self.current == Some(size) {
            return None;
        }
        if let Some(current) = self.current.filter(|current| unanswered == Some(*current)) {
            // Due at once: it settled before it was sent.
            self.pending = Some((current, now.checked_sub(SETTLE).unwrap_or(now)));
            return None;
        }
        self.awaiting_os = true;
        Some(size)
    }

    /// The local window reported `size` at `now`.
    pub fn local(&mut self, size: Size, now: Instant) {
        self.current = Some(size);
        // An app that has not drawn yet has no size to change; the local
        // window has a placeholder one until it does.
        if !self.remote.is_some_and(Size::is_nonzero) {
            return;
        }
        if self.awaiting_os && self.pending.is_none() {
            self.awaiting_os = false;
            return;
        }
        if self.pending.is_none() && self.remote == Some(size) {
            return;
        }
        self.pending = Some((size, now));
    }

    /// The size to send to the host at `now`, once, when it settled. A size
    /// the host already has, or one it answered with another size, is not
    /// sent again.
    pub fn due(&mut self, now: Instant) -> Option<Size> {
        let (size, since) = self.pending?;
        if now < since + SETTLE {
            return None;
        }
        self.pending = None;
        if self.remote == Some(size) || self.refused == Some(size) {
            return None;
        }
        self.sent_at = Some(now);
        self.last_sent = Some(size);
        Some(size)
    }

    /// The client reported a lock state for the window: it now shows the
    /// resize itself, or already ended it.
    pub fn client_reported(&mut self) {
        self.sent_at = None;
    }

    /// Whether the window shows Resizing at `now` on the viewer's own
    /// account: while the user drags, and after the size was sent until the
    /// client reports, for at most [`RESIZE_TIMEOUT`].
    pub fn locks(&self, now: Instant) -> bool {
        self.pending.is_some() || self.sent_at.is_some_and(|sent| now < sent + RESIZE_TIMEOUT)
    }

    /// When [`Resize::due`] or [`Resize::locks`] change next.
    pub fn next_change(&self) -> Option<Instant> {
        match (self.pending, self.sent_at) {
            (Some((_, since)), _) => Some(since + SETTLE),
            (None, Some(sent)) => Some(sent + RESIZE_TIMEOUT),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` milliseconds after `t0`.
    fn at(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    /// An edge drag sends one size, the last, once it held for the settle
    /// time; the window is locked from the first change until the client
    /// reports. The size events of sizes the viewer gave the window are not
    /// a resize by the user.
    #[test]
    fn a_drag_sends_its_final_size_once_and_locks_meanwhile() {
        let t0 = Instant::now();
        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        assert!(!resize.locks(t0), "the window's own first size event");

        for (n, width) in [(0, 810), (100, 840), (200, 900)] {
            resize.local(Size::new(width, 600), at(t0, n));
            assert!(resize.locks(at(t0, n)));
            assert_eq!(resize.due(at(t0, n + 50)), None, "still moving");
        }
        assert_eq!(resize.due(at(t0, 200) + SETTLE), Some(Size::new(900, 600)));
        assert_eq!(resize.due(at(t0, 1000)), None, "sent once");
        assert!(resize.locks(at(t0, 1000)), "until the client shows it");
        resize.client_reported();
        assert!(!resize.locks(at(t0, 1000)));

        // Another viewer resized the window: the viewer applies it, and the
        // size event that follows is not sent back.
        assert_eq!(
            resize.remote(Size::new(1024, 768)),
            Some(Size::new(1024, 768))
        );
        resize.local(Size::new(1024, 768), at(t0, 2000));
        assert!(!resize.locks(at(t0, 2000)));
        assert_eq!(resize.due(at(t0, 5000)), None);
    }

    /// The host's size does not fight a drag under way, and a drag that ends
    /// where the host already is sends nothing.
    #[test]
    fn the_user_wins_over_the_host_and_a_round_trip_sends_nothing() {
        let t0 = Instant::now();
        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        resize.local(Size::new(820, 600), t0);
        assert_eq!(resize.remote(Size::new(700, 500)), None, "the user drags");
        assert_eq!(resize.due(t0 + SETTLE), Some(Size::new(820, 600)));

        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        resize.local(Size::new(900, 600), t0);
        resize.local(Size::new(800, 600), at(t0, 100));
        assert_eq!(resize.due(at(t0, 100) + SETTLE), None);
        assert!(!resize.locks(at(t0, 100) + SETTLE));
    }

    /// No loop between the OS, the viewer and the app: the host clamps a
    /// sent size, the OS rounds the size it is given, and the window stays
    /// where they ended without sending again; the size the host refused is
    /// not sent again either.
    #[test]
    fn sizes_the_os_or_the_app_change_are_not_sent_back() {
        let t0 = Instant::now();
        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        resize.local(Size::new(900, 600), t0);
        assert_eq!(resize.due(t0 + SETTLE), Some(Size::new(900, 600)));
        // The app keeps a minimum: the host answers with another size.
        assert_eq!(
            resize.remote(Size::new(1000, 600)),
            Some(Size::new(1000, 600))
        );
        // The OS gives the window 998 wide, its size step.
        resize.local(Size::new(998, 600), at(t0, 300));
        assert_eq!(
            resize.due(at(t0, 2000)),
            None,
            "the OS's answer is not a resize"
        );
        // The user drags to the refused size again: not sent again.
        resize.local(Size::new(900, 600), at(t0, 3000));
        assert_eq!(resize.due(at(t0, 3000) + SETTLE), None);
    }

    /// A drag finished while the link was down is sent after the reconnect;
    /// without one, the window takes the new host's size.
    #[test]
    fn a_reconnect_keeps_the_sizes_together() {
        let t0 = Instant::now();
        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        resize.local(Size::new(900, 600), t0);
        assert_eq!(resize.reconnected(Size::new(800, 600), at(t0, 10)), None);
        assert_eq!(resize.due(at(t0, 10) + SETTLE), Some(Size::new(900, 600)));

        // Sent, but the old connection lost it: sent again at once.
        assert_eq!(resize.reconnected(Size::new(800, 600), at(t0, 500)), None);
        assert_eq!(resize.due(at(t0, 500)), Some(Size::new(900, 600)));

        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        assert_eq!(
            resize.reconnected(Size::new(1024, 768), t0),
            Some(Size::new(1024, 768))
        );
        resize.local(Size::new(1024, 768), t0);
        assert_eq!(resize.due(at(t0, 1000)), None);
    }

    /// A client that never reports a state (its window went away) does not
    /// keep the window locked past the resize timeout.
    #[test]
    fn the_viewer_lock_ends_on_the_resize_timeout() {
        let t0 = Instant::now();
        let mut resize = Resize::new(Size::new(800, 600));
        resize.local(Size::new(800, 600), t0);
        resize.local(Size::new(900, 700), t0);
        let sent = t0 + SETTLE;
        assert!(resize.due(sent).is_some());
        assert_eq!(resize.next_change(), Some(sent + RESIZE_TIMEOUT));
        assert!(!resize.locks(sent + RESIZE_TIMEOUT));
    }
}
