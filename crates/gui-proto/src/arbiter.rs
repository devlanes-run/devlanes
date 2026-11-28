//! Who may drive the one pointer and keyboard of a display.
//!
//! Whoever holds a button or a key, or typed in the last
//! [`HOLD_AFTER_TYPING`], holds input; events of other clients are dropped.
//! Pointer motion and wheel events from a client while nobody holds input pass
//! but do not take it. Only one client can hold anything down at a time.
//!
//! A held button or key normally ends with its release. A client whose link
//! died mid-drag never sends one, so a client that holds something and sends
//! nothing for [`HELD_INPUT_TIMEOUT`] loses it: [`InputArbiter::expire`] and
//! [`InputArbiter::remove`] return what it held, for the host to release on
//! the app. If the client comes back still reporting those buttons or keys
//! down, its events for them are dropped until it reports them released, so
//! the app never sees a second press without a release in between.

use std::time::{Duration, Instant};

use crate::{Buttons, KeyCode, RECONNECT_AFTER, SessionId};

/// How long typing keeps input after the last key or text.
pub const HOLD_AFTER_TYPING: Duration = Duration::from_millis(500);

/// How long a client holding a button or key may stay silent before what it
/// holds is released. Clients send [`crate::ClientMessage::KeepAlive`] or
/// pointer messages at least every [`crate::HEARTBEAT_INTERVAL`] while they
/// hold something. A P2P link can pause for about four seconds after a network
/// switch, so two arrivals can be about six seconds apart; this is the same
/// threshold a viewer uses before it shows Reconnecting.
pub const HELD_INPUT_TIMEOUT: Duration = RECONNECT_AFTER;

/// Keys one client may hold at once. Further key presses are dropped, which
/// bounds the memory a client can make the host keep.
pub const MAX_HELD_KEYS: usize = 32;

/// Whether an input event reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Pass it to the app.
    Accept,
    /// Drop it: another client holds input, or the client holds too many keys.
    Drop,
}

/// What a client held when it left or went silent, for the host to release on
/// the app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Released {
    /// Buttons to release.
    pub buttons: Buttons,
    /// Keys to release.
    pub keys: Vec<KeyCode>,
}

/// What one client holds down, and when it was last heard from.
#[derive(Debug)]
struct Client {
    session: SessionId,
    buttons: Buttons,
    keys: Vec<KeyCode>,
    last_event: Instant,
    /// What [`InputArbiter::expire`] released that the client has not
    /// reported released yet.
    expired: Released,
}

impl Client {
    /// Whether it holds a button or key and has not gone silent.
    fn holds_down(&self, now: Instant) -> bool {
        (!self.buttons.is_empty() || !self.keys.is_empty())
            && now < self.last_event + HELD_INPUT_TIMEOUT
    }
}

/// Arbitrates the input of one display between its clients.
///
/// Call [`InputArbiter::expire`] before handling each event and when
/// [`InputArbiter::next_change`] passes, so what a silent client held is
/// released on the app before someone else takes input.
#[derive(Debug, Default)]
pub struct InputArbiter {
    /// The client that last took input; it holds it while [`Self::holder`] says so.
    owner: Option<SessionId>,
    last_typed: Option<Instant>,
    clients: Vec<Client>,
}

impl InputArbiter {
    /// An arbiter where nobody holds input.
    pub fn new() -> Self {
        Self::default()
    }

    /// The client that holds input at `now`, if any.
    pub fn holder(&self, now: Instant) -> Option<SessionId> {
        let owner = self.owner?;
        let holds_down = self.client(owner).is_some_and(|c| c.holds_down(now));
        let typed = self
            .last_typed
            .is_some_and(|at| now < at + HOLD_AFTER_TYPING);
        (holds_down || typed).then_some(owner)
    }

