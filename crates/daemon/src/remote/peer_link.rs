//! Client side of peer-to-peer connections: dialing a box paired by ticket
//! ([`PeerTransport`]) and how long its link keeps trying ([`PeerAttempts`]).
//!
//! Both wrap [`Transport`] for [`devlanes_transport::Link`], whose reconnect
//! rules stay as they are for every box: a P2P box only differs in that it
//! stops being retried after a day away, an hour for an ephemeral one, and
//! at once when the box no longer accepts this machine's key.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use devlanes_transport::{
    Alpn, BoxConnection, BoxDuplex, Connection, ConnectionError, PeerAddr, PeerEndpoint, PeerError,
    Transport, TransportError,
};
use futures_util::FutureExt as _;
use tokio::time::Instant;

use crate::model::lock;

/// How long a box paired by ticket may stay unreachable before its link
/// stops trying.
pub(super) const RETRY_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// [`RETRY_FOR`] for a box paired with an ephemeral ticket, which is then
/// forgotten.
pub(super) const EPHEMERAL_RETRY_FOR: Duration = Duration::from_secs(60 * 60);

/// Why a link to a box paired by ticket stopped for good. Not a
/// [`PeerError`]: their texts are about one dial, and wrapping these in one
/// would say twice that the box is not reachable.
#[derive(Debug, thiserror::Error)]
pub(super) enum PeerLinkError {
    /// The box was unreachable for [`RETRY_FOR`].
    #[error(
        "box {name} has not been reachable for 24 hours, so devlanes stopped trying. Run \
         `devlanes remote connect {name}` once it is back."
    )]
    GaveUp {
        /// The box's name here.
        name: String,
    },
    /// An ephemeral box was unreachable for [`EPHEMERAL_RETRY_FOR`], so it
    /// is forgotten.
    #[error(
        "box {name} has not been reachable for an hour, so devlanes stopped trying and forgot \
         it. Pair with it again using a new ticket from `devlanes daemon pair` on the box."
    )]
    GaveUpEphemeral {
        /// The box's name here.
        name: String,
    },
    /// The box refused this machine's key, as after `daemon revoke` there.
    #[error(
        "box {name} no longer accepts this machine. Run `devlanes daemon pair` on the box, then \
         `devlanes remote connect <ticket>` here."
    )]
    Refused {
        /// The box's name here.
        name: String,
    },
}

/// What [`PeerTransport`] remembers of the last connection it returned.
#[derive(Default)]
struct LastConnection {
    /// The serial of that connection; an older one writes nothing here.
    serial: u64,
    /// The box closed that connection because it refused this machine's key.
    refused: bool,
}

/// Dials one box paired by ticket, pinned to the key the pairing stored.
///
/// A box that revokes its last client paired by ticket closes that client's
/// session as refused and then stops listening, so the next dial finds no
/// listener, which is transient. So every connection this returns reports a
/// refusal it sees back here, and the next [`Transport::connect`] fails as
/// refused, for good, without dialing. Only the record is shared: a strong
/// reference to an earlier connection would keep that QUIC connection open.
pub(super) struct PeerTransport {
    endpoint: Arc<dyn PeerEndpoint>,
    addr: PeerAddr,
    last: Arc<Mutex<LastConnection>>,
}

impl PeerTransport {
    /// Dials `addr` through `endpoint`. A new transport, as a new link gets,
    /// remembers no refusal.
    pub(super) fn new(endpoint: Arc<dyn PeerEndpoint>, addr: PeerAddr) -> Self {
        Self {
            endpoint,
            addr,
            last: Arc::default(),
        }
    }
}

#[async_trait]
impl Transport for PeerTransport {
    /// A session connection to the box, or [`PeerError::Refused`] without a
    /// dial when the box refused the last one.
    async fn connect(&self) -> Result<BoxConnection, TransportError> {
        if lock(&self.last).refused {
            return Err(PeerError::Refused.into());
        }
        let inner = self.endpoint.connect(&self.addr, Alpn::Tunnel).await?;
        let serial = {
            let mut last = lock(&self.last);
            last.serial += 1;
            last.refused = false;
            last.serial
        };
        Ok(Box::new(Watched {
            inner,
            last: self.last.clone(),
            serial,
        }))
    }
}

