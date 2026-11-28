//! What the session and the box need from a connection, and what the daemon
//! needs from a peer endpoint, checked against the traits only. Each check
//! is an `async fn` taking a [`Pair`] or [`Peers`] and failing with an error
//! or a panic; a transport's tests build a fresh fixture for each check and
//! bound it with a timeout, since a broken transport often waits forever
//! instead of failing.
//!
//! Built with the `test-support` feature, so transports in other crates run
//! the same checks as this crate's yamux connections and memory peers.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use crate::peer::{Admission, Alpn, PeerEndpoint, PeerError, PeerId, PeerIncoming};
use crate::session::MAX_STREAMS;
use crate::transport::{
    BoxConnection, BoxDuplex, BoxError, BoxServerConnection, ConnectionError, TransportError,
};

/// Outcome of one check.
pub type CheckResult = Result<(), BoxError>;

/// One client and box connection of a transport under test, and a way to
/// break the link under them.
pub struct Pair {
    /// The client's side.
    pub client: BoxConnection,
    /// The box's side of the same connection.
    pub server: BoxServerConnection,
    /// Breaks the link the way a network failure would: both sides close,
    /// and the client gets a failure reason.
    pub sever: Box<dyn Fn() + Send + Sync>,
}

/// Reads exactly `expected` from `stream`.
async fn expect(stream: &mut BoxDuplex, expected: &[u8]) -> CheckResult {
    let mut read = vec![0; expected.len()];
    stream.read_exact(&mut read).await?;
    assert_eq!(read, expected);
    Ok(())
}

/// Reads `stream` to its end.
async fn rest(stream: &mut BoxDuplex) -> Result<Vec<u8>, BoxError> {
    let mut read = Vec::new();
    stream.read_to_end(&mut read).await?;
    Ok(read)
}

/// A payload larger than one mux frame, with a pattern a lost or reordered
/// byte breaks.
fn payload() -> Vec<u8> {
    (0..251u8).cycle().take(256 * 1024).collect()
}

/// Opens a stream and announces it with `hello`, as every protocol stream
/// starts, then accepts it on the box and reads the hello.
async fn opened(pair: &mut Pair, hello: &[u8]) -> Result<(BoxDuplex, BoxDuplex), BoxError> {
    let mut client = pair.client.open().await?;
    client.write_all(hello).await?;
    let mut boxed = pair.server.accept().await.ok_or("no stream")?;
    expect(&mut boxed, hello).await?;
    Ok((client, boxed))
}

/// Each side's `last_inbound` moves when the peer's bytes arrive, and only
/// then: not with this side's own writes, and not to "now" on every call.
/// No sleep separates the marks: tokio's clock never goes back, and every
/// frame yamux sends on its own at the start (its first ping and pong) is on
/// the pipe before the box's first data, so nothing else arrives in between.
/// Over QUIC only stream data counts, so keep-alives and ACKs arriving in
/// between cannot move the mark either.
pub async fn last_inbound_moves_with_what_the_peer_sends(mut pair: Pair) -> CheckResult {
    let (mut client, mut boxed) = opened(&mut pair, b"open").await?;
    let mark = Instant::now();
    boxed.write_all(b"from the box").await?;
    expect(&mut client, b"from the box").await?;
    assert!(
        pair.client.last_inbound() >= mark,
        "the box's bytes left no mark"
    );
    let after = Instant::now();
    client.write_all(b"from the client").await?;
    expect(&mut boxed, b"from the client").await?;
    assert!(
        pair.client.last_inbound() <= after,
        "the client's own write counted as inbound"
    );
    assert!(
        pair.server.last_inbound() >= after,
        "the client's bytes left no mark"
    );
    Ok(())
}

