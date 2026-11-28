//! The tool side of the GUI protocol for the `devlanes gui` commands: connect
//! to a GUI host as a tool, pick a window, send input and wait for the host
//! to apply it, take screenshots and encode them as PNG.
//!
//! Every wait after input is the protocol's `Sync`. A host answers it once
//! it passed the input to the app and captured a frame after it, or after 3
//! seconds at the latest, so an answer says the input reached the app, not
//! that the app drew its result.

pub mod keys;

use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use devlanes_gui_client::{Client, ClientError, ClientOptions, Event, Events};
use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    Buttons, LockState, MAX_DIMENSION, MAX_TEXT, Point, Size, Wheel, WheelUnit, WindowId,
    WindowInfo, WindowKind,
};

pub use keys::{Chord, KeyError, known_names, parse_chord};

/// How long one step may take: connecting, a sync, a screenshot, a resize.
/// A host answers a sync within 3 seconds and waits up to 3 seconds for a
/// frame after a resize, so a step past this means the host is stuck.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait before the one retry of a screenshot the host answered
/// with `Failed`, which it does while it encodes too many at once.
const RETRY_AFTER: Duration = Duration::from_millis(150);

/// Why a command could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The connection or a request failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// A step did not finish within [`STEP_TIMEOUT`].
    #[error("the GUI host did not answer {0} in time")]
    Timeout(&'static str),
    /// `--window` names no window of the host.
    #[error("no window {wanted}")]
    UnknownWindow {
        /// What `--window` said.
        wanted: String,
        /// The windows there are.
        windows: Vec<WindowInfo>,
    },
    /// `--window` names a title several windows have.
    #[error("several windows are titled {wanted}")]
    AmbiguousWindow {
        /// What `--window` said.
        wanted: String,
        /// The windows with that title.
        windows: Vec<WindowInfo>,
    },
    /// No `--window`, and the host has no main window open.
    #[error("no main window is open")]
    NoMainWindow {
        /// The windows there are.
        windows: Vec<WindowInfo>,
    },
    /// A click or scroll position outside the window.
    #[error("{point:?} is outside the {size:?} window")]
    OutsideWindow {
        /// The position asked for.
        point: Point,
        /// The window's size.
        size: Size,
    },
    /// A resize to a side above [`MAX_DIMENSION`].
    #[error("each side may be at most {max} pixels")]
    TooLarge {
        /// The largest side.
        max: u32,
    },
    /// The host closed the connection while a step waited.
    #[error("the GUI host closed the connection")]
    Closed,
    /// A picture could not be encoded or written as PNG.
    #[error("writing the PNG failed: {0}")]
    Png(String),
}

/// What became of the window by the time the host applied the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// It is open and takes input.
    Open,
    /// It closed, such as after a click on its close button.
    Closed,
    /// It is locked now, so later input is dropped until it unlocks.
    Locked(LockState),
    /// It is open, but smaller than the point the input went to, so the host
    /// may have dropped it as outside the window.
    Outside(Size),
}

/// A tool connection to one GUI host.
#[derive(Debug)]
pub struct Session {
    client: Client,
    events: Events,
    windows: Vec<WindowInfo>,
    /// Windows this session asked for a size.
    resized: Vec<WindowId>,
}

/// Connects to the host at `address` as a tool named `name`.
pub async fn connect(address: SocketAddr, name: &str) -> Result<Session, ToolError> {
    let id = format!("devlanes-gui-tool:{}", std::process::id());
    let options = ClientOptions::new(devlanes_gui_proto::Role::Tool, id, name);
    let (client, events) = step(
        "the connection",
        devlanes_gui_client::connect(address, options),
    )
    .await??;
    let windows = client.welcome().windows.clone();
    Ok(Session {
        client,
        events,
        windows,
        resized: Vec::new(),
    })
}

/// Runs `future` for at most [`STEP_TIMEOUT`].
async fn step<T>(what: &'static str, future: impl Future<Output = T>) -> Result<T, ToolError> {
    tokio::time::timeout(STEP_TIMEOUT, future)
        .await
        .map_err(|_| ToolError::Timeout(what))
}

