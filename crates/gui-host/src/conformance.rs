//! Checks every backend runs against itself: a window is found, a frame
//! contains the grid of [`crate::pattern`], a click arrives at the right
//! spot, typed text arrives, a resize applies.
//!
//! [`run`] drives a served host as a tool through the headless client. What
//! the app received comes from a [`Probe`]: the test pattern's own list here,
//! the lines the fixture app prints for real backends. The app must draw the
//! grid over its whole window and accept a resize to a smaller size.

use std::future::Future;
use std::time::Duration;

use devlanes_gui_client::{Client, ClientError, Event, Events};
use devlanes_gui_proto::{Buttons, Point, Size, WindowId, WindowInfo, WindowKind};

use crate::pattern::{cell_centre, shows_grid};
use crate::{Echo, TestPatternHandle};

/// How long each step may take.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// The text the typing check sends.
pub const TYPED: &str = "acme 42";

/// What the app reports it received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// A button went down at a position in the window.
    Pressed(Point),
    /// Text was typed.
    Text(String),
    /// The window took a new size.
    Resized(Size),
}

/// What the app under test received.
pub trait Probe: Send {
    /// What the app reported since the last call, waiting a short while for
    /// the first report when there is none yet. Empty when nothing came.
    fn events(&mut self) -> impl Future<Output = Vec<AppEvent>> + Send;
}

/// Why a check failed.
#[derive(Debug, thiserror::Error)]
pub enum ConformanceError {
    /// The client failed.
    #[error("the GUI client failed")]
    Client(#[from] ClientError),
    /// A step took longer than [`STEP_TIMEOUT`].
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    /// No top-level window with a size appeared.
    #[error("no window appeared")]
    NoWindow,
    /// The screenshot does not show the grid.
    #[error("the {0}x{1} screenshot does not show the test grid")]
    NoGrid(u32, u32),
    /// The press arrived somewhere else, or not at all.
    #[error("clicked at {expected:?}, the app got {got:?}")]
    ClickMissed {
        /// Where the click was sent.
        expected: Point,
        /// The presses the app reported.
        got: Vec<Point>,
    },
    /// The typed text did not arrive whole.
    #[error("typed {expected:?}, the app got {got:?}")]
    TextMissed {
        /// What was typed.
        expected: &'static str,
        /// What the app reported.
        got: String,
    },
    /// The window did not take the requested size.
    #[error("asked for {wanted:?}, the window is {got:?}")]
    ResizeNotApplied {
        /// The size requested.
        wanted: Size,
        /// The size of the screenshot after it.
        got: Size,
    },
}

/// Runs every check in order against the host `client` is connected to as a
/// tool, stopping at the first failure.
pub async fn run(
    client: &Client,
    events: &mut Events,
    probe: &mut impl Probe,
) -> Result<(), ConformanceError> {
    let window = find_window(client, events).await?;
    let shot = step("a screenshot", client.screenshot(Some(window.id))).await??;
    let size = Size::new(shot.frame.width(), shot.frame.height());
    if !shows_grid(&shot.frame) {
        return Err(ConformanceError::NoGrid(size.width, size.height));
    }

    let target = cell_centre(1, 1, size);
    client.click(window.id, target, Buttons::LEFT)?;
    step("the click to apply", client.sync()).await??;
    let mut presses = Vec::new();
    collect(probe, |event| match event {
        AppEvent::Pressed(at) => {
            presses.push(at);
            presses.iter().any(|at| near(*at, target))
        }
        _ => false,
    })
    .await;
    if !presses.iter().any(|at| near(*at, target)) {
        return Err(ConformanceError::ClickMissed {
            expected: target,
            got: presses,
        });
    }

    client.text(window.id, TYPED)?;
    step("the text to apply", client.sync()).await??;
    let mut typed = String::new();
    collect(probe, |event| match event {
        AppEvent::Text(text) => {
            typed.push_str(&text);
            typed.contains(TYPED)
        }
        _ => false,
    })
    .await;
    if !typed.contains(TYPED) {
        return Err(ConformanceError::TextMissed {
            expected: TYPED,
            got: typed,
        });
    }

    let wanted = Size::new(
        size.width.saturating_sub(40).max(100),
        size.height.saturating_sub(30).max(100),
    );
    let request = client.resize(window.id, wanted)?;
    step("the resize to apply", async {
        while let Some(event) = events.next().await {
            if let Event::ResizeApplied {
                window: id,
                request: answered,
                ..
            } = event
                && id == window.id
                && answered == request
            {
                return Ok(());
            }
        }
        Err(ConformanceError::Client(ClientError::Closed))
    })
    .await??;
    step("a frame after the resize", client.sync()).await??;
    let after = step("a screenshot", client.screenshot(Some(window.id))).await??;
    let got = Size::new(after.frame.width(), after.frame.height());
    if got != wanted {
        return Err(ConformanceError::ResizeNotApplied { wanted, got });
    }
    Ok(())
}

/// Runs `future` for at most [`STEP_TIMEOUT`].
async fn step<T>(what: &'static str, future: impl Future<Output = T>) -> Result<T, ConformanceError> {
    tokio::time::timeout(STEP_TIMEOUT, future)
        .await
        .map_err(|_| ConformanceError::Timeout(what))
}

/// The first top-level window with a size, from the welcome or as it appears.
async fn find_window(client: &Client, events: &mut Events) -> Result<WindowInfo, ConformanceError> {
    let usable = |w: &WindowInfo| {
        w.kind == WindowKind::Toplevel && w.parent.is_none() && w.size.is_nonzero()
    };
    if let Some(window) = client.welcome().windows.iter().find(|w| usable(w)) {
        return Ok(window.clone());
    }
    step("a window", async {
        while let Some(event) = events.next().await {
            if let Event::WindowAdded(window) | Event::WindowChanged(window) = event
                && usable(&window)
            {
                return Ok(window);
            }
        }
        Err(ConformanceError::NoWindow)
    })
    .await?
}

/// Feeds reports to `done` until it says so or the probe stays quiet for a
/// whole [`STEP_TIMEOUT`].
async fn collect(probe: &mut impl Probe, mut done: impl FnMut(AppEvent) -> bool) {
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        for event in probe.events().await {
            if done(event) {
                return;
            }
        }
    }
}