/// The side that shuts its write half down first still reads the other's
/// reply, whichever side that is: a request followed by its end, as a
/// mirrored connection forwards it.
pub async fn a_half_closed_stream_keeps_the_other_direction(mut pair: Pair) -> CheckResult {
    for client_first in [true, false] {
        let (mut client, mut boxed) = opened(&mut pair, b"open").await?;
        let (first, second) = if client_first {
            (&mut client, &mut boxed)
        } else {
            (&mut boxed, &mut client)
        };
        first.write_all(b"request").await?;
        first.shutdown().await?;
        assert_eq!(
            rest(second).await?,
            b"request",
            "client first: {client_first}"
        );
        second.write_all(b"reply").await?;
        second.shutdown().await?;
        assert_eq!(rest(first).await?, b"reply", "client first: {client_first}");
    }
    Ok(())
}

/// A stream dropped without a shutdown ends cleanly for the reader after
/// every byte written before, whichever side drops it.
pub async fn a_dropped_stream_ends_cleanly_after_its_bytes(mut pair: Pair) -> CheckResult {
    let payload = payload();
    for client_drops in [true, false] {
        let (client, boxed) = opened(&mut pair, b"open").await?;
        let (mut writer, mut reader) = if client_drops {
            (client, boxed)
        } else {
            (boxed, client)
        };
        let sent = payload.clone();
        let writing = tokio::spawn(async move {
            writer.write_all(&sent).await?;
            drop(writer);
            Ok::<_, std::io::Error>(())
        });
        let read = rest(&mut reader).await?;
        writing.await??;
        assert!(
            read == payload,
            "client drops: {client_drops}: {} bytes",
            read.len()
        );
    }
    Ok(())
}

/// [`MAX_STREAMS`] streams stay open at once without an open waiting on the
/// box to close one.
pub async fn max_streams_stay_open_at_once(pair: Pair) -> CheckResult {
    let mut server = pair.server;
    // Like the real box, answer every stream: yamux stops opening streams
    // while too many wait for their first reply.
    let answering = tokio::spawn(async move {
        let mut held = Vec::new();
        while held.len() < MAX_STREAMS {
            let mut stream = server.accept().await.ok_or("the connection closed")?;
            expect(&mut stream, b"o").await?;
            stream.write_all(b"k").await?;
            held.push(stream);
        }
        Ok::<_, BoxError>((server, held))
    });
    let mut open = Vec::new();
    for _ in 0..MAX_STREAMS {
        let mut stream = pair.client.open().await?;
        stream.write_all(b"o").await?;
        open.push(stream);
    }
    for stream in &mut open {
        expect(stream, b"k").await?;
    }
    let _held = answering.await??;
    Ok(())
}

/// Streams opened and dropped one after another next to `MAX_STREAMS - 1`
/// held ones never wait in `open`: the transport frees dropped streams fast
/// enough for its headroom, as the session expects when it frees a slot and
/// opens again at once.
pub async fn open_never_waits_while_streams_churn_at_the_limit(pair: Pair) -> CheckResult {
    const HELD: usize = MAX_STREAMS - 1;
    const CYCLES: usize = 128;
    // A transport that runs out of stream credit waits in `open` forever, so
    // the bound only names the failing cycle; it is generous because a
    // loaded CI runner may deschedule the mux driver for a while.
    const OPEN_WAIT: Duration = Duration::from_secs(5);
    let mut server = pair.server;
    // The box answers every stream, holds the first `HELD` and drops each
    // later one right after its answer.
    let answering = tokio::spawn(async move {
        let mut held = Vec::new();
        for index in 0..HELD + CYCLES {
            let mut stream = server.accept().await.ok_or("the connection closed")?;
            expect(&mut stream, b"o").await?;
            stream.write_all(b"k").await?;
            if index < HELD {
                held.push(stream);
            }
        }
        Ok::<_, BoxError>((server, held))
    });
    let mut open = Vec::new();
    for _ in 0..HELD {
        let mut stream = pair.client.open().await?;
        stream.write_all(b"o").await?;
        open.push(stream);
    }
    for stream in &mut open {
        expect(stream, b"k").await?;
    }
    for cycle in 0..CYCLES {
        let Ok(opened) = tokio::time::timeout(OPEN_WAIT, pair.client.open()).await else {
            return Err(format!("open {cycle} waited with {HELD} streams held").into());
        };
        let mut stream = opened?;
        stream.write_all(b"o").await?;
        expect(&mut stream, b"k").await?;
    }
    let _held = answering.await??;
    Ok(())
}

