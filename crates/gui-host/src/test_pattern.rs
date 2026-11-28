//! A backend with no app behind it: windows that draw the colour grid of
//! [`crate::pattern`] and a counter, and that echo input into their picture
//! and into a list a test can read.
//!
//! It does its work inside [`BackendControl::send`]. Drawing a small grid
//! takes microseconds, and staying on the caller's thread keeps tests on a
//! paused clock deterministic.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    Buttons, KeyCode, LockState, MAX_DIMENSION, Point, Size, WindowId, WindowInfo, WindowKind,
};

use crate::backend::{
    Backend, BackendControl, BackendError, CapturedFrame, Command, Event, EventSink,
};
use crate::pattern::draw_grid;

/// The app id of every test-pattern window.
pub const APP_ID: &str = "run.devlanes.test-pattern";

/// How the test pattern behaves.
#[derive(Debug, Clone)]
pub struct TestPatternOptions {
    /// The size of the first window and the default for new ones.
    pub size: Size,
    /// The smallest size windows take, the way toolkits clamp a resize.
    pub min_size: Option<Size>,
    /// Send frames only when the picture changes, as Windows capture does:
    /// a resize that keeps the size draws nothing, and nothing is redrawn
    /// unless input changed the picture.
    pub changes_only: bool,
    /// What [`Backend::real_desktop`] answers.
    pub real_desktop: bool,
}

impl Default for TestPatternOptions {
    /// One 320x200 window, no clamp, frames on every redraw.
    fn default() -> Self {
        Self {
            size: Size::new(320, 200),
            min_size: None,
            changes_only: false,
            real_desktop: false,
        }
    }
}

/// What a test-pattern window received, the way an app would see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Echo {
    /// A button went down at `position`.
    Pressed {
        /// The window.
        window: WindowId,
        /// Where, in window pixels.
        position: Point,
        /// Every button held after the press.
        buttons: Buttons,
    },
    /// A key went down or up.
    Key {
        /// The window.
        window: WindowId,
        /// The key.
        code: KeyCode,
        /// Down or up.
        down: bool,
    },
    /// Text was typed.
    Text {
        /// The window.
        window: WindowId,
        /// The text.
        text: String,
    },
    /// The window took a new size.
    Resized {
        /// The window.
        window: WindowId,
        /// The size it took, after the clamp.
        size: Size,
    },
    /// The window closed on request.
    Closed {
        /// The window.
        window: WindowId,
    },
}

/// One window of the pattern.
#[derive(Debug)]
struct PatternWindow {
    info: WindowInfo,
    /// Where the last press was; drawn as a small black square.
    marker: Option<Point>,
    /// Redraws so far; drawn as bits along the bottom row.
    counter: u32,
    last: Option<Frame>,
}

/// The pattern's state, shared by the backend and its handle.
#[derive(Debug)]
struct Pattern {
    options: TestPatternOptions,
    sink: Option<EventSink>,
    windows: BTreeMap<WindowId, PatternWindow>,
    next_id: u32,
    buttons: Buttons,
    commands: Vec<Command>,
    echoes: Vec<Echo>,
    hold: bool,
    held: Vec<Event>,
    epoch: Instant,
}

impl Pattern {
    /// Sends `event`, unless the pattern vanished. Frames wait while held.
    fn emit(&mut self, event: Event) {
        if self.hold && matches!(event, Event::Frame(_)) {
            self.held.push(event);
            return;
        }
        if let Some(sink) = &self.sink {
            // A host that stopped needs nothing more.
            let _ = sink.send(event);
        }
    }

