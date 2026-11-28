//! The connection to the GUI host, on a tokio runtime of its own off the
//! window thread: it connects, reconnects with a short backoff after the
//! connection ends or goes silent, forwards what the host sends, and sends
//! what the [`crate::model::Viewer`] asks.
//!
//! Frames do not queue towards the window thread: each window has one slot
//! that holds its newest decoded frame, and the window thread is woken only
//! when a slot fills, so a window thread that falls behind skips frames.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use devlanes_gui_client::{Client, ClientError, ClientOptions, DecodedFrame, Event, connect};
use devlanes_gui_proto::{RECONNECT_AFTER, WindowId, WindowInfo};
use tokio::sync::mpsc;

use crate::model::{Check, Out};

/// What the connection tells the window thread.
#[derive(Debug)]
pub enum FromLink {
    /// A connection is up with these windows.
    Connected(Vec<WindowInfo>),
    /// Something the host sent, other than a frame.
    Event(Event),
    /// A frame of this window waits in its slot ([`Frames::take`]).
    Frame(WindowId),
    /// The connection ended; another attempt follows unless told to stop.
    Disconnected,
    /// The first connection failed: the viewer ends with this error.
    Failed(ClientError),
    /// The connection's task panicked: the viewer ends with this message.
    Crashed(String),
    /// The answer to [`Out::CheckUnit`].
    UnitChecked(Check),
    /// Someone asked for the windows again (`devlanes gui open`).
    Show,
    /// The local clipboard's text, read off the window thread when a window
    /// gained focus.
    LocalClipboard(String),
}

/// The newest decoded frame of each window, waiting for the window thread.
#[derive(Debug, Clone, Default)]
pub struct Frames(Arc<Mutex<HashMap<WindowId, DecodedFrame>>>);

impl Frames {
    /// Keeps `frame` as its window's newest; true when the slot was empty,
    /// so the window thread has to be told.
    fn put(&self, frame: DecodedFrame) -> bool {
        let mut slots = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        slots.insert(frame.window, frame).is_none()
    }

    /// The newest frame of `window`, emptying its slot.
    pub fn take(&self, window: WindowId) -> Option<DecodedFrame> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&window)
    }
}

/// How the viewer asks the daemon whether its service still runs: by running
/// `devlanes gui ls UNIT --json` from the environment the viewer was started
/// in, without starting a daemon.
#[derive(Debug, Clone)]
pub struct UnitCheck {
    /// The `devlanes` executable.
    pub devlanes: std::path::PathBuf,
    /// The unit, as `lane:unit`.
    pub unit: String,
}

/// The first wait before a reconnect, doubled after each failed attempt.
const FIRST_BACKOFF: Duration = Duration::from_millis(200);

/// The longest wait between attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// How long a connection may stay silent before it is given up and dialed
/// again: twice as long as the windows take to show Reconnecting.
const SILENCE: Duration = Duration::from_secs(RECONNECT_AFTER.as_secs() * 2);

/// The channel to the window thread: false once the window thread is gone.
pub type Tell = Arc<dyn Fn(FromLink) -> bool + Send + Sync>;

