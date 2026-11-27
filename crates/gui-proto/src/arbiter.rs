//! Who may drive the one pointer and keyboard of a display.
//!
//! Whoever holds a button or a key, or typed in the last
//! [`HOLD_AFTER_TYPING`], holds input; events of other clients are dropped.
//! Pointer motion and wheel events from a client while nobody holds input pass
//! but do not take it. Only one client can hold anything down at a time, and
//! [`InputArbiter::remove`] returns what a leaving client held so the host can
//! release it on the app.

use std::time::{Duration, Instant};

use crate::{Buttons, KeyCode, SessionId};

/// How long typing keeps input after the last key or text.
pub const HOLD_AFTER_TYPING: Duration = Duration::from_millis(500);

/// Whether an input event reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Pass it to the app.
    Accept,
    /// Drop it: another client holds input.
    Drop,
}

/// What a client still held when it left, for the host to release on the app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Released {
    /// Buttons to release.
    pub buttons: Buttons,
    /// Keys to release.
    pub keys: Vec<KeyCode>,
}

/// What one client holds down.
#[derive(Debug)]
struct Held {
    session: SessionId,
    buttons: Buttons,
    keys: Vec<KeyCode>,
}

/// Arbitrates the input of one display between its clients.
#[derive(Debug, Default)]
pub struct InputArbiter {
    /// The client that last took input; it holds it while [`Self::holder`] says so.
    owner: Option<SessionId>,
    last_typed: Option<Instant>,
    held: Vec<Held>,
}

impl InputArbiter {
    /// An arbiter where nobody holds input.
    pub fn new() -> Self {
        Self::default()
    }

    /// The client that holds input at `now`, if any.
    pub fn holder(&self, now: Instant) -> Option<SessionId> {
        let owner = self.owner?;
        let holds_down = self
            .held
            .iter()
            .any(|h| h.session == owner && (!h.buttons.is_empty() || !h.keys.is_empty()));
        let typed = self
            .last_typed
            .is_some_and(|at| now < at + HOLD_AFTER_TYPING);
        (holds_down || typed).then_some(owner)
    }

    /// When the holder lets go of input if nothing else happens, so the host
    /// can tell clients; `None` while something is held down or nobody holds it.
    pub fn release_at(&self, now: Instant) -> Option<Instant> {
        let owner = self.holder(now)?;
        let holds_down = self
            .held
            .iter()
            .any(|h| h.session == owner && (!h.buttons.is_empty() || !h.keys.is_empty()));
        if holds_down {
            return None;
        }
        self.last_typed.map(|at| at + HOLD_AFTER_TYPING)
    }

    /// Pointer motion with the buttons `session` holds now. Pressing a button
    /// takes input.
    pub fn pointer(&mut self, session: SessionId, buttons: Buttons, now: Instant) -> Verdict {
        if self.blocked(session, now) {
            return Verdict::Drop;
        }
        let before = self.held(session).map_or(Buttons::NONE, |h| h.buttons);
        if !buttons.pressed_since(before).is_empty() {
            self.owner = Some(session);
        }
        self.entry(session).buttons = buttons;
        Verdict::Accept
    }

    /// A wheel event. It passes unless another client holds input, and does not
    /// take input.
    pub fn wheel(&mut self, session: SessionId, now: Instant) -> Verdict {
        match self.blocked(session, now) {
            true => Verdict::Drop,
            false => Verdict::Accept,
        }
    }

    /// A key going down or up. Either takes input.
    pub fn key(&mut self, session: SessionId, code: KeyCode, down: bool, now: Instant) -> Verdict {
        if self.blocked(session, now) {
            return Verdict::Drop;
        }
        let pressed = self.held(session).is_some_and(|h| h.keys.contains(&code));
        self.take(session, now);
        let keys = &mut self.entry(session).keys;
        match down {
            true if !pressed => keys.push(code),
            true => {}
            false => keys.retain(|key| *key != code),
        }
        Verdict::Accept
    }

    /// Committed text. Takes input.
    pub fn text(&mut self, session: SessionId, now: Instant) -> Verdict {
        if self.blocked(session, now) {
            return Verdict::Drop;
        }
        self.take(session, now);
        Verdict::Accept
    }

    /// Forgets a client that left and returns what it still held down.
    pub fn remove(&mut self, session: SessionId) -> Released {
        if self.owner == Some(session) {
            self.owner = None;
            self.last_typed = None;
        }
        let Some(index) = self.held.iter().position(|h| h.session == session) else {
            return Released::default();
        };
        let held = self.held.swap_remove(index);
        Released {
            buttons: held.buttons,
            keys: held.keys,
        }
    }