/// A connection [`PeerTransport`] returned, which writes a refusal it sees
/// into the transport's record.
///
/// The session asks [`Connection::failure_within`] when the connection
/// closes, on a read error and on a failed handshake, but not when it ends
/// through a failed ping or report write, which can coincide with the
/// box's refusal. Every ending calls [`Connection::shutdown`], so that and
/// [`Connection::closed`] look for the refusal too.
struct Watched {
    inner: BoxConnection,
    last: Arc<Mutex<LastConnection>>,
    serial: u64,
}

impl Watched {
    /// Records `reason` as a refusal when it is the box refusing this
    /// machine's key and this is still the transport's newest connection.
    fn note(&self, reason: Option<&str>) {
        // `contains`: the memory endpoint's reason comes wrapped in yamux's.
        let refused = PeerError::Refused.to_string();
        if reason.is_some_and(|reason| reason.contains(&refused)) {
            let mut last = lock(&self.last);
            if last.serial == self.serial {
                last.refused = true;
            }
        }
    }

    /// Records the failure reason of a connection that is already closed,
    /// without waiting. Runs from `shutdown`, which is not async and may run
    /// from a drop: only a future that is ready is polled, so no timer is
    /// waited on, and nothing runs outside a runtime, where creating one
    /// would panic.
    fn note_if_closed(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.inner.closed().now_or_never().is_none() {
            return;
        }
        if let Some(reason) = self.inner.failure_within(Duration::ZERO).now_or_never() {
            self.note(reason.as_deref());
        }
    }
}

#[async_trait]
impl Connection for Watched {
    /// Opens a stream on the connection.
    async fn open(&self) -> Result<BoxDuplex, ConnectionError> {
        self.inner.open().await
    }

    /// When the box last sent data.
    fn last_inbound(&self) -> Instant {
        self.inner.last_inbound()
    }

    /// The connection's failure reason, recorded when it is a refusal.
    async fn failure_within(&self, limit: Duration) -> Option<String> {
        let reason = self.inner.failure_within(limit).await;
        self.note(reason.as_deref());
        reason
    }

    /// Looks for a refusal before closing: after this side's own close the
    /// box's reason could no longer be told apart.
    fn shutdown(&self) {
        self.note_if_closed();
        self.inner.shutdown();
    }

    /// Resolves once closed, after recording a refusal.
    async fn closed(&self) {
        self.inner.closed().await;
        let reason = self.inner.failure_within(Duration::ZERO).await;
        self.note(reason.as_deref());
    }
}

/// Bounds how long the link to a box paired by ticket keeps trying, around
/// its [`PeerTransport`].
///
/// A failed dial past the deadline, and a refused key at once, become
/// permanent [`PeerLinkError`]s, which end the link `failed`. The deadline
/// is on tokio's clock: it only measures time this daemon ran, and the
/// time a box was away before the daemon started comes in through
/// [`PeerAttempts::new`], from stored wall-clock times.
pub(super) struct PeerAttempts {
    inner: Arc<dyn Transport>,
    name: String,
    ephemeral: bool,
    /// When the next failed dial ends the link; `None` while the last dial
    /// connected, so the box was reachable then.
    deadline: Mutex<Option<Instant>>,
    /// Set once the link stopped trying because the box stayed away.
    gave_up: Arc<AtomicBool>,
}

impl PeerAttempts {
    /// Tries box `name` through `inner` for [`RETRY_FOR`], or
    /// [`EPHEMERAL_RETRY_FOR`] when `ephemeral`, less `away`: how long the
    /// box had not been reached when the link started (zero for a link a
    /// person started).
    pub(super) fn new(
        inner: Arc<dyn Transport>,
        name: &str,
        ephemeral: bool,
        away: Duration,
    ) -> Self {
        let window = Self::window(ephemeral);
        Self {
            inner,
            name: name.to_owned(),
            ephemeral,
            deadline: Mutex::new(Some(Instant::now() + window.saturating_sub(away))),
            gave_up: Arc::default(),
        }
    }

    /// How long a box may stay unreachable.
    fn window(ephemeral: bool) -> Duration {
        if ephemeral {
            EPHEMERAL_RETRY_FOR
        } else {
            RETRY_FOR
        }
    }