/// Runs the connection to `address` until `rx` closes or the viewer asks it
/// to stop reconnecting, reporting through `tell`, frames through `frames`.
pub async fn run(
    address: SocketAddr,
    options: ClientOptions,
    check: Option<UnitCheck>,
    mut rx: mpsc::UnboundedReceiver<Out>,
    tell: Tell,
    frames: Frames,
) {
    let mut first = true;
    let mut backoff = FIRST_BACKOFF;
    let mut reconnect = true;
    loop {
        let (client, mut events) = match connect(address, options.clone()).await {
            Ok(connected) => connected,
            Err(error) if first => {
                tell(FromLink::Failed(error));
                return;
            }
            Err(error) => {
                tracing::debug!(%error, "the GUI host did not answer; trying again");
                if !wait(&mut rx, backoff, &tell, &check, &mut reconnect).await || !reconnect {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        first = false;
        backoff = FIRST_BACKOFF;
        if !tell(FromLink::Connected(client.welcome().windows.clone())) {
            return;
        }
        let mut watch = tokio::time::interval(MAX_BACKOFF);
        loop {
            tokio::select! {
                event = events.next() => match event {
                    Some(Event::Closed(Some(error))) => {
                        tracing::info!(%error, "the GUI connection closed");
                        break;
                    }
                    Some(Event::Closed(None)) | None => break,
                    Some(Event::Frame(frame)) => {
                        let window = frame.window;
                        if frames.put(frame) && !tell(FromLink::Frame(window)) {
                            return;
                        }
                    }
                    Some(event) => {
                        if !tell(FromLink::Event(event)) {
                            return;
                        }
                    }
                },
                out = rx.recv() => match out {
                    Some(out) => apply(&client, out, &tell, &check, &mut reconnect),
                    None => return,
                },
                // A link that broke without closing (a sleeping laptop, a
                // network that went away) never ends by itself.
                _ = watch.tick() => {
                    if client.silent_for() >= SILENCE {
                        tracing::info!("the GUI connection went silent; dialing again");
                        break;
                    }
                }
            }
        }
        client.close();
        drop(events);
        if !tell(FromLink::Disconnected) {
            return;
        }
        // The viewer answers a final close with StopReconnecting right away.
        if !wait(&mut rx, Duration::ZERO, &tell, &check, &mut reconnect).await || !reconnect {
            return;
        }
    }
}

/// Waits `delay` while handling what the viewer asks with no connection.
/// False when the viewer is gone.
async fn wait(
    rx: &mut mpsc::UnboundedReceiver<Out>,
    delay: Duration,
    tell: &Tell,
    check: &Option<UnitCheck>,
    reconnect: &mut bool,
) -> bool {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        // What is already queued first, so a stop right after the end of a
        // connection is seen before the next attempt.
        while let Ok(out) = rx.try_recv() {
            idle(out, tell, check, reconnect);
        }
        if !*reconnect {
            return true;
        }
        tokio::select! {
            _ = &mut sleep => return true,
            out = rx.recv() => match out {
                Some(out) => idle(out, tell, check, reconnect),
                None => return false,
            },
        }
    }
}

/// Handles what the viewer asks while no connection is up: input has nowhere
/// to go, the rest is done.
fn idle(out: Out, tell: &Tell, check: &Option<UnitCheck>, reconnect: &mut bool) {
    match out {
        Out::StopReconnecting => *reconnect = false,
        Out::CheckUnit => start_check(tell, check),
        _ => {}
    }
}

/// Sends what the viewer asks on `client`. A failed send means the
/// connection is closing, which its events report.
fn apply(client: &Client, out: Out, tell: &Tell, check: &Option<UnitCheck>, reconnect: &mut bool) {
    let sent = match out {
        Out::Pointer(pointer) => client.pointer(pointer),
        Out::PointerLeft(window) => client.pointer_left(window),
        Out::Wheel(wheel) => client.wheel(wheel),
        Out::Key(window, code, down) => client.key(window, code, down),
        Out::Text(window, text) => client.text(window, text),
        // The request number only matters to the client's own lock state.
        Out::Resize(window, size) => client.resize(window, size).map(|_| ()),
        Out::Close(window) => client.close_window(window),
        Out::Focus(window, focused) => client.focus(window, focused),
        Out::Clipboard(text) => client.clipboard(text),
        other => {
            idle(other, tell, check, reconnect);
            Ok(())
        }
    };
    if let Err(error) = sent {
        tracing::debug!(%error, "GUI input was not sent");
    }
}

/// Runs the unit check in the background and reports its answer.
fn start_check(tell: &Tell, check: &Option<UnitCheck>) {
    let tell = tell.clone();
    let check = check.clone();
    tokio::spawn(async move {
        let answer = match check {
            Some(check) => unit_check(&check).await,
            None => Check::Unknown,
        };
        tell(FromLink::UnitChecked(answer));
    });
}

/// Exit code of a `devlanes` command whose unit or lane does not exist.
const NOT_FOUND_EXIT: i32 = 3;

/// Exit code of a `devlanes` command that found no daemon to ask.
const DAEMON_EXIT: i32 = 4;

/// How long the check may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// What `devlanes gui ls UNIT --json` says about the unit: its `runs` field;
/// gone when the unit or its lane does not exist (for a box's lane, only
/// when every box this machine knows is connected and none has it, which
/// `gui ls` says by that exit code); the daemon away when there is none to
/// ask, since `DEVLANES_NO_SPAWN` keeps the check from starting one.
async fn unit_check(check: &UnitCheck) -> Check {
    let mut command = tokio::process::Command::new(&check.devlanes);
    command
        .args(["gui", "ls", &check.unit, "--json"])
        .env(devlanes_proto::NO_SPAWN_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(CHECK_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        _ => return Check::Unknown,
    };
    match output.status.code() {
        Some(NOT_FOUND_EXIT) => return Check::Gone,
        Some(DAEMON_EXIT) => return Check::DaemonAway,
        Some(0) => {}
        _ => return Check::Unknown,
    }
    let runs = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|answer| answer["items"][0]["runs"].as_bool());
    match runs {
        Some(true) => Check::Runs,
        Some(false) => Check::Gone,
        None => Check::Unknown,
    }
}
