//! Sessions over iroh connections: one QUIC stream per protocol stream, an
//! activity marker for the session's ping timeout, and the watcher that ends
//! a client's connection once it has had no direct path for a few seconds.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{
    BoxDuplex, Connection, ConnectionError, Joined, PeerError, ServerConnection,
};
use futures_util::StreamExt;
use iroh::endpoint::{self, RecvStream, SendStream, VarInt, WeakConnectionHandle};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;

/// How long a session may go without a direct path selected (the relay or
/// nothing) before the client closes the connection, counted from when the
/// watcher notices, which is at most [`SELECTION_TICK`] late. On a network
/// switch iroh selected the relay for 1 to 22 ms before a new direct path; a
/// blocked direct path left it there for 24 to 37 s. 3 s keeps switches
/// alive with room for internet round trips and still ends blocked paths,
/// so relayed session data is bounded by the relay's rate limit over about
/// 3 s.
pub(crate) const RELAY_GRACE: Duration = Duration::from_secs(3);

/// How long a dropped box side waits for the client to close before closing
/// itself, like yamux's close timeout: QUIC's close discards stream data the
/// client has not read yet, such as a refusal written just before the drop.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Application close code of a connection the box refused: a key it does not
/// admit, in the handshake or later through `close_peer`.
pub(crate) const REFUSED: u32 = 403;

/// Application close code of a close the client starts itself because the
/// connection may not use the path it has.
pub(crate) const NO_DIRECT_PATH: u32 = 1;

/// What the relay watcher records when it closes the connection.
const LOST_DIRECT_PATH: &str = "the direct connection to the box was lost for 3 seconds";

/// When stream data last arrived from the peer, on tokio's clock.
///
/// Touched by stream reads only, so QUIC keep-alives and ACKs, which the
/// peer's network stack sends whether or not its process runs, never count.
#[derive(Debug)]
pub(crate) struct Activity {
    last: Mutex<Instant>,
}

impl Activity {
    /// Starts at "now", the connection's start.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            last: Mutex::new(Instant::now()),
        })
    }

    /// Records bytes from the peer.
    fn touch(&self) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    /// When bytes last arrived.
    fn last(&self) -> Instant {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A stream's receive half with its reads recorded in an [`Activity`].
struct Watched {
    recv: RecvStream,
    activity: Arc<Activity>,
}

impl AsyncRead for Watched {
    /// Reads from the stream and touches the marker when bytes arrived.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let poll = Pin::new(&mut this.recv).poll_read(cx, buf);
        if matches!(poll, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            this.activity.touch();
        }
        poll
    }
}

/// One bidirectional QUIC stream as a duplex whose reads count as activity.
fn duplex(send: SendStream, recv: RecvStream, activity: &Arc<Activity>) -> BoxDuplex {
    let recv = Watched {
        recv,
        activity: activity.clone(),
    };
    Box::new(Joined::new(recv, send))
}

/// The text a session shows for how a connection closed; `None` for a close
/// either side asked for. `recorded` is a reason this side closed it for.
fn failure(recorded: Option<String>, closed: Option<endpoint::ConnectionError>) -> Option<String> {
    if recorded.is_some() {
        return recorded;
    }
    match closed? {
        endpoint::ConnectionError::LocallyClosed => None,
        endpoint::ConnectionError::ApplicationClosed(close) => {
            match close.error_code.into_inner() {
                0 => None,
                code if code == u64::from(REFUSED) => Some(PeerError::Refused.to_string()),
                _ => Some(endpoint::ConnectionError::ApplicationClosed(close).to_string()),
            }
        }
        other => Some(other.to_string()),
    }
}

/// The client's side of a session's connection to a box.
pub(crate) struct IrohConnection {
    connection: endpoint::Connection,
    /// A stream opened while dialing to learn that the box admitted this
    /// machine; the first `open` hands it out before opening more.
    first: Mutex<Option<(SendStream, RecvStream)>>,
    activity: Arc<Activity>,
    /// Why this side closed the connection, when the relay watcher did.
    recorded: Arc<Mutex<Option<String>>>,
}

