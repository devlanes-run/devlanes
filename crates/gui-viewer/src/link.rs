//! The connection to the GUI host, on a tokio runtime of its own off the
//! window thread: it connects, reconnects with a short backoff after the
//! connection ends, forwards what the host sends, and sends what the
//! [`crate::model::Viewer`] asks.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use devlanes_gui_client::{Client, ClientError, ClientOptions, Event, connect};
use devlanes_gui_proto::WindowInfo;
use tokio::sync::mpsc;

use crate::model::Out;

/// What the connection tells the window thread.
#[derive(Debug)]
pub enum FromLink {
    /// A connection is up with these windows.
    Connected(Vec<WindowInfo>),
    /// Something the host sent.
    Event(Event),
    /// The connection ended; another attempt follows unless told to stop.
    Disconnected,
    /// The first connection failed: the viewer ends with this error.
    Failed(ClientError),
    /// The answer to [`Out::CheckUnit`]: whether the service still runs.
    UnitChecked(bool),
    /// Someone asked for the windows again (`devlanes gui open`).
    Show,
}

/// How the viewer asks the daemon whether its service still runs: by running
/// `devlanes gui ls UNIT --json` from the environment the viewer was started in.
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

/// Runs the connection to `address` until `rx` closes or the viewer asks it
/// to stop reconnecting, reporting through `tell`, which returns false once
/// the window thread is gone.
pub async fn run(
    address: SocketAddr,
    options: ClientOptions,
    check: Option<UnitCheck>,
    mut rx: mpsc::UnboundedReceiver<Out>,
    tell: Arc<dyn Fn(FromLink) -> bool + Send + Sync>,
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
        loop {
            tokio::select! {
                event = events.next() => match event {
                    Some(Event::Closed(Some(error))) => {
                        tracing::info!(%error, "the GUI connection closed");
                        break;
                    }
                    Some(Event::Closed(None)) | None => break,
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
            }
        }
        client.close();
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
    tell: &Arc<dyn Fn(FromLink) -> bool + Send + Sync>,
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
fn idle(
    out: Out,
    tell: &Arc<dyn Fn(FromLink) -> bool + Send + Sync>,
    check: &Option<UnitCheck>,
    reconnect: &mut bool,
) {
    match out {
        Out::StopReconnecting => *reconnect = false,
        Out::CheckUnit => start_check(tell, check),
        _ => {}
    }
}

/// Sends what the viewer asks on `client`. A failed send means the
/// connection is closing, which its events report.
fn apply(
    client: &Client,
    out: Out,
    tell: &Arc<dyn Fn(FromLink) -> bool + Send + Sync>,
    check: &Option<UnitCheck>,
    reconnect: &mut bool,
) {
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

/// Runs the unit check in the background and reports its answer. Without a
/// check, or when the check cannot tell, the service counts as running.
fn start_check(tell: &Arc<dyn Fn(FromLink) -> bool + Send + Sync>, check: &Option<UnitCheck>) {
    let tell = tell.clone();
    let check = check.clone();
    tokio::spawn(async move {
        let running = match check {
            Some(check) => unit_runs(&check).await.unwrap_or(true),
            None => true,
        };
        tell(FromLink::UnitChecked(running));
    });
}

/// Exit code of a `devlanes` command whose unit or lane does not exist.
const NOT_FOUND_EXIT: i32 = 3;

/// How long the check may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether the unit still runs, from `devlanes gui ls UNIT --json`: its
/// `runs` field, or `false` when the unit or its lane is gone. `None` when
/// the command could not tell, such as with no daemon to ask.
async fn unit_runs(check: &UnitCheck) -> Option<bool> {
    let mut command = tokio::process::Command::new(&check.devlanes);
    command
        .args(["gui", "ls", &check.unit, "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(CHECK_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if output.status.code() == Some(NOT_FOUND_EXIT) {
        return Some(false);
    }
    if !output.status.success() {
        return None;
    }
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    answer["items"][0]["runs"].as_bool()
}