    /// When the holder lets go of input or a silent client loses what it
    /// holds, if nothing else happens; for the host's timer.
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        let silence = self
            .clients
            .iter()
            .filter(|c| c.holds_down(now))
            .map(|c| c.last_event + HELD_INPUT_TIMEOUT);
        let typing = self
            .last_typed
            .map(|at| at + HOLD_AFTER_TYPING)
            .filter(|at| *at > now);
        silence.chain(typing).min()
    }

    /// Releases what clients silent for [`HELD_INPUT_TIMEOUT`] hold, and
    /// returns it per client for the host to release on the app.
    pub fn expire(&mut self, now: Instant) -> Vec<(SessionId, Released)> {
        let mut released = Vec::new();
        for client in &mut self.clients {
            let silent = now >= client.last_event + HELD_INPUT_TIMEOUT;
            if silent && (!client.buttons.is_empty() || !client.keys.is_empty()) {
                let held = Released {
                    buttons: std::mem::take(&mut client.buttons),
                    keys: std::mem::take(&mut client.keys),
                };
                client.expired.buttons = client.expired.buttons | held.buttons;
                // Bounded like held keys, so repeated silences cannot grow it.
                for key in &held.keys {
                    let keys = &mut client.expired.keys;
                    if !keys.contains(key) && keys.len() < MAX_HELD_KEYS {
                        keys.push(*key);
                    }
                }
                released.push((client.session, held));
            }
        }
        released
    }

    /// The client is there and sent nothing else: a keep-alive, or any message
    /// that is not input.
    pub fn alive(&mut self, session: SessionId, now: Instant) {
        self.entry(session, now);
    }

    /// Pointer motion with the buttons `session` holds now. Pressing a button
    /// takes input.
    pub fn pointer(&mut self, session: SessionId, buttons: Buttons, now: Instant) -> Verdict {
        let expired = &mut self.entry(session, now).expired.buttons;
        // Released buttons it reports up again are forgotten; while one is still
        // reported down, its events are dropped.
        *expired = Buttons(expired.0 & buttons.0);
        if !expired.is_empty() {
            return Verdict::Drop;
        }
        if self.blocked(session, now) {
            return Verdict::Drop;
        }
        let client = self.entry(session, now);
        let pressed = !buttons.pressed_since(client.buttons).is_empty();
        client.buttons = buttons;
        if pressed {
            self.owner = Some(session);
        }
        Verdict::Accept
    }

    /// A wheel event or the pointer leaving the window. It passes unless
    /// another client holds input, and does not take input.
    pub fn wheel(&mut self, session: SessionId, now: Instant) -> Verdict {
        self.entry(session, now);
        match self.blocked(session, now) {
            true => Verdict::Drop,
            false => Verdict::Accept,
        }
    }

    /// A key going down or up. Either takes input.
    pub fn key(&mut self, session: SessionId, code: KeyCode, down: bool, now: Instant) -> Verdict {
        let expired = &mut self.entry(session, now).expired.keys;
        if expired.contains(&code) {
            // The app already got this key's release from `expire`.
            if !down {
                expired.retain(|key| *key != code);
            }
            return Verdict::Drop;
        }
        if self.blocked(session, now) {
            return Verdict::Drop;
        }
        let keys = &mut self.entry(session, now).keys;
        let pressed = keys.contains(&code);
        match down {
            true if pressed => {}
            true if keys.len() >= MAX_HELD_KEYS => return Verdict::Drop,
            true => keys.push(code),
            false => keys.retain(|key| *key != code),
        }
        self.take(session, now);
        Verdict::Accept
    }

    /// Committed text. Takes input.
    pub fn text(&mut self, session: SessionId, now: Instant) -> Verdict {
        self.entry(session, now);
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
        let Some(index) = self.clients.iter().position(|c| c.session == session) else {
            return Released::default();
        };
        let client = self.clients.swap_remove(index);
        Released {
            buttons: client.buttons,
            keys: client.keys,
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

    /// The state of `session`, if it was ever heard from.
    fn client(&self, session: SessionId) -> Option<&Client> {
        self.clients.iter().find(|c| c.session == session)
    }

    /// The state of `session`, created on first use, marked as heard from at `now`.
    fn entry(&mut self, session: SessionId, now: Instant) -> &mut Client {
        let index = match self.clients.iter().position(|c| c.session == session) {
            Some(index) => index,
            None => {
                self.clients.push(Client {
                    session,
                    buttons: Buttons::NONE,
                    keys: Vec::new(),
                    last_event: now,
                    expired: Released::default(),
                });
                self.clients.len() - 1
            }
        };
        let client = &mut self.clients[index];
        client.last_event = client.last_event.max(now);
        client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SessionId = SessionId(1);
    const B: SessionId = SessionId(2);
    const KEY_A: KeyCode = KeyCode::keyboard(0x04);

    /// `n` milliseconds.
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_held_button_keeps_input_for_as_long_as_it_is_held() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        assert_eq!(arbiter.pointer(A, Buttons::LEFT, t0), Verdict::Accept);
        assert_eq!(arbiter.holder(t0), Some(A));

        let later = t0 + ms(4000);
        arbiter.alive(A, t0 + ms(2000));
        assert_eq!(arbiter.pointer(B, Buttons::NONE, later), Verdict::Drop);
        assert_eq!(arbiter.pointer(B, Buttons::LEFT, later), Verdict::Drop);
        assert_eq!(arbiter.wheel(B, later), Verdict::Drop);
        assert_eq!(arbiter.key(B, KEY_A, true, later), Verdict::Drop);
        assert_eq!(arbiter.holder(later), Some(A));

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
        assert_eq!(arbiter.next_change(t0 + ms(60)), Some(t0 + ms(560)));

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
        assert_eq!(arbiter.holder(t0 + ms(3000)), Some(A));
        assert_eq!(arbiter.text(B, t0 + ms(3000)), Verdict::Drop);
    }

    #[test]
    fn a_holder_that_goes_silent_loses_what_it_holds() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        arbiter.pointer(A, Buttons::LEFT, t0);
        arbiter.key(A, KEY_A, true, t0);
        arbiter.alive(A, t0 + ms(4000));
        let deadline = t0 + ms(4000) + HELD_INPUT_TIMEOUT;
        assert_eq!(arbiter.next_change(t0 + ms(4000)), Some(deadline));
        assert_eq!(arbiter.expire(deadline - ms(1)), Vec::new());
        assert_eq!(arbiter.text(B, deadline - ms(1)), Verdict::Drop);

        let released = arbiter.expire(deadline);
        assert_eq!(
            released,
            vec![(
                A,
                Released {
                    buttons: Buttons::LEFT,
                    keys: vec![KEY_A]
                }
            )]
        );
        assert_eq!(arbiter.holder(deadline), None);
        assert_eq!(arbiter.pointer(B, Buttons::LEFT, deadline), Verdict::Accept);
        assert_eq!(
            arbiter.expire(deadline + ms(1)),
            Vec::new(),
            "released once"
        );
    }

    #[test]
    fn a_client_back_from_silence_cannot_press_what_was_released_for_it_again() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        arbiter.pointer(A, Buttons::LEFT, t0);
        arbiter.key(A, KEY_A, true, t0);
        let back = t0 + HELD_INPUT_TIMEOUT;
        assert_eq!(arbiter.expire(back).len(), 1);

        // Its link recovers while the user still holds both.
        assert_eq!(arbiter.pointer(A, Buttons::LEFT, back), Verdict::Drop);
        assert_eq!(arbiter.key(A, KEY_A, true, back), Verdict::Drop);
        assert_eq!(arbiter.holder(back), None);
        // The releases end that, and are dropped too: the app had them already.
        assert_eq!(arbiter.key(A, KEY_A, false, back), Verdict::Drop);
        assert_eq!(arbiter.pointer(A, Buttons::NONE, back), Verdict::Accept);
        assert_eq!(arbiter.pointer(A, Buttons::LEFT, back), Verdict::Accept);
        assert_eq!(arbiter.key(A, KEY_A, true, back), Verdict::Accept);
        assert_eq!(arbiter.holder(back), Some(A));
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
    fn a_client_cannot_hold_more_than_the_key_limit() {
        let t0 = Instant::now();
        let mut arbiter = InputArbiter::new();
        for usage in 0..MAX_HELD_KEYS as u16 {
            assert_eq!(
                arbiter.key(A, KeyCode::keyboard(usage), true, t0),
                Verdict::Accept
            );
        }
        assert_eq!(
            arbiter.key(A, KeyCode::keyboard(0x99), true, t0),
            Verdict::Drop
        );
        assert_eq!(
            arbiter.key(A, KeyCode::keyboard(0), false, t0),
            Verdict::Accept
        );
        assert_eq!(arbiter.remove(A).keys.len(), MAX_HELD_KEYS - 1);
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
