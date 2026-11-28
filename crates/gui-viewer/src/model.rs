//! Every decision of the viewer, behind the [`Windows`] trait, so it runs and
//! is tested without opening a window or reading a clock.
//!
//! The [`Viewer`] takes what the connection reports (windows, frames, lock
//! states, the end of a connection) and what the local windows report (size,
//! close, focus, input), and answers with calls on [`Windows`] and messages
//! for the connection ([`Out`]). It keeps its windows across reconnects: the
//! client's lock state lasts one connection, so while the link is down every
//! window shows Reconnecting here, and a new connection's windows take over
//! the local windows of the old one, by id, or in order for a new host.
//!
//! Keys go out as typed text where they type a character and as physical
//! keys otherwise ([`Viewer::key`]), so a layout other than the host's types
//! what its user sees on the keys, and shortcuts keep their positions. The
//! host inserts text exactly as given, whatever modifiers the viewer holds.

use std::time::{Duration, Instant};

use devlanes_gui_client::Event;
use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    Buttons, CursorShape, KeyCode, LockState, Point, Pointer, Size, Wheel, WheelUnit, WindowId,
    WindowInfo, WindowKind, keys,
};

use crate::frame::{self, CloseAction, Decoration, Drag, Os};
use crate::resize::Resize;

/// How long a window shows Closed before its local window goes, and how long
/// the viewer shows Closed before it ends once the service is gone.
pub const CLOSED_LINGER: Duration = Duration::from_secs(3);

/// How long the link may stay down before the viewer asks the daemon whether
/// the service still runs, and how often it asks again. Mirrored ports hold
/// connections for 10 seconds while a box link reconnects, and a restarting
/// service can take a while to start, so this leaves room for both.
pub const CHECK_AFTER: Duration = Duration::from_secs(30);

/// How long the daemon must stay unreachable before its service counts as
/// gone: longer than a `daemon restart` or an update takes, so neither ends
/// a viewer.
pub const DAEMON_AWAY: Duration = Duration::from_secs(30);

/// A local window, as the viewer numbers them. A local window outlives the
/// remote window it shows when a new connection's window takes its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub u64);

/// What a local window looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The title, the app's own.
    pub title: String,
    /// The size in physical pixels, one per window pixel.
    pub size: Size,
    /// The smallest size the app accepts.
    pub min_size: Option<Size>,
    /// The largest size the app accepts.
    pub max_size: Option<Size>,
    /// The frame.
    pub decoration: Decoration,
    /// For a popup, the local window it is placed in, and where.
    pub parent: Option<(LocalId, Point)>,
    /// Whether the window takes input; tooltips do not.
    pub takes_input: bool,
    /// Whether the window takes focus when it opens.
    pub focus: bool,
}

/// The local windows, as the OS layer implements them.
pub trait Windows {
    /// Opens a local window.
    fn open(&mut self, window: LocalId, spec: &Spec);
    /// Applies a changed title or size limits to an open window.
    fn update(&mut self, window: LocalId, spec: &Spec);
    /// Closes a local window.
    fn close(&mut self, window: LocalId);
    /// Gives a window the host's size.
    fn set_size(&mut self, window: LocalId, size: Size);
    /// Asks for a window to be drawn again from [`Viewer::view`].
    fn redraw(&mut self, window: LocalId);
    /// Shows the app's cursor over a window.
    fn set_cursor(&mut self, window: LocalId, shape: &CursorShape);
    /// Starts a local OS drag of a window, with the button still held.
    fn drag(&mut self, window: LocalId, drag: Drag);
    /// Brings a window to the front.
    fn focus(&mut self, window: LocalId);
    /// Puts text on the local clipboard.
    fn set_clipboard(&mut self, text: &str);
    /// Ends the viewer.
    fn exit(&mut self);
}