impl Session {
    /// Whether the host's display is the machine's real desktop.
    pub fn real_desktop(&self) -> bool {
        self.client.welcome().real_desktop
    }

    /// The windows the host has open, as of the last step.
    pub fn windows(&self) -> &[WindowInfo] {
        &self.windows
    }

    /// The window `wanted` names, by id or by exact title; without one, the
    /// first main window (a top-level window without a parent that is not
    /// closed).
    pub fn window(&self, wanted: Option<&str>) -> Result<WindowInfo, ToolError> {
        pick(&self.windows, wanted)
    }

    /// A picture of `window`. A host busy with other screenshots answers
    /// `Failed`, so that answer is tried once more after a moment.
    pub async fn screenshot(&self, window: WindowId) -> Result<Frame, ToolError> {
        let shot = match step("the screenshot", self.client.screenshot(Some(window))).await? {
            Err(ClientError::ScreenshotFailed(_)) => {
                tokio::time::sleep(RETRY_AFTER).await;
                step("the screenshot", self.client.screenshot(Some(window))).await?
            }
            other => other,
        }?;
        Ok(shot.frame)
    }

    /// Clicks `button` at `at` in `window`, twice for `double`, and waits
    /// until the host applied it. A point outside the window is refused.
    pub async fn click(
        &mut self,
        window: WindowId,
        at: Point,
        button: Buttons,
        double: bool,
    ) -> Result<After, ToolError> {
        self.inside(window, at)?;
        self.client.click(window, at, button)?;
        if double {
            self.client.click(window, at, button)?;
        }
        let after = self.finish(window).await?;
        Ok(self.still_inside(window, at, after))
    }

    /// Types `text` into `window`, in pieces of at most the protocol's
    /// [`MAX_TEXT`] bytes. It waits for a frame first: toolkits drop keys that
    /// arrive in the same instant as the click that focused a field, and that
    /// click may have come from the command just before.
    pub async fn type_text(&mut self, window: WindowId, text: &str) -> Result<After, ToolError> {
        self.sync().await?;
        for piece in pieces(text, MAX_TEXT) {
            self.client.text(window, piece)?;
        }
        self.finish(window).await
    }

    /// Presses each chord in turn: its keys down in order, then up in
    /// reverse. Waits for a frame first, as [`Session::type_text`] does.
    pub async fn keys(&mut self, window: WindowId, chords: &[Chord]) -> Result<After, ToolError> {
        self.sync().await?;
        for chord in chords {
            for key in &chord.keys {
                self.client.key(window, *key, true)?;
            }
            for key in chord.keys.iter().rev() {
                self.client.key(window, *key, false)?;
            }
        }
        self.finish(window).await
    }

    /// Scrolls `window` by `dx`, `dy` lines with the pointer at `at`, which
    /// must be inside the window.
    pub async fn scroll(
        &mut self,
        window: WindowId,
        at: Point,
        dx: f32,
        dy: f32,
    ) -> Result<After, ToolError> {
        self.inside(window, at)?;
        self.client.wheel(Wheel {
            window,
            position: at,
            delta_x: dx,
            delta_y: dy,
            unit: WheelUnit::Lines,
        })?;
        let after = self.finish(window).await?;
        Ok(self.still_inside(window, at, after))
    }

    /// `after`, or [`After::Outside`] when `window` is open but no longer
    /// covers `at`: it shrank between the session's start and the input.
    fn still_inside(&self, window: WindowId, at: Point, after: After) -> After {
        match (after, self.inside(window, at)) {
            (After::Open, Err(ToolError::OutsideWindow { size, .. })) => After::Outside(size),
            (after, _) => after,
        }
    }

