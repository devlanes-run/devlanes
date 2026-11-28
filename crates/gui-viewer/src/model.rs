//! Every decision of the viewer, behind the [`Windows`] trait, so it runs and
//! is tested without opening a window or reading a clock.
//!
//! The [`Viewer`] takes what the connection reports (windows, frames, lock
//! states, the end of a connection) and what the local windows report (size,
//! close, focus, input), and answers with calls on [`Windows`] and messages
//! for the connection ([`Out`]). It keeps its windows across reconnects: the
//! client's lock state lasts one connection, so while the link is down every
//! window shows Reconnecting here, and a new connection's windows take over
//! the local windows of the old one, by id and else in order.

use std::time::{Duration, Instant};

use devlanes_gui_client::Event;
use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    Buttons, CursorShape, KeyCode, LockState, Point, Pointer, Size, Wheel, WheelUnit, WindowId,
    WindowInfo, WindowKind,
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
    /// Committed text.
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
    keys: Vec<KeyCode>,
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
    /// Whether the viewer may ask the daemon about the service.
    can_check: bool,
    /// When to ask next, while the link is down.
    check_at: Option<Instant>,
    /// When the viewer ends.
    end_at: Option<Instant>,
    /// The clipboard text last seen on either side, so neither echoes it.
    clipboard: Option<String>,
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
            can_check,
            check_at: None,
            end_at: None,
            clipboard: None,
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
    /// connection keep their local window: by id (a restarted app keeps its
    /// ids), else in order among main windows (a new host numbers anew).
    /// Local windows nothing takes over close.
    pub fn connected(&mut self, windows: Vec<WindowInfo>, now: Instant, ui: &mut impl Windows) {
        self.link = Link::Up;
        self.check_at = None;
        let mut taken = vec![false; self.windows.len()];
        let mut fresh = Vec::new();
        for info in windows {
            match self.index_of(info.id) {
                Some(index) if !taken[index] => {
                    taken[index] = true;
                    self.take_over(index, info, ui);
                }
                _ => fresh.push(info),
            }
        }
        let mut added = Vec::new();
        for info in fresh {
            let main = |kind: WindowKind| kind == WindowKind::Toplevel;
            let free = (0..self.windows.len())
                .find(|&index| !taken[index] && main(self.windows[index].info.kind));
            match free.filter(|_| main(info.kind)) {
                Some(index) => {
                    taken[index] = true;
                    self.take_over(index, info, ui);
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
            self.add(info, ui);
        }
        self.redraw_changed(now, ui);
    }

    /// The window at `index` now shows `info` of a new connection.
    fn take_over(&mut self, index: usize, info: WindowInfo, ui: &mut impl Windows) {
        let win = &mut self.windows[index];
        let size_changed = win.info.size != info.size && info.size.is_nonzero();
        win.lock = info.lock;
        win.closed_since = None;
        win.resize = Resize::new(info.size);
        win.info = info;
        if win.open {
            let (local, size) = (win.local, win.info.size);
            let spec = self.spec(&self.windows[index], false);
            ui.update(local, &spec);
            if size_changed {
                ui.set_size(local, size);
            }
        }
    }

    /// Adds `info` with a new local window, opened unless its parent's is
    /// closed.
    fn add(&mut self, info: WindowInfo, ui: &mut impl Windows) {
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
            resize: Resize::new(info.size),
            buttons: Buttons::NONE,
            keys: Vec::new(),
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
                    self.add(info, ui);
                }
            }
            Event::WindowChanged(info) => self.changed(info, ui),
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
                if let Some(drag) = frame::drag(decoration, started.edge)
                    && self.windows[index].open
                {
                    // The OS takes the pointer for the drag, so the release
                    // would never reach the app; it goes now, which also ends
                    // the app's own move.
                    self.release_buttons(index);
                    ui.drag(self.windows[index].local, drag);
                }
            }
            Event::Clipboard(text) if self.clipboard.as_deref() != Some(text.as_str()) => {
                ui.set_clipboard(&text);
                self.clipboard = Some(text);
            }
            // Applied sizes, the input holder and activity change nothing
            // here, and `disconnected` follows the end of the connection.
            _ => {}
        }
        self.redraw_changed(now, ui);
    }

    /// The host described a window anew.
    fn changed(&mut self, info: WindowInfo, ui: &mut impl Windows) {
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
            .then(|| win.resize.remote(info.size))
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

    /// The connection ended. When the host had closed every window, the
    /// service is gone; otherwise the windows show Reconnecting and the
    /// connection reconnects.
    pub fn disconnected(&mut self, now: Instant, ui: &mut impl Windows) {
        if self.link == Link::Over {
            return;
        }
        let gone = !self.windows.is_empty()
            && self.windows.iter().all(|win| win.lock == LockState::Closed);
        if gone {
            self.over(now);
        } else {
            self.link = Link::Down;
            self.check_at = self.can_check.then_some(now + CHECK_AFTER);
        }
        self.redraw_changed(now, ui);
    }

    /// The daemon said whether the service still runs, after
    /// [`Out::CheckUnit`]. A gone service ends the viewer; otherwise it asks
    /// again later while the link stays down.
    pub fn unit_checked(&mut self, running: bool, now: Instant, ui: &mut impl Windows) {
        if self.link != Link::Down {
            return;
        }
        if running {
            self.check_at = Some(now + CHECK_AFTER);
        } else {
            self.over(now);
            self.redraw_changed(now, ui);
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

    /// `local` gained or lost focus. On a gain, `clipboard` is the local
    /// clipboard's text, sent when it changed; on a loss, held keys and
    /// buttons are released, since their releases go to another window.
    pub fn focused(&mut self, local: LocalId, focused: bool, clipboard: Option<String>) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        if focused {
            if let Some(text) = clipboard
                && self.clipboard.as_deref() != Some(text.as_str())
            {
                self.clipboard = Some(text.clone());
                self.out.push(Out::Clipboard(text));
            }
        } else {
            self.release_all(index);
        }
        let window = self.windows[index].info.id;
        self.out.push(Out::Focus(window, focused));
    }

    /// The pointer moved to `position` in `local`.
    pub fn pointer_moved(&mut self, local: LocalId, position: Point, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let buttons = self.windows[index].buttons;
        if buttons.is_empty() && !self.accepts(index, now) {
            self.windows[index].pointer = Some(position);
            return;
        }
        self.send_pointer(index, position, buttons);
    }

    /// A mouse button went down or up in `local`. Releases of held buttons
    /// always go out, so the app never keeps a button the user let go of.
    pub fn button(&mut self, local: LocalId, button: Buttons, pressed: bool, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let held = self.windows[index].buttons;
        let buttons = if pressed {
            if !self.accepts(index, now) {
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
    pub fn pointer_left(&mut self, local: LocalId) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        let win = &mut self.windows[index];
        // A drag that leaves the window goes on on the app's side.
        if win.buttons.is_empty() {
            win.pointer = None;
            self.out.push(Out::PointerLeft(win.info.id));
        }
    }

    /// A scroll by `delta_x` and `delta_y` in `unit` in `local`.
    pub fn wheel(
        &mut self,
        local: LocalId,
        (delta_x, delta_y, unit): (f32, f32, WheelUnit),
        now: Instant,
    ) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        if !self.accepts(index, now) {
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

    /// A key went down or up in `local`. Releases of held keys always go out.
    pub fn key(&mut self, local: LocalId, code: KeyCode, pressed: bool, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        if pressed && !self.accepts(index, now) {
            return;
        }
        let win = &mut self.windows[index];
        let held = win.keys.contains(&code);
        if pressed && !held {
            win.keys.push(code);
        } else if !pressed {
            if !held {
                return;
            }
            win.keys.retain(|key| *key != code);
        }
        self.out.push(Out::Key(win.info.id, code, pressed));
    }

    /// Text committed in `local`, such as by an input method.
    pub fn text(&mut self, local: LocalId, text: String, now: Instant) {
        let Some(index) = self.index_of_local(local) else {
            return;
        };
        if !text.is_empty() && self.accepts(index, now) {
            self.out.push(Out::Text(self.windows[index].info.id, text));
        }
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

    /// Whether the window at `index` takes presses, motion, wheel, keys and
    /// text at `now`: when unlocked, and when frozen, where a click reaches
    /// the host as activity that thaws the lane.
    fn accepts(&self, index: usize, now: Instant) -> bool {
        matches!(
            self.shown(&self.windows[index], now),
            LockState::Unlocked | LockState::Frozen
        )
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
        for code in std::mem::take(&mut win.keys) {
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