impl IrohConnection {
    /// Wraps a dialed `connection`, with `first` its first stream, and starts
    /// its relay watcher on `events`, which the dial subscribed to before it
    /// checked the paths.
    ///
    /// The watcher is never aborted, also not when this wrapper drops: open
    /// streams keep the connection alive after that, and it must still close
    /// a session left on the relay. It holds only a weak handle and ends once
    /// the connection is closed, so it cannot outlive it.
    pub(crate) fn new(
        connection: endpoint::Connection,
        first: (SendStream, RecvStream),
        events: endpoint::PathEventStream,
    ) -> Self {
        let recorded = Arc::new(Mutex::new(None));
        tokio::spawn(watch_relay(
            connection.weak_handle(),
            events,
            recorded.clone(),
        ));
        Self {
            connection,
            first: Mutex::new(Some(first)),
            activity: Activity::new(),
            recorded,
        }
    }
}

#[async_trait]
impl Connection for IrohConnection {
    /// Hands out the stream opened while dialing, then opens new ones. The
    /// box grants a session 640 at once, so `open` waits only for streams the
    /// session dropped and the box has not freed yet; a pairing connection
    /// gets 1, the one its exchange uses.
    async fn open(&self) -> Result<BoxDuplex, ConnectionError> {
        if self.connection.close_reason().is_some() {
            return Err(ConnectionError::Closed);
        }
        let first = self
            .first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let (send, recv) = match first {
            Some(stream) => stream,
            None => self
                .connection
                .open_bi()
                .await
                .map_err(|_| ConnectionError::Closed)?,
        };
        Ok(duplex(send, recv, &self.activity))
    }

    /// When stream data last arrived from the box.
    fn last_inbound(&self) -> Instant {
        self.activity.last()
    }

    /// Waits up to `limit` for the close, then reports what closed it.
    async fn failure_within(&self, limit: Duration) -> Option<String> {
        let closed = tokio::time::timeout(limit, self.connection.closed())
            .await
            .ok();
        let recorded = self
            .recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        failure(recorded, closed)
    }

    /// Closes the connection with code 0, which the box reads as a clean close.
    fn shutdown(&self) {
        self.connection.close(VarInt::from_u32(0), b"");
    }

    /// Resolves once the connection is closed for any reason.
    async fn closed(&self) {
        self.connection.closed().await;
    }
}

/// The box's side of a connection a client opened.
pub(crate) struct IrohServerConnection {
    connection: endpoint::Connection,
    activity: Arc<Activity>,
}

impl IrohServerConnection {
    /// Wraps an accepted `connection`.
    pub(crate) fn new(connection: endpoint::Connection) -> Self {
        Self {
            connection,
            activity: Activity::new(),
        }
    }
}

#[async_trait]
impl ServerConnection for IrohServerConnection {
    /// The next stream the client opened; `None` once the connection closed,
    /// including when the client closed it.
    async fn accept(&mut self) -> Option<BoxDuplex> {
        let (send, recv) = self.connection.accept_bi().await.ok()?;
        Some(duplex(send, recv, &self.activity))
    }

    /// When stream data last arrived from the client.
    fn last_inbound(&self) -> Instant {
        self.activity.last()
    }
}

impl Drop for IrohServerConnection {
    /// Gives the client up to [`CLOSE_WAIT`] to read what the box wrote last
    /// and close first, then closes. Without a runtime, as when one is shutting
    /// down, it closes at once.
    fn drop(&mut self) {
        let connection = self.connection.clone();
        let close = move || connection.close(VarInt::from_u32(0), b"");
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let waited = self.connection.clone();
                runtime.spawn(async move {
                    let _ = tokio::time::timeout(CLOSE_WAIT, waited.closed()).await;
                    close();
                });
            }
            Err(_) => close(),
        }
    }
}

/// How often the relay watcher reads the selected path without an event:
/// iroh clears the selection without one when the selected path's address
/// is no longer among the connection's paths (`record_selected` in
/// `path_watcher.rs`). The grace starts only when the watcher notices, so
/// this is how much a session may run past [`RELAY_GRACE`] without a direct
/// path; 250 ms keeps that near 3 s for one cheap read of the path list
/// four times a second.
const SELECTION_TICK: Duration = Duration::from_millis(250);

/// Whether a session has gone without a direct path long enough to give
/// up: anything but a direct (IP) path selected, a relay or nothing, starts
/// [`RELAY_GRACE`], a direct path cancels it, and staying in either state
/// changes nothing.
#[derive(Debug, Default)]
pub(crate) struct RelayWatch {
    deadline: Option<Instant>,
}

