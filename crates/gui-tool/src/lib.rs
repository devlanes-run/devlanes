//! What the `devlanes gui` commands do on a GUI host's port: connect as a
//! tool, pick a window, send input and wait for the host to apply it, take
//! screenshots and encode them as PNG.
//!
//! Every wait after input is the protocol's `Sync`. A host answers it once
//! it applied the input and captured a frame after it, or after 3 seconds at
//! the latest, so an answer says the input reached the app, not that the app
//! drew anything.

pub mod keys;

use std::net::SocketAddr;
use std::time::Duration;

use devlanes_gui_client::{Client, ClientError, ClientOptions, Event, Events};
use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    Buttons, LockState, Point, Size, Wheel, WheelUnit, WindowId, WindowInfo, WindowKind,
};

pub use devlanes_gui_proto::MAX_DIMENSION;
pub use keys::{Chord, KeyError, known_names, parse_chord};

/// How long one step may take: connecting, a sync, a screenshot, a resize.
/// A host answers a sync within 3 seconds and waits up to 3 seconds for a
/// frame after a resize, so a step past this means the host is stuck.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// The host closed the connection while a step waited.
    #[error("the GUI host closed the connection")]
    Closed,
    /// A picture could not be encoded as PNG.
    #[error("encoding the PNG failed: {0}")]
    Png(String),
}

/// A tool connection to one GUI host.
#[derive(Debug)]
pub struct Session {
    client: Client,
    events: Events,
    windows: Vec<WindowInfo>,
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

    /// The windows the host had open when the session started.
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
    /// `Failed`, so that answer is tried once more.
    pub async fn screenshot(&self, window: WindowId) -> Result<Frame, ToolError> {
        let shot = match step("the screenshot", self.client.screenshot(Some(window))).await? {
            Err(ClientError::ScreenshotFailed(_)) => {
                step("the screenshot", self.client.screenshot(Some(window))).await?
            }
            other => other,
        }?;
        Ok(shot.frame)
    }

    /// Clicks `button` at `at` in `window`, twice for `double`, and waits
    /// until the host applied it.
    pub async fn click(
        &self,
        window: WindowId,
        at: Point,
        button: Buttons,
        double: bool,
    ) -> Result<(), ToolError> {
        self.client.click(window, at, button)?;
        if double {
            self.client.click(window, at, button)?;
        }
        self.sync().await
    }

    /// Types `text` into `window`. It waits for a frame first: toolkits drop
    /// keys that arrive in the same instant as the click that focused a field,
    /// and that click may have come from the command just before.
    pub async fn type_text(&self, window: WindowId, text: &str) -> Result<(), ToolError> {
        self.sync().await?;
        self.client.text(window, text)?;
        self.sync().await
    }

    /// Presses each chord in turn: its keys down in order, then up in
    /// reverse. Waits for a frame first, as [`Session::type_text`] does.
    pub async fn keys(&self, window: WindowId, chords: &[Chord]) -> Result<(), ToolError> {
        self.sync().await?;
        for chord in chords {
            for key in &chord.keys {
                self.client.key(window, *key, true)?;
            }
            for key in chord.keys.iter().rev() {
                self.client.key(window, *key, false)?;
            }
        }
        self.sync().await
    }

    /// Scrolls `window` by `dx`, `dy` lines with the pointer at `at`.
    pub async fn scroll(
        &self,
        window: WindowId,
        at: Point,
        dx: f32,
        dy: f32,
    ) -> Result<(), ToolError> {
        self.client.wheel(Wheel {
            window,
            position: at,
            delta_x: dx,
            delta_y: dy,
            unit: WheelUnit::Lines,
        })?;
        self.sync().await
    }

    /// Asks for `window` at `size`, waits until the host applied it and a
    /// frame came, and returns the size the window took, which an app that
    /// clamps its size may make another one; `None` when the host did not
    /// say.
    pub async fn resize(&mut self, window: WindowId, size: Size) -> Result<Option<Size>, ToolError> {
        let request = self.client.resize(window, size)?;
        let mut taken = None;
        let events = &mut self.events;
        step("the resize", async {
            while let Some(event) = events.next().await {
                match event {
                    Event::WindowChanged(info) if info.id == window => taken = Some(info.size),
                    Event::ResizeApplied {
                        window: id,
                        request: answered,
                        ..
                    } if id == window && answered == request => return Ok(()),
                    Event::Closed(Some(error)) => return Err(ToolError::Client(error)),
                    Event::Closed(None) => return Err(ToolError::Closed),
                    _ => {}
                }
            }
            Err(ToolError::Closed)
        })
        .await??;
        self.sync().await?;
        // A backend may describe the new size after it answered the resize.
        // Both reached the event queue before the sync's answer, so what is
        // queued now is read without waiting (a zero timeout polls once).
        while let Ok(Some(event)) = tokio::time::timeout(Duration::ZERO, self.events.next()).await
        {
            if let Event::WindowChanged(info) = event
                && info.id == window
            {
                taken = Some(info.size);
            }
        }
        Ok(taken)
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

/// `frame` as a PNG file's bytes, RGBA without its alpha changes. CPU work of
/// a few milliseconds per megapixel; callers on an async runtime run it on
/// the blocking pool.
pub fn encode_png(frame: &Frame) -> Result<Vec<u8>, ToolError> {
    let (width, height) = (frame.width(), frame.height());
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let row = frame
            .row(y)
            .ok_or_else(|| ToolError::Png(format!("row {y} is missing")))?;
        for pixel in row.chunks_exact(4).take(width as usize) {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| ToolError::Png(error.to_string()))?;
    writer
        .write_image_data(&rgba)
        .map_err(|error| ToolError::Png(error.to_string()))?;
    writer
        .finish()
        .map_err(|error| ToolError::Png(error.to_string()))?;
    Ok(out)
}

/// The middle of `window`, where a scroll without `--at` points.
pub fn centre(window: &WindowInfo) -> Point {
    Point::new(
        i32::try_from(window.size.width / 2).unwrap_or(0),
        i32::try_from(window.size.height / 2).unwrap_or(0),
    )
}

/// The button `name` names: `left`, `right` or `middle`.
pub fn button(name: &str) -> Option<Buttons> {
    match name {
        "left" => Some(Buttons::LEFT),
        "right" => Some(Buttons::RIGHT),
        "middle" => Some(Buttons::MIDDLE),
        _ => None,
    }
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