/// What the viewer asks of the connection.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Pointer motion and buttons.
    Pointer(Pointer),
    /// The pointer left a window with no button held.
    PointerLeft(WindowId),
    /// A scroll.
    Wheel(Wheel),
    /// A key going down or up.
    Key(WindowId, KeyCode, bool),
    /// Typed text.
    Text(WindowId, String),
    /// A resize to the size the user settled on.
    Resize(WindowId, Size),
    /// Ask the app to close a window.
    Close(WindowId),
    /// A local window gained or lost focus.
    Focus(WindowId, bool),
    /// The local clipboard's text.
    Clipboard(String),
    /// Stop reconnecting: the service is gone.
    StopReconnecting,
    /// Ask the daemon whether the service still runs, answered with
    /// [`Viewer::unit_checked`].
    CheckUnit,
}

/// What the daemon said about the service, after [`Out::CheckUnit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// It runs, or is on its way up.
    Runs,
    /// It stopped, failed, or no longer exists.
    Gone,
    /// The daemon does not run.
    DaemonAway,
    /// The answer said nothing either way.
    Unknown,
}

/// A key the local window reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    /// Its physical key, when the protocol has a code for it.
    pub code: Option<KeyCode>,
    /// Down or up.
    pub pressed: bool,
    /// A press the OS repeats while the key is held.
    pub repeat: bool,
    /// The text the press types with the local layout and modifiers.
    pub text: Option<String>,
    /// A dead key, whose character comes with the next press.
    pub dead: bool,
    /// For a keypad key, the navigation key its local meaning names (keypad
    /// 4 is Left with Num Lock off), sent in its place when it types nothing.
    pub navigation: Option<KeyCode>,
}

/// The modifiers held with a key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Control.
    pub ctrl: bool,
    /// Alt (Option).
    pub alt: bool,
    /// Super, Windows or Command.
    pub meta: bool,
}

/// What a local window draws.
#[derive(Debug)]
pub struct View<'a> {
    /// The last frame, if one came.
    pub frame: Option<&'a Frame>,
    /// The lock state the window shows; anything but Unlocked dims the frame
    /// under [`overlay_text`].
    pub lock: LockState,
}

/// The one line the overlay of a locked window shows.
pub fn overlay_text(lock: LockState) -> Option<&'static str> {
    Some(match lock {
        LockState::Unlocked => return None,
        LockState::Resizing => "RESIZING",
        LockState::Waiting => "WAITING FOR THE APP",
        LockState::Reconnecting => "RECONNECTING",
        LockState::Frozen => "FROZEN: CLICK TO WAKE THE LANE",
        LockState::Closed => "CLOSED",
        LockState::Locked => "LOCKED",
    })
}

/// The state of the connection, as the viewer sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    /// Connected.
    Up,
    /// Down, and reconnecting.
    Down,
    /// The service is gone; nothing reconnects.
    Over,
}

/// One remote window and its local window.
#[derive(Debug)]
struct Win {
    local: LocalId,
    info: WindowInfo,
    /// The client's lock state for this connection.
    lock: LockState,
    /// The lock state last drawn.
    drawn: LockState,
    /// Whether the local window is open.
    open: bool,
    /// The user closed the local window; it stays closed until `gui open`.
    hidden: bool,
    frame: Option<Frame>,
    resize: Resize,
    buttons: Buttons,
    /// Keys held down as physical keys: the local key and the code sent for
    /// it, which its release sends too.
    keys: Vec<(KeyCode, KeyCode)>,
    /// Keys whose press went out as text, whose release is not sent.
    texted: Vec<KeyCode>,
    pointer: Option<Point>,
    /// Since when the window shows Closed.
    closed_since: Option<Instant>,
}

/// The viewer's state and decisions.
#[derive(Debug)]
pub struct Viewer {
    os: Os,
    /// Whether new windows take focus as they open.
    focus_new: bool,
    windows: Vec<Win>,
    next_local: u64,
    link: Link,
    /// The host showed every window Closed: the service stopped.
    closed_by_host: bool,
    /// Whether the viewer may ask the daemon about the service.
    can_check: bool,
    /// When to ask next, while the link is down.
    check_at: Option<Instant>,
    /// Since when the daemon has not answered the checks.
    away_since: Option<Instant>,
    /// When the viewer ends.
    end_at: Option<Instant>,
    /// The clipboard text last seen on either side, so neither echoes it.
    clipboard: Option<String>,
    /// Local clipboard text read while the link was down, sent once it is up.
    clipboard_pending: Option<String>,
    /// How many clipboard texts the host has sent.
    host_clipboards: u64,
    /// The local window that reported a focus loss this turn of the event
    /// loop; the loss counts only when no other window of the service gained
    /// focus by [`Viewer::settle_focus`].
    blurred: Option<LocalId>,
    out: Vec<Out>,
}