/// The stream the client opened and wrote first is the first one the box
/// accepts, which is how the box finds the control stream.
pub async fn the_first_stream_written_is_accepted_first(mut pair: Pair) -> CheckResult {
    let mut first = pair.client.open().await?;
    first.write_all(b"first").await?;
    let mut second = pair.client.open().await?;
    second.write_all(b"other").await?;
    let mut accepted = pair.server.accept().await.ok_or("no stream")?;
    expect(&mut accepted, b"first").await?;
    Ok(())
}

/// A message the box wrote and shut down before it dropped the connection
/// reaches the client, as a refusal does; a clean close has no failure
/// reason, and the closed connection opens no more streams.
pub async fn finished_bytes_survive_the_box_dropping_the_connection(mut pair: Pair) -> CheckResult {
    let (mut client, mut boxed) = opened(&mut pair, b"hello").await?;
    let message = br#"{"type":"refused","message":"the box does not accept connections"}"#;
    boxed.write_all(message).await?;
    boxed.shutdown().await?;
    drop(boxed);
    drop(pair.server);
    assert_eq!(rest(&mut client).await?, message);
    pair.client.closed().await;
    let failure = pair.client.failure_within(Duration::from_secs(1)).await;
    assert_eq!(failure, None);
    assert!(
        matches!(pair.client.open().await, Err(ConnectionError::Closed)),
        "a stream opened after the box closed the connection"
    );
    Ok(())
}

/// After the client shuts the connection down, it has no failure reason,
/// opens nothing, and the box accepts nothing.
pub async fn a_client_shutdown_ends_the_connection(mut pair: Pair) -> CheckResult {
    pair.client.shutdown();
    pair.client.closed().await;
    let failure = pair.client.failure_within(Duration::from_secs(1)).await;
    assert_eq!(failure, None);
    assert!(
        matches!(pair.client.open().await, Err(ConnectionError::Closed)),
        "a stream opened on a closed connection"
    );
    assert!(
        pair.server.accept().await.is_none(),
        "the box accepted a stream"
    );
    Ok(())
}

/// A link that breaks under the connection closes both sides, and the
/// client has a reason to show.
pub async fn a_broken_link_reports_why(mut pair: Pair) -> CheckResult {
    (pair.sever)();
    pair.client.closed().await;
    let failure = pair.client.failure_within(Duration::from_secs(1)).await;
    assert!(failure.is_some(), "no reason for a broken link");
    assert!(
        pair.server.accept().await.is_none(),
        "the box accepted a stream"
    );
    Ok(())
}

/// Two dialing endpoints and the endpoint they dial, none of them listening
/// yet, of a peer transport under test.
pub struct Peers {
    /// Endpoints that dial `listener`, with different ids.
    pub dialers: [Arc<dyn PeerEndpoint>; 2],
    /// The endpoint the checks make listen.
    pub listener: Arc<dyn PeerEndpoint>,
}

/// An admission that lets in the peers in its set, each for the protocols
/// listed with it, as the daemon lets in pinned keys for sessions and any key
/// for pairing while a ticket is outstanding.
struct Allowed(Mutex<HashSet<(PeerId, Alpn)>>);

impl Allowed {
    /// An admission for `peers`, shared with the check that changes it.
    fn new(peers: impl IntoIterator<Item = (PeerId, Alpn)>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(peers.into_iter().collect())))
    }

    /// An admission for `peers` for sessions only.
    fn sessions(peers: impl IntoIterator<Item = PeerId>) -> Arc<Self> {
        Self::new(peers.into_iter().map(|peer| (peer, Alpn::Tunnel)))
    }

    /// Stops admitting `peer` for anything, as revoking a client does.
    fn remove(&self, peer: &PeerId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(allowed, _)| allowed != peer);
    }
}

