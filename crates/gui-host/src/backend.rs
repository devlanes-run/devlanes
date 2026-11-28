//! The interface between the host core and one OS backend.
//!
//! A backend owns the display: it finds windows, captures their pictures and
//! delivers input to them. It runs on threads of its own (a compositor's
//! event loop, capture callbacks, an input thread), reports through an
//! [`EventSink`] and takes [`Command`]s through its [`BackendControl`]. The
//! host never waits on it: [`Backend::start`] runs on the blocking pool, and
//! [`BackendControl::send`] must return at once.

use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    Cursor, Key, MoveResize, Point, Pointer, Size, Wheel, WindowId, WindowInfo,
};
use tokio::sync::mpsc;

/// An OS backend before it starts.
pub trait Backend: Send + 'static {
    /// Starts the backend, which then reports through `events` from any of
    /// its threads until the returned control is dropped. Called once, on a
    /// thread of the blocking pool, so it may block while the display comes up.
    fn start(self: Box<Self>, events: EventSink) -> Result<Started, BackendError>;
}

/// What a backend hands back once its display is up.
pub struct Started {
    /// How the host reaches it.
    pub control: Box<dyn BackendControl>,
    /// Variables the unit's process needs to reach the display, such as
    /// `WAYLAND_DISPLAY`; `None` removes an inherited one.
    pub env: Vec<(String, Option<String>)>,
    /// Whether the display is the machine's real desktop (a Windows host),
    /// for the welcome.
    pub real_desktop: bool,
}

impl std::fmt::Debug for Started {
    /// The variables and the flag; the control has nothing to show.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Started")
            .field("env", &self.env)
            .field("real_desktop", &self.real_desktop)
            .finish_non_exhaustive()
    }
}

/// How the host reaches a running backend. Dropping it stops the backend.
pub trait BackendControl: Send {
    /// Hands `command` to the backend. Called on an async executor thread,
    /// so it must not block: queue the command for the backend's own thread
    /// (a calloop channel, a message to a capture thread) and return.
    ///
    /// Commands arrive in the order the host decided them, and the backend
    /// applies them in that order. A backend whose thread falls behind
    /// should merge queued pointer motion (keeping the last position of each
    /// run of motion with the same buttons) rather than replay all of it.
    fn send(&mut self, command: Command);
}

/// What keeps a backend from running, so callers can tell the cases apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unavailable {
    /// No logged-in desktop session to show windows in.
    NoDesktopSession,
    /// The process runs elevated, so it cannot drive the user's windows.
    Elevated,
    /// A system library or program is missing.
    MissingLibrary,
    /// This OS has no backend.
    UnsupportedOs,
    /// Anything else; the message says what.
    Other,
}

/// Why a backend could not start.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Something the backend needs is missing, such as a desktop session or
    /// a system library.
    #[error("the GUI backend cannot run here: {message}")]
    Unavailable {
        /// What is missing.
        reason: Unavailable,
        /// The same for people.
        message: String,
    },
    /// Starting failed.
    #[error("the GUI backend failed to start")]
    Failed(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// A rectangle in window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    /// The top-left corner.
    pub origin: Point,
    /// The size.
    pub size: Size,
}

/// A picture of one window.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// The window, as the backend named it.
    pub window: WindowId,
    /// The picture, cropped to the window geometry, with its capture time on
    /// the host's monotonic clock.
    pub frame: Frame,
    /// The regions that changed since the window's previous frame; empty
    /// when the backend does not know, which means all of it.
    pub damage: Vec<Rect>,
    /// The [`Command::RequestFrame`] this frame answers, if any.
    pub answers: Option<u64>,
}