/// Whether two points are at most a pixel apart on each axis, for backends
/// that round through another coordinate system.
fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= 1 && (a.y - b.y).abs() <= 1
}

/// A [`Probe`] over a test pattern's echoes of one window.
#[derive(Debug)]
pub struct TestPatternProbe {
    handle: TestPatternHandle,
    window: WindowId,
    seen: usize,
}

impl TestPatternProbe {
    /// A probe of `window` that reports echoes from now on.
    pub fn new(handle: TestPatternHandle, window: WindowId) -> Self {
        let seen = handle.echoes().len();
        Self {
            handle,
            window,
            seen,
        }
    }
}

impl Probe for TestPatternProbe {
    /// The pattern's echoes since the last call. The pattern records input
    /// as the host hands it over, so after a sync nothing is left to wait for;
    /// an empty answer still yields a tick so a caller's loop moves on.
    async fn events(&mut self) -> Vec<AppEvent> {
        let echoes = self.handle.echoes();
        let new = echoes.get(self.seen..).unwrap_or_default().to_vec();
        self.seen = echoes.len();
        if new.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        new.into_iter()
            .filter_map(|echo| match echo {
                Echo::Pressed {
                    window, position, ..
                } if window == self.window => Some(AppEvent::Pressed(position)),
                Echo::Text { window, text } if window == self.window => Some(AppEvent::Text(text)),
                Echo::Resized { window, size } if window == self.window => {
                    Some(AppEvent::Resized(size))
                }
                _ => None,
            })
            .collect()
    }
}