impl Admission for Allowed {
    /// Whether `peer` is in the set for `alpn`.
    fn admit(&self, peer: &PeerId, alpn: Alpn) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(*peer, alpn))
    }
}

/// Dials `listener` from `dialer` for `alpn`.
async fn dial(
    dialer: &dyn PeerEndpoint,
    listener: &dyn PeerEndpoint,
    alpn: Alpn,
) -> Result<BoxConnection, TransportError> {
    let addr = listener.addr().await.map_err(TransportError::from)?;
    dialer.connect(&addr, alpn).await
}

/// The next connection `listener` accepts, which must come from `from` for
/// `alpn`.
async fn accepted(
    listener: &dyn PeerEndpoint,
    from: &dyn PeerEndpoint,
    alpn: Alpn,
) -> Result<PeerIncoming, BoxError> {
    let incoming = listener.accept().await.ok_or("the listener closed")?;
    assert_eq!(
        incoming.peer,
        from.id().await?,
        "accepted a connection from another peer"
    );
    assert_eq!(
        incoming.alpn, alpn,
        "accepted a connection for another protocol"
    );
    Ok(incoming)
}

/// Opens a stream on `client`, accepts it on `server` and checks bytes go
/// both ways.
async fn exchanged(
    client: &BoxConnection,
    server: &mut BoxServerConnection,
) -> Result<(BoxDuplex, BoxDuplex), BoxError> {
    let mut stream = client.open().await?;
    stream.write_all(b"open").await?;
    let mut boxed = server.accept().await.ok_or("no stream")?;
    expect(&mut boxed, b"open").await?;
    talk(&mut stream, &mut boxed).await?;
    Ok((stream, boxed))
}

/// Sends a line each way on an open stream.
async fn talk(stream: &mut BoxDuplex, boxed: &mut BoxDuplex) -> CheckResult {
    stream.write_all(b"ping").await?;
    expect(boxed, b"ping").await?;
    boxed.write_all(b"pong").await?;
    expect(stream, b"pong").await
}

/// Asserts that a dial failed in the given class, for a cause `reason` accepts.
fn assert_failed_with(
    result: Result<BoxConnection, TransportError>,
    permanent: bool,
    reason: impl Fn(&PeerError) -> bool,
) {
    let Err(error) = result else {
        panic!("the dial succeeded");
    };
    assert_eq!(error.is_permanent(), permanent, "{error}");
    let cause = error.downcast_ref::<PeerError>();
    assert!(cause.is_some_and(reason), "{error}");
}

/// The admission sees the protocol a peer dials for: a key admitted for
/// pairing only is refused for a session, and its pairing dial arrives as
/// one. A key the admission does not know is refused during the handshake
/// as a permanent error, and the box never sees either refused connection:
/// the next one it accepts is the pairing dial made afterwards.
pub async fn an_unadmitted_peer_is_refused_before_any_stream(peers: Peers) -> CheckResult {
    let [pairing, stranger] = &peers.dialers;
    let listener = &*peers.listener;
    listener
        .listen(Some(Allowed::new([(pairing.id().await?, Alpn::Pair)])))
        .await?;
    let refused = |cause: &PeerError| matches!(cause, PeerError::Refused);
    assert_failed_with(
        dial(&**stranger, listener, Alpn::Tunnel).await,
        true,
        refused,
    );
    assert_failed_with(
        dial(&**pairing, listener, Alpn::Tunnel).await,
        true,
        refused,
    );
    let _client = dial(&**pairing, listener, Alpn::Pair).await?;
    let _server = accepted(listener, &**pairing, Alpn::Pair).await?;
    Ok(())
}

