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
use std::time::{Duration, Instant};

use devlanes_gui_client::{Client, ClientError, ClientOptions, DecodedFrame, Event, connect};
use devlanes_gui_proto::{CodecId, RECONNECT_AFTER, WindowId, WindowInfo};
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
    /// gained focus, by a read that started when the host had sent that many
    /// clipboard texts ([`crate::model::Viewer::host_clipboards`]).
    LocalClipboard(String, u64),
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

    /// Empties every slot: the frames of a connection that ended are not
    /// shown after the next one's windows took over.
    fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
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
    /// The box the unit's lane was found on, for a box's lane.
    pub host_box: Option<String>,
}

/// The first wait before a reconnect, doubled after each failed attempt.
const FIRST_BACKOFF: Duration = Duration::from_millis(200);

/// The longest wait between attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// How long a connection must last for the backoff to start over.
const STABLE_AFTER: Duration = Duration::from_secs(5);

/// How long a connection may stay silent before it is given up and dialed
/// again: twice as long as the windows take to show Reconnecting.
const SILENCE: Duration = Duration::from_secs(RECONNECT_AFTER.as_secs() * 2);

/// How long to wait before dialing again.
#[derive(Debug)]
struct Backoff {
    /// The wait after the next failure.
    next: Duration,
    /// When the connection up now was made.
    connected_at: Option<Instant>,
}

impl Backoff {
    /// The first wait at [`FIRST_BACKOFF`].
    fn new() -> Self {
        Self {
            next: FIRST_BACKOFF,
            connected_at: None,
        }
    }

    /// A dial failed: the wait before the next, which doubles up to
    /// [`MAX_BACKOFF`].
    fn failed(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(MAX_BACKOFF);
        wait
    }

    /// A connection was made at `now`.
    fn connected(&mut self, now: Instant) {
        self.connected_at = Some(now);
    }

    /// The connection ended at `now`: the wait before dialing again. A host
    /// that broke the protocol is waited for like a failed dial, and the
    /// wait does not start over until a connection ends for another reason,
    /// so one that keeps doing it is dialed at most every [`MAX_BACKOFF`].
    /// Another close is followed by a dial at once, and one of a connection
    /// that lasted [`STABLE_AFTER`] starts the backoff over.
    fn closed(&mut self, now: Instant, broke_protocol: bool) -> Duration {
        let lasted = self
            .connected_at
            .take()
            .is_some_and(|at| now.saturating_duration_since(at) >= STABLE_AFTER);
        if lasted && !broke_protocol {
            self.next = FIRST_BACKOFF;
        }
        match broke_protocol {
            true => self.failed(),
            false => Duration::ZERO,
        }
    }
}

/// The channel to the window thread: false once the window thread is gone.
pub type Tell = Arc<dyn Fn(FromLink) -> bool + Send + Sync>;