    /// Whether another client holds input.
    fn blocked(&self, session: SessionId, now: Instant) -> bool {
        self.holder(now).is_some_and(|holder| holder != session)
    }

    /// `session` typed: it holds input for [`HOLD_AFTER_TYPING`] from now.
    fn take(&mut self, session: SessionId, now: Instant) {
        self.owner = Some(session);
        self.last_typed = Some(now);
    }

    /// What `session` holds, if it ever pressed anything.
    fn held(&self, session: SessionId) -> Option<&Held> {
        self.held.iter().find(|h| h.session == session)
    }

    /// What `session` holds, created empty on first use.
    fn entry(&mut self, session: SessionId) -> &mut Held {
        let index = match self.held.iter().position(|h| h.session == session) {
            Some(index) => index,
            None => {
                self.held.push(Held {
                    session,
                    buttons: Buttons::NONE,
                    keys: Vec::new(),
                });
                self.held.len() - 1
            }
        };
        &mut self.held[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SessionId = SessionId(1);
    const B: SessionId = SessionId(2);
    const KEY_A: KeyCode = KeyCode(0x04);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_held_button_keeps_input_for_as_long_as_it_is_held() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        assert_eq!(arbiter.pointer(A, Buttons::LEFT, t0), Verdict::Accept);
        assert_eq!(arbiter.holder(t0), Some(A));

        let later = t0 + Duration::from_secs(10);
        assert_eq!(arbiter.pointer(B, Buttons::NONE, later), Verdict::Drop);
        assert_eq!(arbiter.pointer(B, Buttons::LEFT, later), Verdict::Drop);
        assert_eq!(arbiter.wheel(B, later), Verdict::Drop);
        assert_eq!(arbiter.key(B, KEY_A, true, later), Verdict::Drop);
        assert_eq!(arbiter.release_at(later), None);

        assert_eq!(arbiter.pointer(A, Buttons::NONE, later), Verdict::Accept);
        assert_eq!(
            arbiter.holder(later),
            None,
            "a click does not hold after release"
        );
        assert_eq!(arbiter.pointer(B, Buttons::LEFT, later), Verdict::Accept);
        assert_eq!(arbiter.holder(later), Some(B));
    }

    #[test]
    fn typing_hands_over_half_a_second_after_the_last_key() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        assert_eq!(arbiter.key(A, KEY_A, true, t0), Verdict::Accept);
        assert_eq!(arbiter.key(A, KEY_A, false, t0 + ms(50)), Verdict::Accept);
        assert_eq!(arbiter.text(A, t0 + ms(60)), Verdict::Accept);
        assert_eq!(arbiter.release_at(t0 + ms(60)), Some(t0 + ms(560)));

        assert_eq!(arbiter.text(B, t0 + ms(559)), Verdict::Drop);
        assert_eq!(
            arbiter.pointer(B, Buttons::NONE, t0 + ms(559)),
            Verdict::Drop
        );
        assert_eq!(arbiter.holder(t0 + ms(560)), None);
        assert_eq!(arbiter.text(B, t0 + ms(560)), Verdict::Accept);
        assert_eq!(arbiter.holder(t0 + ms(560)), Some(B));
    }

    #[test]
    fn a_held_key_keeps_input_past_the_half_second() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        arbiter.key(A, KEY_A, true, t0);
        assert_eq!(arbiter.holder(t0 + Duration::from_secs(3)), Some(A));
        assert_eq!(arbiter.text(B, t0 + Duration::from_secs(3)), Verdict::Drop);
    }

    #[test]
    fn motion_without_a_holder_passes_from_anyone_and_takes_nothing() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        assert_eq!(arbiter.pointer(A, Buttons::NONE, t0), Verdict::Accept);
        assert_eq!(arbiter.wheel(B, t0), Verdict::Accept);
        assert_eq!(arbiter.holder(t0), None);
    }

    #[test]
    fn a_leaving_holder_hands_back_what_it_held_and_frees_input() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        arbiter.key(A, KEY_A, true, t0);
        arbiter.pointer(A, Buttons::RIGHT, t0);
        let released = arbiter.remove(A);
        assert_eq!(released.buttons, Buttons::RIGHT);
        assert_eq!(released.keys, vec![KEY_A]);
        assert_eq!(arbiter.holder(t0), None);
        assert_eq!(arbiter.pointer(B, Buttons::LEFT, t0), Verdict::Accept);
        assert_eq!(arbiter.remove(SessionId(9)), Released::default());
    }
}