    /// Adds a window and announces it with a first frame once started.
    fn add(
        &mut self,
        size: Size,
        kind: WindowKind,
        parent: Option<WindowId>,
        offset: Point,
    ) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        let info = WindowInfo {
            id,
            title: format!("Test pattern {}", id.0),
            app_id: APP_ID.to_owned(),
            size: self.clamp(size),
            min_size: self.options.min_size,
            max_size: None,
            draws_own_frame: false,
            reports_moves: false,
            parent,
            offset,
            lock: LockState::Unlocked,
            kind,
        };
        self.windows.insert(
            id,
            PatternWindow {
                info: info.clone(),
                marker: None,
                counter: 0,
                last: None,
            },
        );
        if self.sink.is_some() {
            self.emit(Event::WindowAdded(info));
            self.redraw(id, None);
        }
        id
    }

    /// `size` within the clamp and the protocol's limits.
    fn clamp(&self, size: Size) -> Size {
        let min = self.options.min_size.unwrap_or(Size::new(1, 1));
        Size::new(
            size.width.clamp(min.width.max(1), MAX_DIMENSION),
            size.height.clamp(min.height.max(1), MAX_DIMENSION),
        )
    }

    /// Draws `window` again and sends the frame.
    fn redraw(&mut self, window: WindowId, answers: Option<u64>) {
        let epoch = self.epoch;
        let Some(win) = self.windows.get_mut(&window) else {
            return;
        };
        win.counter += 1;
        let Some(frame) = draw(win, epoch) else {
            return;
        };
        win.last = Some(frame.clone());
        self.emit(Event::Frame(CapturedFrame {
            window,
            frame,
            damage: Vec::new(),
            answers,
        }));
    }

    /// Handles one command from the host.
    fn handle(&mut self, command: Command) {
        self.commands.push(command.clone());
        match command {
            Command::Pointer(pointer) => {
                let pressed = pointer.buttons.pressed_since(self.buttons);
                self.buttons = pointer.buttons;
                if !pressed.is_empty() && self.windows.contains_key(&pointer.window) {
                    if let Some(win) = self.windows.get_mut(&pointer.window) {
                        win.marker = Some(pointer.position);
                    }
                    self.echoes.push(Echo::Pressed {
                        window: pointer.window,
                        position: pointer.position,
                        buttons: pointer.buttons,
                    });
                    self.redraw(pointer.window, None);
                }
            }
            Command::Key(key) => {
                self.echoes.push(Echo::Key {
                    window: key.window,
                    code: key.code,
                    down: key.down,
                });
                if key.down {
                    self.redraw(key.window, None);
                }
            }
            Command::Text { window, text } => {
                self.echoes.push(Echo::Text { window, text });
                self.redraw(window, None);
            }
            Command::Resize { window, size, id } => {
                let size = self.clamp(size);
                let Some(win) = self.windows.get_mut(&window) else {
                    return;
                };
                let changed = win.info.size != size;
                win.info.size = size;
                let info = win.info.clone();
                self.emit(Event::Resized {
                    window,
                    answers: Some(id),
                });
                if changed {
                    self.emit(Event::WindowChanged(info));
                    self.echoes.push(Echo::Resized { window, size });
                    self.redraw(window, None);
                } else if !self.options.changes_only {
                    self.redraw(window, None);
                }
            }
            Command::Close { window } => {
                if self.windows.remove(&window).is_some() {
                    self.echoes.push(Echo::Closed { window });
                    self.emit(Event::WindowRemoved(window));
                }
            }
            Command::RequestFrame { window, request } => {
                let Some(win) = self.windows.get(&window) else {
                    return;
                };
                match (&win.last, self.options.changes_only) {
                    // Nothing changed, so a capture-on-change backend resends its last picture.
                    (Some(last), true) => {
                        let frame = last.clone();
                        self.emit(Event::Frame(CapturedFrame {
                            window,
                            frame,
                            damage: Vec::new(),
                            answers: Some(request),
                        }));
                    }
                    _ => self.redraw(window, Some(request)),
                }
            }
            Command::PointerLeft { .. }
            | Command::Wheel(_)
            | Command::Focus { .. }
            | Command::Clipboard { .. }
            | Command::UnitIdentity { .. }
            | Command::UnitExited { .. }
            | Command::Frozen
            | Command::Thawed => {}
        }
    }
}

/// The picture of `win`: the grid, its counter in the bottom row (one pixel
/// per bit, white for 1), and the marker of the last press.
fn draw(win: &PatternWindow, epoch: Instant) -> Option<Frame> {
    let size = win.info.size;
    let mut pixels = draw_grid(size);
    let stride = size.width as usize * 4;
    let mut set = |x: i64, y: i64, bgra: [u8; 4]| {
        if (0..i64::from(size.width)).contains(&x) && (0..i64::from(size.height)).contains(&y) {
            let at = y as usize * stride + x as usize * 4;
            pixels[at..at + 4].copy_from_slice(&bgra);
        }
    };
    let bottom = i64::from(size.height) - 1;
    for bit in 0..32 {
        let on = win.counter >> bit & 1 == 1;
        let value = if on { 0xff } else { 0 };
        set(bit, bottom, [value, value, value, 0xff]);
    }
    if let Some(marker) = win.marker {
        for dy in -1..=1 {
            for dx in -1..=1 {
                set(
                    i64::from(marker.x) + dx,
                    i64::from(marker.y) + dy,
                    [0, 0, 0, 0xff],
                );
            }
        }
    }
    // Sizes are clamped to the protocol's limits, so this does not fail.
    let frame = Frame::packed(size.width, size.height, pixels).ok()?;
    Some(frame.with_timestamp(epoch.elapsed()))
}