impl RelayWatch {
    /// Records at `now` whether a direct path is the selected one.
    pub(crate) fn observe(&mut self, direct: bool, now: Instant) {
        match (direct, self.deadline) {
            (false, None) => self.deadline = Some(now + RELAY_GRACE),
            (false, Some(_)) => {}
            (true, _) => self.deadline = None,
        }
    }

    /// When the connection has to close unless a direct path is selected first.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// Whether the connection behind `handle` has a direct path selected;
/// `None` once it is gone or closed.
fn direct_selected(handle: &WeakConnectionHandle) -> Option<bool> {
    let connection = handle.upgrade()?;
    if connection.close_reason().is_some() {
        return None;
    }
    let direct = connection
        .paths()
        .iter()
        .any(|path| path.is_selected() && path.is_ip());
    Some(direct)
}

/// Closes the connection with [`NO_DIRECT_PATH`] once it has had no direct
/// path selected for [`RELAY_GRACE`], and records why.
///
/// `events` was subscribed before the dial's gate read the paths. The state
/// is seeded from the current selection, then read again on every path event
/// and every [`SELECTION_TICK`], since iroh changes the selection without an
/// event in one case; the event only says when to look. A missed event
/// (`Lagged`) needs nothing more for the same reason.
///
/// Holds a weak handle, upgraded only for each read: a strong one would keep
/// the connection open after the session dropped it. Ends once the
/// connection is closed, when its path events end or a read finds it gone.
async fn watch_relay(
    handle: WeakConnectionHandle,
    mut events: endpoint::PathEventStream,
    recorded: Arc<Mutex<Option<String>>>,
) {
    let mut watch = RelayWatch::default();
    let Some(direct) = direct_selected(&handle) else {
        return;
    };
    watch.observe(direct, Instant::now());
    let mut tick = tokio::time::interval(SELECTION_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let expiry = async {
            match watch.deadline() {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = events.next() => {
                if event.is_none() {
                    return;
                }
                let Some(direct) = direct_selected(&handle) else { return };
                watch.observe(direct, Instant::now());
            }
            _ = tick.tick() => {
                let Some(direct) = direct_selected(&handle) else { return };
                watch.observe(direct, Instant::now());
            }
            () = expiry => {
                tracing::debug!("{LOST_DIRECT_PATH}; closing the connection");
                *recorded.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(LOST_DIRECT_PATH.to_owned());
                if let Some(connection) = handle.upgrade() {
                    connection.close(VarInt::from_u32(NO_DIRECT_PATH), LOST_DIRECT_PATH.as_bytes());
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The watcher gives up only after the session went without a direct
    /// path for the whole grace, whatever it had instead (a relay or
    /// nothing) and whether that was already so when it started: a direct
    /// path selected in time cancels it, and direct to direct moves never
    /// start it.
    #[test]
    fn only_a_session_without_a_direct_path_for_the_whole_grace_closes() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        // Reads as (milliseconds after the start, a direct path selected),
        // and when the connection must close.
        type Case<'a> = (&'a str, &'a [(u64, bool)], Option<Instant>);
        let cases: [Case; 8] = [
            ("relay stays", &[(0, true), (100, false)], Some(at(3_100))),
            ("seeded on the relay", &[(0, false)], Some(at(3_000))),
            (
                "selection cleared, no path at all",
                &[(0, true), (500, false), (1_500, false)],
                Some(at(3_500)),
            ),
            (
                "relay, then direct within the grace",
                &[(0, false), (2_900, true)],
                None,
            ),
            (
                "direct to direct",
                &[(0, true), (5_000, true), (9_000, true)],
                None,
            ),
            (
                "staying off a direct path keeps the first deadline",
                &[(0, true), (100, false), (2_000, false)],
                Some(at(3_100)),
            ),
            (
                "losing the direct path again starts over",
                &[(0, false), (1_000, true), (4_000, false)],
                Some(at(7_000)),
            ),
            (
                "a direct path at the last read before the deadline",
                &[(0, false), (2_999, true), (3_500, true)],
                None,
            ),
        ];
        for (name, reads, closes) in cases {
            let mut watch = RelayWatch::default();
            for &(millis, direct) in reads {
                watch.observe(direct, at(millis));
            }
            assert_eq!(watch.deadline(), closes, "{name}");
        }
    }
}