/// Runs the connection to `address` until `rx` closes or the viewer asks it
/// to stop reconnecting, reporting through `tell`, frames through `frames`.
pub async fn run(
    address: SocketAddr,
    mut options: ClientOptions,
    check: Option<UnitCheck>,
    mut rx: mpsc::UnboundedReceiver<Out>,
    tell: Tell,
    frames: Frames,
) {
    let mut first = true;
    let mut backoff = Backoff::new();
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
                let delay = backoff.failed();
                if !wait(&mut rx, delay, &tell, &check, &mut reconnect).await || !reconnect {
                    return;
                }
                continue;
            }
        };
        first = false;
        backoff.connected(Instant::now());
        if !tell(FromLink::Connected(client.welcome().windows.clone())) {
            return;
        }
        let mut broke_protocol = false;
        let mut watch = tokio::time::interval(MAX_BACKOFF);
        loop {
            tokio::select! {
                event = events.next() => match event {
                    // A host that breaks the protocol is dialed again after
                    // the backoff; the decoders stay, since none of them
                    // failed.
                    Some(Event::Closed(Some(error @ ClientError::UnsupportedCodec(_)))) => {
                        tracing::warn!(%error, "the GUI host sent a frame in a codec this viewer did not offer");
                        broke_protocol = true;
                        break;
                    }
                    Some(Event::Closed(Some(error))) => {
                        tracing::info!(%error, "the GUI connection closed");
                        break;
                    }
                    Some(Event::Closed(None)) | None => break,
                    // The protocol has no way to change codecs on a live
                    // connection, so the viewer dials again without the
                    // decoder; `raw`, which every host has, always stays.
                    Some(Event::CodecFailed(codec)) if codec != CodecId::RAW => {
                        tracing::warn!(codec = ?codec.name(), "a GUI decoder failed twice; reconnecting without it");
                        options.decoders.retain(|decoder| decoder.offer().codec != codec);
                        break;
                    }
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
        frames.clear();
        if !tell(FromLink::Disconnected) {
            return;
        }
        // The viewer answers a final close with StopReconnecting right away.
        let delay = backoff.closed(Instant::now(), broke_protocol);
        if !wait(&mut rx, delay, &tell, &check, &mut reconnect).await || !reconnect {
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

/// Asks `devlanes gui ls UNIT --json` about the unit, without starting a
/// daemon (`DEVLANES_NO_SPAWN`); see [`answer`]. For a box's lane that was
/// not found, `devlanes remote ls --json` then says whether its box is away
/// ([`box_away`]).
async fn unit_check(check: &UnitCheck) -> Check {
    let found = run_json(&check.devlanes, &["gui", "ls", &check.unit, "--json"]).await;
    let Some((code, stdout)) = found else {
        return Check::Unknown;
    };
    let answer = answer(code, &stdout);
    let Some(host_box) = check.host_box.as_deref().filter(|_| answer == Check::Gone) else {
        return answer;
    };
    if code != Some(NOT_FOUND_EXIT) {
        return answer;
    }
    match run_json(&check.devlanes, &["remote", "ls", "--json"]).await {
        Some((Some(0), boxes)) if box_away(&boxes, host_box) == Some(false) => Check::Gone,
        _ => Check::Unknown,
    }
}

/// Runs `devlanes` with `args` and no daemon start, for its exit code and
/// standard output; `None` when it did not run or took too long.
async fn run_json(devlanes: &std::path::Path, args: &[&str]) -> Option<(Option<i32>, Vec<u8>)> {
    let mut command = tokio::process::Command::new(devlanes);
    command
        .args(args)
        .env(devlanes_proto::NO_SPAWN_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(CHECK_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => Some((output.status.code(), output.stdout)),
        _ => None,
    }
}

/// What a `gui ls UNIT --json` that exited with `code` and printed `stdout`
/// says about the unit: its `runs` field; gone when the unit or its lane
/// does not exist; the daemon away when there is none to ask.
fn answer(code: Option<i32>, stdout: &[u8]) -> Check {
    match code {
        Some(NOT_FOUND_EXIT) => Check::Gone,
        Some(DAEMON_EXIT) => Check::DaemonAway,
        Some(0) => {
            let runs = serde_json::from_slice::<serde_json::Value>(stdout)
                .ok()
                .and_then(|answer| answer["items"][0]["runs"].as_bool());
            match runs {
                Some(true) => Check::Runs,
                Some(false) => Check::Gone,
                None => Check::Unknown,
            }
        }
        _ => Check::Unknown,
    }
}

/// Whether `remote ls --json` (`stdout`) lists `host_box` as known but not
/// connected. A box no longer known is not away: its lanes are gone. `None`
/// when the output says nothing readable.
fn box_away(stdout: &[u8], host_box: &str) -> Option<bool> {
    let list = serde_json::from_slice::<serde_json::Value>(stdout).ok()?;
    let items = list["items"].as_array()?;
    Some(
        items
            .iter()
            .find(|item| item["host"] == host_box)
            .is_some_and(|item| item["state"] != "connected"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `remote ls --json` with `boxes` and their states.
    fn boxes(boxes: &[(&str, &str)]) -> Vec<u8> {
        let items: Vec<_> = boxes
            .iter()
            .map(|(host, state)| serde_json::json!({"host": host, "state": state}))
            .collect();
        serde_json::json!({ "items": items })
            .to_string()
            .into_bytes()
    }

    /// A box lane found nowhere is gone when its own box is connected or no
    /// longer known, also while another box is offline; only its own box
    /// being away leaves it unknown.
    #[test]
    fn a_missing_box_lane_is_gone_unless_its_own_box_is_away() {
        // The lane was cleaned while another box is offline.
        let other_offline = boxes(&[("hetzner", "connected"), ("other", "disconnected")]);
        assert_eq!(box_away(&other_offline, "hetzner"), Some(false));
        // The viewer's own box is offline.
        let own_offline = boxes(&[("hetzner", "connecting"), ("other", "connected")]);
        assert_eq!(box_away(&own_offline, "hetzner"), Some(true));
        // The box was removed.
        assert_eq!(
            box_away(&boxes(&[("other", "disconnected")]), "hetzner"),
            Some(false)
        );
        // Output that says nothing readable is no answer either way.
        assert_eq!(box_away(b"not json", "hetzner"), None);
        assert_eq!(box_away(b"{}", "hetzner"), None);
        assert_eq!(answer(Some(NOT_FOUND_EXIT), b""), Check::Gone);
    }

    /// A host that keeps breaking the protocol is dialed after a doubling
    /// wait, up to the longest; a connection that lasted starts it over, and
    /// an ordinary close is followed by a dial at once.
    #[test]
    fn protocol_errors_back_off_until_a_connection_lasts() {
        let t0 = Instant::now();
        let mut backoff = Backoff::new();
        let mut waits = Vec::new();
        for n in 0..6 {
            let at = t0 + Duration::from_secs(n);
            backoff.connected(at);
            waits.push(
                backoff
                    .closed(at + Duration::from_millis(50), true)
                    .as_millis(),
            );
        }
        assert_eq!(waits, [200, 400, 800, 1600, 2000, 2000]);
        // A host that lasts a while before breaking the protocol again
        // stays at the longest wait.
        let later = t0 + Duration::from_secs(60);
        backoff.connected(later);
        let wait = backoff.closed(later + STABLE_AFTER * 2, true);
        assert_eq!(wait, MAX_BACKOFF, "a protocol error never starts over");
        // A connection that lasted and ended otherwise starts it over.
        let after = later + STABLE_AFTER * 3;
        backoff.connected(after);
        assert_eq!(backoff.closed(after + STABLE_AFTER, false), Duration::ZERO);
        backoff.connected(after + STABLE_AFTER);
        let wait = backoff.closed(after + STABLE_AFTER, true);
        assert_eq!(wait, FIRST_BACKOFF);
    }

    /// A unit that exists is gone only when it does not run.
    #[test]
    fn the_runs_field_decides_for_a_unit_that_exists() {
        let items = |runs: bool| {
            serde_json::json!({"items": [{"runs": runs}]})
                .to_string()
                .into_bytes()
        };
        assert_eq!(answer(Some(0), &items(true)), Check::Runs);
        assert_eq!(answer(Some(0), &items(false)), Check::Gone);
        assert_eq!(answer(Some(0), b"{}"), Check::Unknown);
        assert_eq!(answer(Some(DAEMON_EXIT), b""), Check::DaemonAway);
        assert_eq!(answer(Some(1), b""), Check::Unknown);
    }
}