/// Locks the shared state; a panic while it was held is a test failure already.
fn lock(shared: &Mutex<Pattern>) -> MutexGuard<'_, Pattern> {
    shared.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The test-pattern backend, before it starts.
#[derive(Debug)]
pub struct TestPattern {
    shared: Arc<Mutex<Pattern>>,
    real_desktop: bool,
}

/// Drives a test pattern from outside the host and reads what it received.
#[derive(Debug, Clone)]
pub struct TestPatternHandle {
    shared: Arc<Mutex<Pattern>>,
}

impl TestPattern {
    /// A pattern with one top-level window, and a handle to it.
    pub fn new(options: TestPatternOptions) -> (Self, TestPatternHandle) {
        let real_desktop = options.real_desktop;
        let size = options.size;
        let mut pattern = Pattern {
            options,
            sink: None,
            windows: BTreeMap::new(),
            next_id: 1,
            buttons: Buttons::NONE,
            commands: Vec::new(),
            echoes: Vec::new(),
            hold: false,
            held: Vec::new(),
            epoch: Instant::now(),
        };
        pattern.add(size, WindowKind::Toplevel, None, Point::default());
        let shared = Arc::new(Mutex::new(pattern));
        let handle = TestPatternHandle {
            shared: shared.clone(),
        };
        (
            Self {
                shared,
                real_desktop,
            },
            handle,
        )
    }
}

impl Backend for TestPattern {
    /// What the options said.
    fn real_desktop(&self) -> bool {
        self.real_desktop
    }

    /// Announces the windows with a first frame each.
    fn start(self: Box<Self>, events: EventSink) -> Result<Box<dyn BackendControl>, BackendError> {
        let mut pattern = lock(&self.shared);
        pattern.sink = Some(events);
        let windows: Vec<_> = pattern.windows.values().map(|w| w.info.clone()).collect();
        for info in windows {
            let id = info.id;
            pattern.emit(Event::WindowAdded(info));
            pattern.redraw(id, None);
        }
        drop(pattern);
        Ok(Box::new(Control {
            shared: self.shared,
        }))
    }
}

/// The running pattern's control.
#[derive(Debug)]
struct Control {
    shared: Arc<Mutex<Pattern>>,
}

impl BackendControl for Control {
    /// Handles the command at once.
    fn send(&mut self, command: Command) {
        lock(&self.shared).handle(command);
    }
}

impl TestPatternHandle {
    /// The window the pattern starts with.
    pub fn first_window(&self) -> WindowId {
        WindowId(1)
    }

    /// Opens another top-level window of `size`.
    pub fn open_window(&self, size: Size) -> WindowId {
        lock(&self.shared).add(size, WindowKind::Toplevel, None, Point::default())
    }

    /// Opens a popup of `parent` at `offset`.
    pub fn open_popup(&self, parent: WindowId, offset: Point, size: Size) -> WindowId {
        lock(&self.shared).add(size, WindowKind::Popup, Some(parent), offset)
    }

    /// Closes a window as if the app closed it.
    pub fn close_window(&self, window: WindowId) {
        let mut pattern = lock(&self.shared);
        if pattern.windows.remove(&window).is_some() {
            pattern.emit(Event::WindowRemoved(window));
        }
    }

    /// Redraws `window` with its counter one higher, as an animation would.
    pub fn redraw(&self, window: WindowId) {
        lock(&self.shared).redraw(window, None);
    }

    /// Holds frames back from the host while `hold` is set, and sends the
    /// held ones, in order, when it is cleared; other events pass.
    pub fn hold_frames(&self, hold: bool) {
        let mut pattern = lock(&self.shared);
        pattern.hold = hold;
        if !hold {
            for event in std::mem::take(&mut pattern.held) {
                pattern.emit(event);
            }
        }
    }

    /// The size of `window` now, if it is open.
    pub fn window_size(&self, window: WindowId) -> Option<Size> {
        lock(&self.shared).windows.get(&window).map(|w| w.info.size)
    }

    /// Every command the host sent, in order.
    pub fn commands(&self) -> Vec<Command> {
        lock(&self.shared).commands.clone()
    }

    /// What the windows received, in order.
    pub fn echoes(&self) -> Vec<Echo> {
        lock(&self.shared).echoes.clone()
    }

    /// Stops reporting, as a backend whose display died: the host sees its
    /// backend gone.
    pub fn vanish(&self) {
        lock(&self.shared).sink = None;
    }
}