    /// Asks for `window` at `size`, waits until the host applied it and a
    /// frame came, and returns the size the window has then (which an app
    /// that clamps its size may make another one) and what became of it. A
    /// window that closed meanwhile has no size.
    pub async fn resize(
        &mut self,
        window: WindowId,
        size: Size,
    ) -> Result<(Option<Size>, After), ToolError> {
        if !size.is_valid() {
            return Err(ToolError::TooLarge { max: MAX_DIMENSION });
        }
        let request = self.client.resize(window, size)?;
        self.resized.push(window);
        let events = &mut self.events;
        let windows = &mut self.windows;
        step("the resize", async {
            while let Some(event) = events.next().await {
                match event {
                    Event::ResizeApplied {
                        window: id,
                        request: answered,
                        ..
                    } if id == window && answered == request => return Ok(()),
                    Event::Closed(Some(error)) => return Err(ToolError::Client(error)),
                    Event::Closed(None) => return Err(ToolError::Closed),
                    other => track(windows, other),
                }
            }
            Err(ToolError::Closed)
        })
        .await??;
        let after = self.finish(window).await?;
        let taken = match after {
            After::Closed => None,
            _ => self
                .windows
                .iter()
                .find(|info| info.id == window)
                .map(|info| info.size),
        };
        Ok((taken, after))
    }

    /// Refuses `at` when it is outside `window`'s size.
    fn inside(&self, window: WindowId, at: Point) -> Result<(), ToolError> {
        let size = self
            .windows
            .iter()
            .find(|info| info.id == window)
            .map(|info| info.size)
            .unwrap_or_default();
        let fits = |value: i32, side: u32| u32::try_from(value).is_ok_and(|value| value < side);
        if fits(at.x, size.width) && fits(at.y, size.height) {
            Ok(())
        } else {
            Err(ToolError::OutsideWindow { point: at, size })
        }
    }

    /// Waits until the host applied what this session sent, and says what
    /// became of `window` meanwhile.
    async fn finish(&mut self, window: WindowId) -> Result<After, ToolError> {
        self.sync().await?;
        // What the host said before it answered the sync is queued already,
        // so it is read without waiting (a zero timeout polls once).
        while let Ok(Some(event)) = tokio::time::timeout(Duration::ZERO, self.events.next()).await {
            track(&mut self.windows, event);
        }
        if !self.windows.iter().any(|info| info.id == window) {
            return Ok(After::Closed);
        }
        Ok(match self.client.lock_state(window) {
            None => After::Closed,
            Some(LockState::Unlocked) => After::Open,
            // The client shows a window this session resized as Resizing until
            // it sees a frame, which a tool never streams. The sync waited for
            // the host's frame after the resize, so the host has ended it.
            Some(LockState::Resizing) if self.resized.contains(&window) => After::Open,
            Some(LockState::Closed) => After::Closed,
            Some(lock) => After::Locked(lock),
        })
    }

    /// Waits until the host applied what this session sent.
    async fn sync(&self) -> Result<(), ToolError> {
        step("the sync", self.client.sync()).await??;
        Ok(())
    }

    /// Closes the connection.
    pub fn close(self) {
        self.client.close();
    }
}

/// Keeps `windows` up to date with a window event.
fn track(windows: &mut Vec<WindowInfo>, event: Event) {
    match event {
        Event::WindowAdded(info) | Event::WindowChanged(info) => {
            match windows.iter_mut().find(|known| known.id == info.id) {
                Some(known) => *known = info,
                None => windows.push(info),
            }
        }
        Event::WindowRemoved(id) => windows.retain(|known| known.id != id),
        _ => {}
    }
}

/// `text` in pieces of at most `limit` bytes, split between characters.
fn pieces(text: &str, limit: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(limit);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // A limit below one character's width still moves on by a character.
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (piece, tail) = rest.split_at(end);
        pieces.push(piece);
        rest = tail;
    }
    pieces
}