impl Viewer {
    /// A viewer on `os`, not connected yet, whose new windows take focus
    /// when `focus_new`, and that asks the daemon about the service while it
    /// cannot reconnect when `can_check`.
    pub fn new(os: Os, focus_new: bool, can_check: bool) -> Self {
        Self {
            os,
            focus_new,
            windows: Vec::new(),
            next_local: 1,
            link: Link::Down,
            closed_by_host: false,
            can_check,
            check_at: None,
            away_since: None,
            end_at: None,
            clipboard: None,
            clipboard_pending: None,
            host_clipboards: 0,
            blurred: None,
            out: Vec::new(),
        }
    }

    /// What to send, in order, since the last call.
    pub fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    /// What `local` draws at `now`.
    pub fn view(&self, local: LocalId, now: Instant) -> Option<View<'_>> {
        let win = self.windows.iter().find(|win| win.local == local)?;
        Some(View {
            frame: win.frame.as_ref(),
            lock: self.shown(win, now),
        })
    }

    /// A connection came up with `windows`. Windows known from the last
    /// connection keep their local window by id (an app restarted under the
    /// same host keeps its ids). When no id matches, the host is a new one
    /// that numbers anew, and its main windows take over the old main windows
    /// in order. Local windows nothing takes over close.
    pub fn connected(&mut self, windows: Vec<WindowInfo>, now: Instant, ui: &mut impl Windows) {
        self.link = Link::Up;
        self.check_at = None;
        self.away_since = None;
        self.closed_by_host = false;
        let mut taken = vec![false; self.windows.len()];
        let mut fresh = Vec::new();
        for info in windows {
            match self.index_of(info.id) {
                Some(index) if !taken[index] => {
                    taken[index] = true;
                    self.take_over(index, info, now, ui);
                }
                _ => fresh.push(info),
            }
        }
        let new_host = !taken.contains(&true);
        let mut added = Vec::new();
        for info in fresh {
            let main = |kind: WindowKind| kind == WindowKind::Toplevel;
            let free = (0..self.windows.len())
                .find(|&index| !taken[index] && main(self.windows[index].info.kind))
                .filter(|_| new_host && main(info.kind));
            match free {
                Some(index) => {
                    taken[index] = true;
                    self.take_over(index, info, now, ui);
                }
                None => added.push(info),
            }
        }
        let mut index = 0;
        self.windows.retain(|win| {
            let keep = taken[index];
            index += 1;
            if !keep && win.open {
                ui.close(win.local);
            }
            keep
        });
        for info in added {
            self.add(info, now, ui);
        }
        if let Some(text) = self.clipboard_pending.take() {
            self.send_clipboard(text);
        }
        self.redraw_changed(now, ui);
    }

    /// The window at `index` now shows `info` of a new connection. Its size
    /// is reconciled with the local window's: see [`Resize::reconnected`].
    fn take_over(&mut self, index: usize, info: WindowInfo, now: Instant, ui: &mut impl Windows) {
        let win = &mut self.windows[index];
        let size = win.resize.reconnected(info.size, now);
        win.lock = info.lock;
        win.closed_since = None;
        win.info = info;
        if win.open {
            let local = win.local;
            let spec = self.spec(&self.windows[index], false);
            ui.update(local, &spec);
            if let Some(size) = size {
                ui.set_size(local, size);
            }
        }
    }

    /// Adds `info` with a new local window, opened unless its parent's is
    /// closed.
    fn add(&mut self, info: WindowInfo, now: Instant, ui: &mut impl Windows) {
        let local = LocalId(self.next_local);
        self.next_local += 1;
        let hidden = info
            .parent
            .and_then(|parent| self.windows.iter().find(|win| win.info.id == parent))
            .is_some_and(|parent| !parent.open);
        self.windows.push(Win {
            local,
            lock: info.lock,
            drawn: LockState::Unlocked,
            open: false,
            hidden,
            frame: None,
            resize: Resize::new(info.size, now),
            buttons: Buttons::NONE,
            keys: Vec::new(),
            texted: Vec::new(),
            pointer: None,
            closed_since: None,
            info,
        });
        if !hidden {
            self.open_local(self.windows.len() - 1, self.focus_new, ui);
        }
    }

    /// Opens the local window of the window at `index`.
    fn open_local(&mut self, index: usize, focus: bool, ui: &mut impl Windows) {
        let spec = self.spec(&self.windows[index], focus);
        let win = &mut self.windows[index];
        win.open = true;
        ui.open(win.local, &spec);
    }

    /// The local window's look for `win`.
    fn spec(&self, win: &Win, focus: bool) -> Spec {
        let decoration = frame::decoration(&win.info, self.os);
        let parent = win.info.parent.and_then(|parent| {
            let parent = self.windows.iter().find(|w| w.info.id == parent)?;
            Some((parent.local, win.info.offset))
        });
        Spec {
            title: win.info.title.clone(),
            size: win.info.size,
            min_size: win.info.min_size,
            max_size: win.info.max_size,
            decoration,
            parent: parent.filter(|_| decoration == Decoration::Popup),
            takes_input: win.info.kind != WindowKind::Tooltip,
            focus: focus && decoration != Decoration::Popup,
        }
    }

    /// Handles an event of the connection.
    pub fn event(&mut self, event: Event, now: Instant, ui: &mut impl Windows) {
        match event {
            Event::WindowAdded(info) => {
                if self.index_of(info.id).is_none() {
                    self.closed_by_host = false;
                    self.add(info, now, ui);
                }
            }
            Event::WindowChanged(info) => self.changed(info, now, ui),
            Event::WindowRemoved(window) => {
                if let Some(index) = self.index_of(window) {
                    let win = self.windows.remove(index);
                    if win.open {
                        ui.close(win.local);
                    }
                }
            }
            Event::Frame(decoded) => {
                if let Some(index) = self.index_of(decoded.window) {
                    let win = &mut self.windows[index];
                    win.frame = Some(decoded.frame);
                    if win.open {
                        ui.redraw(win.local);
                    }
                }
            }
            Event::Lock { window, state } => {
                if let Some(index) = self.index_of(window) {
                    self.windows[index].lock = state;
                    self.windows[index].resize.client_reported();
                }
                // Remembered, since windows that showed Closed go away
                // before the connection ends.
                self.closed_by_host = !self.windows.is_empty()
                    && self.windows.iter().all(|win| win.lock == LockState::Closed);
            }
            Event::Cursor(cursor) => {
                if let Some(index) = self.index_of(cursor.window)
                    && self.windows[index].open
                {
                    ui.set_cursor(self.windows[index].local, &cursor.shape);
                }
            }
            Event::MoveResizeStarted(started) => {
                let Some(index) = self.index_of(started.window) else {
                    return;
                };
                let decoration = frame::decoration(&self.windows[index].info, self.os);
                let win = &self.windows[index];
                // The OS drags only with the left button down, as the app's
                // title bar did when it asked.
                if let Some(drag) = frame::drag(decoration, started.edge)
                    && win.open
                    && win.buttons.contains(Buttons::LEFT)
                {
                    // The OS takes the pointer for the drag, so the release
                    // would never reach the app; it goes now, which also ends
                    // the app's own move.
                    self.release_buttons(index);
                    ui.drag(self.windows[index].local, drag);
                }
            }
            Event::Clipboard(text) => {
                self.host_clipboards += 1;
                if self.clipboard.as_deref() != Some(text.as_str()) {
                    ui.set_clipboard(&text);
                    self.clipboard = Some(text);
                }
            }
            // Applied sizes, the input holder and activity change nothing
            // here, and `disconnected` follows the end of the connection.
            _ => {}
        }
        self.redraw_changed(now, ui);
    }

    /// The host described a window anew.
    fn changed(&mut self, info: WindowInfo, now: Instant, ui: &mut impl Windows) {
        let Some(index) = self.index_of(info.id) else {
            return;
        };
        let win = &mut self.windows[index];
        // The client reports the end of its own resize as a lock event; the
        // host's description does not know about that resize.
        win.lock = match (win.lock, info.lock) {
            (LockState::Resizing, LockState::Unlocked) => LockState::Resizing,
            (_, host) => host,
        };
        let size = (win.info.size != info.size && info.size.is_nonzero())
            .then(|| win.resize.remote(info.size, now))
            .flatten();
        let looks_changed = win.info.title != info.title
            || win.info.min_size != info.min_size
            || win.info.max_size != info.max_size;
        win.info = info;
        if !win.open {
            return;
        }
        let local = win.local;
        if looks_changed {
            let spec = self.spec(&self.windows[index], false);
            ui.update(local, &spec);
        }
        if let Some(size) = size {
            ui.set_size(local, size);
        }
    }

    /// The connection ended. When the host had shown every window Closed,
    /// the service is gone; otherwise the windows show Reconnecting and the
    /// connection reconnects.
    pub fn disconnected(&mut self, now: Instant, ui: &mut impl Windows) {
        if self.link == Link::Over {
            return;
        }
        if self.closed_by_host {
            self.over(now);
        } else {
            self.link = Link::Down;
            self.check_at = self.can_check.then_some(now + CHECK_AFTER);
        }
        self.redraw_changed(now, ui);
    }

    /// The daemon's answer about the service, after [`Out::CheckUnit`]. A
    /// gone service ends the viewer, and so does a daemon that has not
    /// answered for [`DAEMON_AWAY`]; otherwise it asks again later while the
    /// link stays down.
    pub fn unit_checked(&mut self, check: Check, now: Instant, ui: &mut impl Windows) {
        if self.link != Link::Down {
            return;
        }
        let gone = match check {
            Check::Gone => true,
            Check::DaemonAway => {
                let since = *self.away_since.get_or_insert(now);
                now >= since + DAEMON_AWAY
            }
            Check::Runs => {
                self.away_since = None;
                false
            }
            Check::Unknown => false,
        };
        if gone {
            self.over(now);
            self.redraw_changed(now, ui);
        } else {
            self.check_at = Some(now + CHECK_AFTER);
        }
    }

    /// The service is gone: every window shows Closed, nothing reconnects,
    /// and the viewer ends after [`CLOSED_LINGER`].
    fn over(&mut self, now: Instant) {
        self.link = Link::Over;
        self.check_at = None;
        self.end_at = Some(now + CLOSED_LINGER);
        self.out.push(Out::StopReconnecting);
    }

    /// `devlanes gui open` asked again: windows the user closed open again,
    /// and the main windows come to the front.
    pub fn show(&mut self, ui: &mut impl Windows) {
        // Parents first, so their popups find their local window.
        for index in 0..self.windows.len() {
            if self.windows[index].hidden {
                self.windows[index].hidden = false;
                self.open_local(index, true, ui);
            }
        }
        for win in &self.windows {
            if win.open && frame::decoration(&win.info, self.os) != Decoration::Popup {
                ui.focus(win.local);
            }
        }
    }

    /// The user closed `local` with its outer close button; see
    /// [`frame::close_action`].
    pub fn close_requested(&mut self, local: LocalId, ui: &mut impl Windows) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let window = self.windows[index].info.id;
        match frame::close_action(&self.windows[index].info) {
            CloseAction::App => self.out.push(Out::Close(window)),
            CloseAction::Local => {
                self.release_all(index);
                if self.blurred == Some(local) {
                    self.blurred = None;
                }
                for win in &mut self.windows {
                    if win.local == local || win.info.parent == Some(window) {
                        win.hidden = true;
                        if win.open {
                            win.open = false;
                            ui.close(win.local);
                        }
                    }
                }
            }
        }
    }

    /// `local` reports its size.
    pub fn resized(&mut self, local: LocalId, size: Size, now: Instant, ui: &mut impl Windows) {
        if let Some(index) = self.index_of_local(local) {
            self.windows[index].resize.local(size, now);
            ui.redraw(local);
        }
        self.redraw_changed(now, ui);
    }

    /// `local` gained or lost focus. A gain is said at once and cancels a
    /// loss of another window of the service reported in the same turn of
    /// the event loop (OSes report the loss first), so moving between the
    /// viewer's windows does not look like leaving the app. A loss waits for
    /// [`Viewer::settle_focus`].
    pub fn focus(&mut self, local: LocalId, focused: bool, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        if !focused {
            self.blurred = Some(local);
            return;
        }
        if self.blurred == Some(local) {
            self.blurred = None;
            return;
        }
        if let Some(other) = self
            .blurred
            .take()
            .and_then(|other| self.index_of_local(other))
        {
            // Its keys went to the window that has focus now.
            let held = std::mem::take(&mut self.windows[other].keys);
            let window = self.windows[other].info.id;
            self.windows[other].texted.clear();
            for (_, code) in held {
                self.out.push(Out::Key(window, code, false));
            }
        }
        if self.shown(&self.windows[index], now) != LockState::Frozen {
            self.out.push(Out::Focus(self.windows[index].info.id, true));
        }
    }

    /// At the end of a turn of the event loop: a focus loss no other window
    /// of the service took over means focus left the app, which releases
    /// what was held and tells the host.
    pub fn settle_focus(&mut self, now: Instant) {
        let Some(index) = self
            .blurred
            .take()
            .and_then(|local| self.index_of_local(local))
        else {
            return;
        };
        self.release_all(index);
        if self.shown(&self.windows[index], now) != LockState::Frozen {
            self.out
                .push(Out::Focus(self.windows[index].info.id, false));
        }
    }

    /// How many clipboard texts the host has sent, for a read of the local
    /// clipboard to say when it started.
    pub fn host_clipboards(&self) -> u64 {
        self.host_clipboards
    }

    /// The local clipboard's text, read when a window of the service gained
    /// focus, from a read that started when [`Viewer::host_clipboards`] was
    /// `started`. Sent when it changed, once the link can carry it. A read
    /// the host's clipboard overtook is dropped: the host's text is newer,
    /// and the read may predate it on the local clipboard.
    pub fn local_clipboard(&mut self, text: String, started: u64) {
        if started != self.host_clipboards || self.clipboard.as_deref() == Some(text.as_str()) {
            return;
        }
        if self.link == Link::Up {
            self.send_clipboard(text);
        } else {
            self.clipboard_pending = Some(text);
        }
    }

    /// Sends `text` as the clipboard and records it as seen.
    fn send_clipboard(&mut self, text: String) {
        if self.clipboard.as_deref() != Some(text.as_str()) {
            self.clipboard = Some(text.clone());
            self.out.push(Out::Clipboard(text));
        }
    }

    /// The pointer moved to `position` in `local`. Motion goes out while the
    /// window is unlocked, and with a button held while it is locked for
    /// anything but a frozen lane, where only presses go, since every input
    /// the host gets would thaw the lane.
    pub fn pointer_moved(&mut self, local: LocalId, position: Point, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let buttons = self.windows[index].buttons;
        let shown = self.shown(&self.windows[index], now);
        let sends =
            shown == LockState::Unlocked || (!buttons.is_empty() && shown != LockState::Frozen);
        if !sends {
            self.windows[index].pointer = Some(position);
            return;
        }
        self.send_pointer(index, position, buttons);
    }

    /// A mouse button went down or up in `local`. Presses go out while the
    /// window is unlocked or frozen (a click thaws the lane); releases of
    /// held buttons always do, so the app never keeps a button the user let go
    /// of.
    pub fn button(&mut self, local: LocalId, button: Buttons, pressed: bool, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let held = self.windows[index].buttons;
        let buttons = if pressed {
            if !matches!(
                self.shown(&self.windows[index], now),
                LockState::Unlocked | LockState::Frozen
            ) {
                return;
            }
            held | button
        } else if held.contains(button) {
            Buttons(held.0 & !button.0)
        } else {
            return;
        };
        let position = self.windows[index].pointer.unwrap_or(Point::new(0, 0));
        self.send_pointer(index, position, buttons);
    }

    /// The pointer left `local`.
    pub fn pointer_left(&mut self, local: LocalId, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let frozen = self.shown(&self.windows[index], now) == LockState::Frozen;
        let win = &mut self.windows[index];
        // A drag that leaves the window goes on on the app's side.
        if win.buttons.is_empty() {
            win.pointer = None;
            if !frozen {
                self.out.push(Out::PointerLeft(win.info.id));
            }
        }
    }

    /// A scroll by `delta_x` and `delta_y` in `unit` in `local`, while the
    /// window is unlocked.
    pub fn wheel(
        &mut self,
        local: LocalId,
        (delta_x, delta_y, unit): (f32, f32, WheelUnit),
        now: Instant,
    ) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        if self.shown(&self.windows[index], now) != LockState::Unlocked {
            return;
        }
        let win = &self.windows[index];
        if let Some(position) = win.pointer {
            self.out.push(Out::Wheel(Wheel {
                window: win.info.id,
                position,
                delta_x,
                delta_y,
                unit,
            }));
        }
    }

    /// A key went down or up in `local`, with `modifiers` held.
    ///
    /// A press that types a character with the local layout goes out as that
    /// text, and its release not at all; a repeat of it repeats the text.
    /// Shortcuts stay physical keys, so they keep their positions: presses
    /// with Control without Alt (Control with Alt is AltGr on Windows), with
    /// Super or Command, and on Windows and Linux with Alt without Control
    /// (menu mnemonics; Option types characters on macOS). A key keeps the
    /// way its press went out until its release, whatever the modifiers do
    /// meanwhile. Space and keys that type nothing go out as physical keys,
    /// a keypad key that types nothing as the navigation key it means; the
    /// app repeats a held one itself. Caps Lock and Num Lock stay local: text
    /// already has their effect, and the host types text as given. A dead
    /// key sends nothing; its character comes as the text of the next press.
    /// Presses go out only while the window is unlocked; releases of held
    /// keys always do.
    pub fn key(&mut self, local: LocalId, input: KeyInput, modifiers: Modifiers, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let window = self.windows[index].info.id;
        let Some(code) = input.code else {
            // Only text can go without a code; a release has none.
            if input.pressed
                && !input.dead
                && self.shown(&self.windows[index], now) == LockState::Unlocked
                && let Some(text) = self.text_of(&input, modifiers)
            {
                self.out.push(Out::Text(window, text));
            }
            return;
        };
        let win = &mut self.windows[index];
        if !input.pressed {
            if let Some(at) = win.keys.iter().position(|(held, _)| *held == code) {
                let (_, sent) = win.keys.remove(at);
                self.out.push(Out::Key(window, sent, false));
            } else if let Some(at) = win.texted.iter().position(|held| *held == code) {
                win.texted.remove(at);
            }
            return;
        }
        // A key that went down as a physical key stays one; the app repeats
        // it. One whose press went as text never sends a key press, so its
        // release needs none either.
        if win.keys.iter().any(|(held, _)| *held == code)
            || code == keys::CAPS_LOCK
            || code == keys::NUM_LOCK
        {
            return;
        }
        if self.shown(&self.windows[index], now) != LockState::Unlocked || input.dead {
            return;
        }
        let text = self.text_of(&input, modifiers);
        let win = &mut self.windows[index];
        if let Some(text) = text {
            if !win.texted.contains(&code) {
                win.texted.push(code);
            }
            self.out.push(Out::Text(window, text));
            return;
        }
        if input.repeat || win.texted.contains(&code) {
            return;
        }
        let sent = input.navigation.unwrap_or(code);
        win.keys.push((code, sent));
        self.out.push(Out::Key(window, sent, true));
    }

    /// The text a press sends, or `None` when it goes as a physical key.
    fn text_of(&self, input: &KeyInput, modifiers: Modifiers) -> Option<String> {
        let mnemonic = modifiers.alt && !modifiers.ctrl && self.os != Os::Mac;
        let shortcut = (modifiers.ctrl && !modifiers.alt) || modifiers.meta || mnemonic;
        input.text.clone().filter(|text| {
            !shortcut && !text.is_empty() && text != " " && !text.chars().any(char::is_control)
        })
    }

    /// Does what is due at `now`: sends settled sizes, closes windows that
    /// showed Closed long enough, asks the daemon, ends the viewer, and
    /// redraws windows whose lock state changed.
    pub fn tick(&mut self, now: Instant, ui: &mut impl Windows) {
        for win in &mut self.windows {
            if let Some(size) = win.resize.due(now) {
                self.out.push(Out::Resize(win.info.id, size));
            }
        }
        if self.check_at.is_some_and(|at| now >= at) {
            self.check_at = None;
            self.out.push(Out::CheckUnit);
        }
        if self.end_at.is_some_and(|at| now >= at) {
            self.end_at = None;
            for win in &mut self.windows {
                if win.open {
                    win.open = false;
                    ui.close(win.local);
                }
            }
            ui.exit();
            return;
        }
        if self.link == Link::Up {
            // A window the host closed for good while the service runs goes
            // once it showed Closed for a moment.
            self.windows.retain(|win| {
                let gone = win
                    .closed_since
                    .is_some_and(|since| now >= since + CLOSED_LINGER);
                if gone && win.open {
                    ui.close(win.local);
                }
                !gone
            });
        }
        self.redraw_changed(now, ui);
    }

    /// When [`Viewer::tick`] has something to do next.
    pub fn next_deadline(&self) -> Option<Instant> {
        let link_up = self.link == Link::Up;
        self.windows
            .iter()
            .flat_map(|win| {
                [
                    win.resize.next_change(),
                    win.closed_since
                        .filter(|_| link_up)
                        .map(|since| since + CLOSED_LINGER),
                ]
            })
            .chain([self.check_at, self.end_at])
            .flatten()
            .min()
    }

    /// The state `win` shows at `now`. Closed from the host or for the
    /// service wins; then the viewer's own link; then the client's state for
    /// this connection; then the viewer's own resize.
    fn shown(&self, win: &Win, now: Instant) -> LockState {
        if self.link == Link::Over || win.lock == LockState::Closed {
            LockState::Closed
        } else if self.link != Link::Up {
            LockState::Reconnecting
        } else if win.lock.is_locked() {
            win.lock
        } else if win.resize.locks(now) {
            LockState::Resizing
        } else {
            LockState::Unlocked
        }
    }

    /// Sends the pointer at `position` with `buttons` for the window at
    /// `index`, with the on-screen movement while a button stays held.
    fn send_pointer(&mut self, index: usize, position: Point, buttons: Buttons) {
        let win = &mut self.windows[index];
        let screen_delta = match win.pointer {
            Some(last) if !buttons.is_empty() && !win.buttons.is_empty() => {
                Some(Point::new(position.x - last.x, position.y - last.y))
            }
            _ => None,
        };
        win.pointer = Some(position);
        win.buttons = buttons;
        self.out.push(Out::Pointer(Pointer {
            window: win.info.id,
            position,
            buttons,
            screen_delta,
        }));
    }

    /// Releases the held buttons of the window at `index`.
    fn release_buttons(&mut self, index: usize) {
        if !self.windows[index].buttons.is_empty() {
            let position = self.windows[index].pointer.unwrap_or(Point::new(0, 0));
            self.send_pointer(index, position, Buttons::NONE);
        }
    }

    /// Releases every held key and button of the window at `index`.
    fn release_all(&mut self, index: usize) {
        let win = &mut self.windows[index];
        let window = win.info.id;
        win.texted.clear();
        for (_, code) in std::mem::take(&mut win.keys) {
            self.out.push(Out::Key(window, code, false));
        }
        self.release_buttons(index);
    }

    /// Redraws open windows whose shown state changed, and notes since when
    /// a window shows Closed.
    fn redraw_changed(&mut self, now: Instant, ui: &mut impl Windows) {
        for index in 0..self.windows.len() {
            let shown = self.shown(&self.windows[index], now);
            let win = &mut self.windows[index];
            if shown == LockState::Closed {
                win.closed_since.get_or_insert(now);
            } else {
                win.closed_since = None;
            }
            if shown != win.drawn {
                win.drawn = shown;
                if win.open {
                    ui.redraw(win.local);
                }
            }
        }
    }

    /// The index of remote window `window`.
    fn index_of(&self, window: WindowId) -> Option<usize> {
        self.windows.iter().position(|win| win.info.id == window)
    }

    /// The index of local window `local`.
    fn index_of_local(&self, local: LocalId) -> Option<usize> {
        self.windows.iter().position(|win| win.local == local)
    }
}