/// Closing one peer ends its connection with the refused-key reason and,
/// once the admission drops it, refuses its next dial, while another peer's
/// connection keeps working.
pub async fn close_peer_ends_only_that_peers_connections(peers: Peers) -> CheckResult {
    let [closed, kept] = &peers.dialers;
    let listener = &*peers.listener;
    let closed_id = closed.id().await?;
    let admission = Allowed::sessions([closed_id, kept.id().await?]);
    listener.listen(Some(admission.clone())).await?;
    let closed_client = dial(&**closed, listener, Alpn::Tunnel).await?;
    let mut closed_server = accepted(listener, &**closed, Alpn::Tunnel)
        .await?
        .connection;
    let kept_client = dial(&**kept, listener, Alpn::Tunnel).await?;
    let mut kept_server = accepted(listener, &**kept, Alpn::Tunnel).await?.connection;
    let (mut kept_stream, mut kept_boxed) = exchanged(&kept_client, &mut kept_server).await?;

    admission.remove(&closed_id);
    listener.close_peer(&closed_id);
    closed_client.closed().await;
    let failure = closed_client
        .failure_within(Duration::from_secs(1))
        .await
        .unwrap_or_default();
    assert!(
        failure.contains(&PeerError::Refused.to_string()),
        "the closed peer's reason: {failure:?}"
    );
    assert!(
        closed_server.accept().await.is_none(),
        "the box accepted a stream from the closed peer"
    );
    assert_failed_with(
        dial(&**closed, listener, Alpn::Tunnel).await,
        true,
        |cause| matches!(cause, PeerError::Refused),
    );

    talk(&mut kept_stream, &mut kept_boxed).await
}

/// A listener that stopped refuses new dials as a transient failure, since
/// it may listen again, and admits again once it does. The connection made
/// before keeps carrying bytes both ways through both changes, as a paired
/// client's session does when the box mints a ticket or one expires.
pub async fn a_listener_that_stopped_accepts_nothing(peers: Peers) -> CheckResult {
    let [dialer, _] = &peers.dialers;
    let listener = &*peers.listener;
    let admission = Allowed::sessions([dialer.id().await?]);
    listener.listen(Some(admission.clone())).await?;
    let client = dial(&**dialer, listener, Alpn::Tunnel).await?;
    let mut server = accepted(listener, &**dialer, Alpn::Tunnel)
        .await?
        .connection;
    let (mut stream, mut boxed) = exchanged(&client, &mut server).await?;

    listener.listen(None).await?;
    assert_failed_with(
        dial(&**dialer, listener, Alpn::Tunnel).await,
        false,
        |cause| matches!(cause, PeerError::Unreachable(_)),
    );
    talk(&mut stream, &mut boxed).await?;

    listener.listen(Some(admission)).await?;
    let _again = dial(&**dialer, listener, Alpn::Tunnel).await?;
    let _server = accepted(listener, &**dialer, Alpn::Tunnel).await?;
    talk(&mut stream, &mut boxed).await
}

/// Closing an endpoint ends its connections as a clean close, with no
/// failure reason for the client, ends `accept`, and fails everything that
/// needs the network with [`PeerError::Closed`], while `id` keeps working.
pub async fn a_closed_endpoint_ends_its_connections_cleanly(peers: Peers) -> CheckResult {
    let [dialer, _] = &peers.dialers;
    let listener = &*peers.listener;
    let id = listener.id().await?;
    listener
        .listen(Some(Allowed::sessions([dialer.id().await?])))
        .await?;
    let client = dial(&**dialer, listener, Alpn::Tunnel).await?;
    let mut server = accepted(listener, &**dialer, Alpn::Tunnel)
        .await?
        .connection;
    let _streams = exchanged(&client, &mut server).await?;
    let dialer_addr = dialer.addr().await?;

    listener.close().await;
    client.closed().await;
    assert_eq!(client.failure_within(Duration::from_secs(1)).await, None);
    assert!(listener.accept().await.is_none(), "accepted after close");
    let closed = |cause: &PeerError| matches!(cause, PeerError::Closed);
    assert!(matches!(listener.addr().await, Err(PeerError::Closed)));
    assert!(matches!(
        listener.listen(None).await,
        Err(PeerError::Closed)
    ));
    assert_failed_with(
        listener.connect(&dialer_addr, Alpn::Tunnel).await,
        false,
        closed,
    );
    assert_eq!(listener.id().await?, id);
    Ok(())
}