/// [`Session::window`] over `windows`.
fn pick(windows: &[WindowInfo], wanted: Option<&str>) -> Result<WindowInfo, ToolError> {
    let Some(wanted) = wanted else {
        return windows
            .iter()
            .find(|window| {
                window.kind == WindowKind::Toplevel
                    && window.parent.is_none()
                    && window.lock != LockState::Closed
            })
            .cloned()
            .ok_or_else(|| ToolError::NoMainWindow {
                windows: windows.to_vec(),
            });
    };
    if let Ok(id) = wanted.parse::<u32>()
        && let Some(window) = windows.iter().find(|window| window.id == WindowId(id))
    {
        return Ok(window.clone());
    }
    let titled: Vec<WindowInfo> = windows
        .iter()
        .filter(|window| window.title == wanted)
        .cloned()
        .collect();
    match titled.as_slice() {
        [window] => Ok(window.clone()),
        [] => Err(ToolError::UnknownWindow {
            wanted: wanted.to_owned(),
            windows: windows.to_vec(),
        }),
        _ => Err(ToolError::AmbiguousWindow {
            wanted: wanted.to_owned(),
            windows: titled,
        }),
    }
}

/// Writes `frame` to `out` as a PNG, RGBA with the frame's alpha, one row at
/// a time, so no second copy of the picture is held. CPU work of a few
/// milliseconds per megapixel; callers on an async runtime run it on the
/// blocking pool.
pub fn write_png(frame: &Frame, out: impl Write) -> Result<(), ToolError> {
    let png = |error: &dyn std::fmt::Display| ToolError::Png(error.to_string());
    let (width, height) = (frame.width(), frame.height());
    let mut encoder = png::Encoder::new(out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| png(&error))?;
    let mut stream = writer.stream_writer().map_err(|error| png(&error))?;
    let mut rgba = Vec::with_capacity(width as usize * 4);
    for y in 0..height {
        let row = frame
            .row(y)
            .ok_or_else(|| ToolError::Png(format!("row {y} is missing")))?;
        rgba.clear();
        for [b, g, r, a] in row.as_chunks::<4>().0.iter().take(width as usize) {
            rgba.extend_from_slice(&[*r, *g, *b, *a]);
        }
        stream.write_all(&rgba).map_err(|error| png(&error))?;
    }
    stream.finish().map_err(|error| png(&error))
}

/// The middle of `window`, where a scroll without `--at` points.
pub fn centre(window: &WindowInfo) -> Point {
    Point::new(
        i32::try_from(window.size.width / 2).unwrap_or(0),
        i32::try_from(window.size.height / 2).unwrap_or(0),
    )
}

// Compiles the README's example with the doc tests, so it keeps matching the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A window with `id`, `title`, `kind` and `lock`.
    fn window(id: u32, title: &str, kind: WindowKind, lock: LockState) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            title: title.into(),
            app_id: String::new(),
            size: Size::new(10, 10),
            min_size: None,
            max_size: None,
            draws_own_frame: false,
            reports_moves: false,
            parent: None,
            offset: Point::default(),
            lock,
            kind,
        }
    }

    /// Without `--window` the first open main window; with it an id or a
    /// unique exact title; anything else an error that lists the windows.
    #[test]
    fn windows_are_picked_by_id_or_title() {
        let windows = [
            window(3, "Gone", WindowKind::Toplevel, LockState::Closed),
            window(4, "Menu", WindowKind::Popup, LockState::Unlocked),
            window(5, "Editor", WindowKind::Toplevel, LockState::Unlocked),
            window(6, "Twin", WindowKind::Toplevel, LockState::Unlocked),
            window(7, "Twin", WindowKind::Dialog, LockState::Unlocked),
            window(8, "6", WindowKind::Toplevel, LockState::Unlocked),
        ];
        let id = |wanted| pick(&windows, wanted).map(|window| window.id.0);
        assert_eq!(id(None).ok(), Some(5));
        assert_eq!(id(Some("4")).ok(), Some(4));
        assert_eq!(id(Some("Editor")).ok(), Some(5));
        // An id wins over a title that looks like one.
        assert_eq!(id(Some("6")).ok(), Some(6));
        assert!(matches!(
            pick(&windows, Some("Twin")),
            Err(ToolError::AmbiguousWindow { windows, .. }) if windows.len() == 2
        ));
        assert!(matches!(
            pick(&windows, Some("9")),
            Err(ToolError::UnknownWindow { windows, .. }) if windows.len() == 6
        ));
        assert!(matches!(
            pick(&windows[..2], None),
            Err(ToolError::NoMainWindow { windows }) if windows.len() == 2
        ));
    }
}
