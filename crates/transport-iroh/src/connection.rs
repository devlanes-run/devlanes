//! Sessions over iroh connections: one QUIC stream per protocol stream, an
//! activity marker for the session's ping timeout, and the watcher that ends
//! a client's connection when only the relay is left.

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
use iroh::endpoint::{self, PathEvent, RecvStream, SendStream, VarInt, WeakConnectionHandle};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// How long the relay may stay the selected path before the client closes
/// the connection. On a network switch iroh selected the relay for 1 to 22 ms
/// before a new direct path; a blocked direct path left it there for 24 to
/// 37 s. 3 s keeps switches alive with room for internet round trips and
/// still ends blocked paths, so relayed session data is bounded by the
/// relay's rate limit over 3 s.
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
const LOST_DIRECT_PATH: &str = "the direct path to the box was lost and only the relay is left";

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
    watcher: JoinHandle<()>,
}

impl IrohConnection {
    /// Wraps a dialed `connection`, with `first` its first stream, and starts
    /// its relay watcher.
    pub(crate) fn new(connection: endpoint::Connection, first: (SendStream, RecvStream)) -> Self {
        let recorded = Arc::new(Mutex::new(None));
        let watcher = tokio::spawn(watch_relay(
            connection.weak_handle(),
            connection.path_events(),
            recorded.clone(),
        ));
        Self {
            connection,
            first: Mutex::new(Some(first)),
            activity: Activity::new(),
            recorded,
            watcher,
        }
    }
}

impl Drop for IrohConnection {
    /// Stops the relay watcher; the connection closes once its streams are gone.
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

#[async_trait]
impl Connection for IrohConnection {
    /// Hands out the stream opened while dialing, then opens new ones. The
    /// box grants 640 at once, so `open` waits only for streams the session
    /// dropped and the box has not freed yet.
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

/// Whether the relay has been the selected path long enough to give up on
/// a direct one: a relay selection starts [`RELAY_GRACE`], a direct one
/// cancels it, and moving between relays or between direct paths changes
/// nothing.
#[derive(Debug, Default)]
pub(crate) struct RelayWatch {
    deadline: Option<Instant>,
}

impl RelayWatch {
    /// Records that iroh selected a relay (`relay`) or a direct path at `now`.
    pub(crate) fn selected(&mut self, relay: bool, now: Instant) {
        match (relay, self.deadline) {
            (true, None) => self.deadline = Some(now + RELAY_GRACE),
            (true, Some(_)) => {}
            (false, _) => self.deadline = None,
        }
    }

    /// When the connection has to close unless a direct path is selected first.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// Closes the connection with [`NO_DIRECT_PATH`] once the relay has been
/// selected for [`RELAY_GRACE`], and records why.
///
/// Holds a weak handle: a strong one would keep the connection open after
/// the session dropped it. Ends with the connection's path events.
async fn watch_relay(
    handle: WeakConnectionHandle,
    mut events: endpoint::PathEventStream,
    recorded: Arc<Mutex<Option<String>>>,
) {
    let mut watch = RelayWatch::default();
    loop {
        let expiry = async {
            match watch.deadline() {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = events.next() => match event {
                None => return,
                Some(PathEvent::Selected { remote_addr, .. }) => {
                    watch.selected(remote_addr.is_relay(), Instant::now());
                }
                // Missed events may include selections; the current selected
                // path is what counts.
                Some(PathEvent::Lagged { .. }) => {
                    let Some(connection) = handle.upgrade() else { return };
                    let selected = connection.paths().iter().find(|path| path.is_selected()).map(|path| path.is_relay());
                    if let Some(relay) = selected {
                        watch.selected(relay, Instant::now());
                    }
                }
                Some(_) => {}
            },
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

    /// The watcher gives up only after the relay stayed selected for the
    /// whole grace: a direct path selected in time cancels it, and direct
    /// to direct moves never start it.
    #[test]
    fn only_a_relay_selected_for_the_whole_grace_closes() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        // Selections as (milliseconds after the start, relay), and when the
        // connection must close.
        type Case<'a> = (&'a str, &'a [(u64, bool)], Option<Instant>);
        let cases: [Case; 5] = [
            ("relay stays", &[(0, false), (100, true)], Some(at(3_100))),
            (
                "relay, then direct within the grace",
                &[(0, true), (2_900, false)],
                None,
            ),
            (
                "direct to direct",
                &[(0, false), (5_000, false), (9_000, false)],
                None,
            ),
            (
                "relay to another relay keeps the first deadline",
                &[(0, false), (100, true), (2_000, true)],
                Some(at(3_100)),
            ),
            (
                "relay again after a direct path starts over",
                &[(0, true), (1_000, false), (4_000, true)],
                Some(at(7_000)),
            ),
        ];
        for (name, selections, closes) in cases {
            let mut watch = RelayWatch::default();
            for &(millis, relay) in selections {
                watch.selected(relay, at(millis));
            }
            assert_eq!(watch.deadline(), closes, "{name}");
        }
    }
}