    /// Set once the link stopped trying because the box stayed away.
    pub(super) fn gave_up(&self) -> Arc<AtomicBool> {
        self.gave_up.clone()
    }
}

#[async_trait]
impl Transport for PeerAttempts {
    /// Dials through the inner transport and applies the bounds.
    async fn connect(&self) -> Result<BoxConnection, TransportError> {
        let error = match self.inner.connect().await {
            Ok(connection) => {
                *lock(&self.deadline) = None;
                return Ok(connection);
            }
            Err(error) => error,
        };
        let name = self.name.clone();
        if matches!(error.downcast_ref(), Some(PeerError::Refused)) {
            return Err(TransportError::permanent(PeerLinkError::Refused { name }));
        }
        if error.is_permanent() {
            return Err(error);
        }
        let now = Instant::now();
        // The first failure after a connection starts the window: the box
        // was reached until about then.
        let deadline = *lock(&self.deadline).get_or_insert(now + Self::window(self.ephemeral));
        if now < deadline {
            return Err(error);
        }
        self.gave_up.store(true, Ordering::SeqCst);
        tracing::info!(host = %self.name, %error, "stopped trying a box that stayed unreachable");
        Err(TransportError::permanent(if self.ephemeral {
            PeerLinkError::GaveUpEphemeral { name }
        } else {
            PeerLinkError::GaveUp { name }
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use devlanes_transport::{
        Admission, PeerId,
        memory::{MemoryPeers, MemoryTransport, memory_transport},
    };

    use super::*;

    /// Lets every peer in, for both ALPNs.
    struct AdmitAll;

    impl Admission for AdmitAll {
        /// Always true.
        fn admit(&self, _: &PeerId, _: Alpn) -> bool {
            true
        }
    }

    /// How a test session ends.
    #[derive(Debug, Clone, Copy)]
    enum Ending {
        /// The session asked why the connection failed.
        AskedWhy,
        /// The session waited for the close.
        AwaitedClose,
        /// The session only closed its side, as after a failed ping write.
        ShutDown,
    }

    /// A box that refused this machine's key and stopped listening fails
    /// the next dial as refused, for good, whichever way the session saw
    /// the close; a box that only stopped listening is unreachable.
    #[tokio::test]
    async fn a_refusal_on_the_last_connection_refuses_the_next_dial() {
        let rows = [
            (Ending::AskedWhy, true, true),
            (Ending::AwaitedClose, true, true),
            (Ending::ShutDown, true, true),
            (Ending::ShutDown, false, false),
        ];
        for (ending, revoked, refused) in rows {
            let peers = MemoryPeers::new();
            let (listener, dialer) = (peers.endpoint(), peers.endpoint());
            listener
                .listen(Some(Arc::new(AdmitAll)))
                .await
                .expect("listen");
            let addr = listener.addr().await.expect("addr");
            let transport = PeerTransport::new(dialer.clone(), addr);
            let connection = transport.connect().await.expect("connect");
            if revoked {
                listener.close_peer(&dialer.id().await.expect("id"));
            }
            listener.listen(None).await.expect("stop listening");
            let closed = tokio::time::timeout(Duration::from_secs(5), async {
                match ending {
                    Ending::AskedWhy => {
                        connection.failure_within(Duration::from_secs(5)).await;
                    }
                    Ending::AwaitedClose => connection.closed().await,
                    // Polls the connection through a call that records
                    // nothing until it is gone, then only shuts it down.
                    Ending::ShutDown => {
                        if revoked {
                            while connection.open().await.is_ok() {
                                tokio::task::yield_now().await;
                            }
                        }
                        connection.shutdown();
                    }
                }
            })
            .await;
            assert!(closed.is_ok(), "{ending:?}: the connection never closed");
            let Err(error) = transport.connect().await else {
                panic!("{ending:?}: dialed a box that stopped listening");
            };
            let was_refused = matches!(error.downcast_ref(), Some(PeerError::Refused));
            assert_eq!(was_refused, refused, "{ending:?} {revoked}: {error}");
            assert_eq!(
                error.is_permanent(),
                refused,
                "{ending:?} {revoked}: {error}"
            );
        }
    }

    /// What a scripted dial does: connect for `None`, else fail with the
    /// error the function makes.
    type Outcome = Option<fn() -> TransportError>;

    /// A transport that answers each dial with the next queued outcome.
    struct Scripted {
        outcomes: Mutex<VecDeque<Outcome>>,
        memory: MemoryTransport,
    }

    #[async_trait]
    impl Transport for Scripted {
        /// The next outcome.
        async fn connect(&self) -> Result<BoxConnection, TransportError> {
            let outcome = lock(&self.outcomes).pop_front().flatten();
            match outcome {
                Some(error) => Err(error()),
                None => self.memory.connect().await,
            }
        }
    }

    /// A dial that finds no listener.
    fn unreachable() -> TransportError {
        PeerError::Unreachable("no endpoint with that id is listening".into()).into()
    }

    /// A dial the box refused.
    fn refused() -> TransportError {
        PeerError::Refused.into()
    }

    /// Failures stay transient until the window since the box was last
    /// reached is over, and the first one after it is permanent with the
    /// give-up text and sets `gave_up`; a refused key is permanent at once
    /// and gives up nothing.
    #[tokio::test(start_paused = true)]
    async fn attempts_give_up_once_the_box_stayed_away_too_long() {
        let hour = Duration::from_secs(60 * 60);
        let second = Duration::from_secs(1);
        type Row = (
            bool,
            Duration,
            Duration,
            fn() -> TransportError,
            Option<&'static str>,
            bool,
        );
        let rows: [Row; 6] = [
            // (ephemeral, away at start, then waited, dial, permanent text, gave up)
            (false, Duration::ZERO, 23 * hour, unreachable, None, false),
            (false, 23 * hour, hour - second, unreachable, None, false),
            (
                false,
                23 * hour,
                hour,
                unreachable,
                Some("for 24 hours"),
                true,
            ),
            (
                false,
                25 * hour,
                Duration::ZERO,
                unreachable,
                Some("for 24 hours"),
                true,
            ),
            (
                true,
                2 * hour,
                Duration::ZERO,
                unreachable,
                Some("forgot it"),
                true,
            ),
            (
                false,
                Duration::ZERO,
                Duration::ZERO,
                refused,
                Some("no longer accepts"),
                false,
            ),
        ];
        let (memory, _listener) = memory_transport();
        for (ephemeral, away, waited, dial, permanent, gave_up) in rows {
            let inner = Arc::new(Scripted {
                outcomes: Mutex::new(VecDeque::from([Some(dial)])),
                memory: memory.clone(),
            });
            let attempts = PeerAttempts::new(inner, "acme", ephemeral, away);
            tokio::time::advance(waited).await;
            let Err(error) = attempts.connect().await else {
                panic!("connected");
            };
            let row = format!("{ephemeral} {away:?} {waited:?}: {error}");
            assert_eq!(error.is_permanent(), permanent.is_some(), "{row}");
            if let Some(text) = permanent {
                assert!(error.to_string().contains(text), "{row}");
            }
            assert_eq!(attempts.gave_up().load(Ordering::SeqCst), gave_up, "{row}");
        }
    }

    /// A connection moves the deadline: a box reached 23 hours into its
    /// window gets a full day from its first failure after that.
    #[tokio::test(start_paused = true)]
    async fn a_connection_restarts_the_window() {
        let hour = Duration::from_secs(60 * 60);
        let (memory, _listener) = memory_transport();
        let inner = Arc::new(Scripted {
            outcomes: Mutex::new(VecDeque::from([
                None,
                Some(unreachable as fn() -> TransportError),
                Some(unreachable),
                Some(unreachable),
            ])),
            memory,
        });
        let attempts = PeerAttempts::new(inner, "acme", false, 23 * hour);
        assert!(attempts.connect().await.is_ok(), "the box answered");
        tokio::time::advance(2 * hour).await;
        let first = attempts.connect().await.err().expect("unreachable");
        assert!(!first.is_permanent(), "{first}");
        tokio::time::advance(23 * hour).await;
        let later = attempts.connect().await.err().expect("unreachable");
        assert!(
            !later.is_permanent(),
            "23 hours after the first failure: {later}"
        );
        tokio::time::advance(hour).await;
        let last = attempts.connect().await.err().expect("unreachable");
        assert!(last.is_permanent(), "a day after the first failure: {last}");
    }
}