/// What a backend reports, in the order it happened.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// A window appeared. The backend picks its id and never reuses one;
    /// a popup's `parent` names the backend's id of its parent. The host
    /// sets the lock state itself, so `lock` is ignored.
    WindowAdded(WindowInfo),
    /// A window's description changed: the whole new description.
    WindowChanged(WindowInfo),
    /// A window closed.
    WindowRemoved(WindowId),
    /// A picture of a window. Frames of a window arrive in capture order.
    Frame(CapturedFrame),
    /// The cursor the app shows changed.
    Cursor(Cursor),
    /// The app started an interactive move or resize.
    MoveResizeStarted(MoveResize),
    /// The app's clipboard text changed.
    Clipboard(String),
    /// The backend applied a size change to a window: a
    /// [`Command::Resize`] (`answers` holds its id) or one the app made
    /// (`None`). Sent before any frame captured after the change, and for
    /// every `Resize` exactly once, also when the app keeps its size, so
    /// the viewer that asked can stop waiting.
    Resized {
        /// The window.
        window: WindowId,
        /// The [`Command::Resize`] it answers.
        answers: Option<u64>,
    },
}

/// What the host asks a backend to do. Windows are named by the backend's ids.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// A window got its first viewer (`true`) or lost its last (`false`).
    /// Windows start unwatched. A backend may stop capturing a window nobody
    /// watches; it still answers [`Command::RequestFrame`] for it.
    Watch {
        /// The window.
        window: WindowId,
        /// Whether any viewer watches it now.
        watched: bool,
    },
    /// Move the shared pointer and set the buttons it holds; a change from
    /// the previous command's buttons is a press or release. Buttons belong
    /// to the display, not a window: a release may name another window than
    /// the press did, such as when that window locked or closed meanwhile.
    Pointer(Pointer),
    /// The pointer left a window with no button held.
    PointerLeft {
        /// The window.
        window: WindowId,
    },
    /// Scroll.
    Wheel(Wheel),
    /// A physical key went down or up.
    Key(Key),
    /// Committed text for the focused window.
    Text {
        /// The window.
        window: WindowId,
        /// The text.
        text: String,
    },
    /// Resize a window; answer with [`Event::Resized`] carrying `id`.
    Resize {
        /// The window.
        window: WindowId,
        /// The size a viewer wants; the app may pick another.
        size: Size,
        /// Echoed in the answer.
        id: u64,
    },
    /// Ask the app to close a window.
    Close {
        /// The window.
        window: WindowId,
    },
    /// A viewer's window of it gained or lost focus.
    Focus {
        /// The window.
        window: WindowId,
        /// Whether it has focus now.
        focused: bool,
    },
    /// Set the clipboard text the app sees.
    Clipboard {
        /// The text.
        text: String,
    },
    /// Send one frame of `window` with `answers` set to `request`, captured
    /// after every command sent before this one took effect. A backend that
    /// captures only on change sends its newest capture again once those
    /// commands were applied. Ignored for a window that is gone.
    RequestFrame {
        /// The window.
        window: WindowId,
        /// Echoed in the frame's `answers`.
        request: u64,
    },
    /// The unit's process, from the daemon, so the backend can find its
    /// windows. The fields are those of the control message.
    UnitIdentity {
        /// The process id.
        pid: u32,
        /// The process start time, compared for equality only.
        start_time: u64,
        /// The unit's job object on Windows.
        job: Option<String>,
    },
    /// The unit's process exited.
    UnitExited {
        /// The exit code, when there is one.
        code: Option<i32>,
    },
    /// The lane is frozen: its processes are stopped. A backend stops
    /// sending what would fill a stopped client's buffers.
    Frozen,
    /// The lane is thawed.
    Thawed,
}

/// Where a backend sends its events; cheap to clone and usable from any
/// thread. Sending never blocks.
#[derive(Debug, Clone)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<Event>,
}

/// The host stopped and no longer takes events; the backend can stop too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the GUI host stopped")]
pub struct HostGone;

impl EventSink {
    /// A sink and the receiver the host reads. Unbounded, because frames
    /// must not be lost (a static window sends its last one only once) and
    /// the host drains it without doing blocking work in between.
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Reports `event`.
    pub fn send(&self, event: Event) -> Result<(), HostGone> {
        self.tx.send(event).map_err(|_| HostGone)
    }
}
