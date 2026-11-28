//! The daemon's one iroh endpoint: bound on first use, listening while the
//! daemon asks it to, and dialing boxes over direct paths only.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{
    Admission, Alpn, BoxConnection, PAIR_ALPN, PeerAddr, PeerEndpoint, PeerError, PeerId,
    PeerIncoming, TUNNEL_ALPN, TransportError,
};
use futures_util::{FutureExt, Stream, StreamExt};
use iroh::address_lookup::{
    AddressLookup, DnsAddressLookup, EndpointData, EndpointInfo, Error as LookupError,
    Item as LookupItem, PkarrPublisher, PkarrResolver,
};
use iroh::endpoint::{
    self, AfterHandshakeOutcome, EndpointHooks, PortmapperConfig, QuicTransportConfig, Side,
    VarInt, WeakConnectionHandle, presets,
};
use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher,
};
use tokio::sync::{OnceCell, mpsc};

use crate::connection::{IrohConnection, IrohServerConnection, NO_DIRECT_PATH, REFUSED};
use crate::key;
use crate::ports;
use crate::text::peer_text;

/// How long a dial waits for a direct path: three of iroh's 5 s hole-punch
/// rounds. Locally a direct path took 2 to 31 ms.
const DIRECT_PATH_WAIT: Duration = Duration::from_secs(15);

/// How long `addr` waits for the relay connection before it names the first
/// configured relay instead of the home relay iroh picked.
const ONLINE_WAIT: Duration = Duration::from_secs(5);

/// Admitted session connections waiting for `accept`. A full queue drops new
/// ones, so whatever connects cannot grow the daemon's memory.
const TUNNEL_QUEUE: usize = 64;

/// Admitted pairing connections waiting for `accept`, in a queue of their
/// own: pairing admits any key while a ticket is outstanding, and a flood of
/// those must not push paired clients' sessions out.
const PAIR_QUEUE: usize = 16;

/// Socket addresses a ticket carries at most.
const MAX_DIRECT_ADDRS: usize = 8;

/// Streams a client may have open at once on a box: the session holds 512
/// (`MAX_STREAMS`), and noq returns stream credit only after more than 1/8 of
/// the limit is freed, so the limit must exceed 512 x 8/7 = 586 or an open
/// waits on streams the session already dropped. 640 passed 300 churn cycles
/// in the spike; 576 blocked at cycle 65.
const BOX_STREAMS: u32 = 640;

/// Streams a peer may have open at once on a pairing connection: the
/// exchange is one request and one answer on one stream.
const PAIR_STREAMS: u32 = 1;

/// Unread data a single stream may hold: 4 MiB reaches 40 MiB/s at 100 ms
/// round trip, the ceiling yamux's 4 MiB stream window gives over SSH.
const STREAM_WINDOW: u32 = 4 * 1024 * 1024;

/// Unread data a whole connection may hold whatever its stream count. noq's
/// default is unbounded, and 640 streams of 4 MiB would be 2.5 GiB.
const CONNECTION_WINDOW: u32 = 16 * 1024 * 1024;

/// Unacknowledged data a connection may have in flight: the peer's receive
/// window, since nothing more can be in flight anyway.
const SEND_WINDOW: u64 = 16 * 1024 * 1024;

/// noq's default idle timeout of 30 s in milliseconds, written down: the
/// client session's ping timeout (20 s after a ping) and the box's idle limit
/// (30 s) fire first or together, as over SSH.
const IDLE_TIMEOUT_MS: u32 = 30_000;

/// iroh's own keep-alive interval, written down: it keeps NAT bindings open.
/// Keep-alives never count as activity for the session's ping timeout.
const KEEP_ALIVE: Duration = Duration::from_secs(5);

/// How this machine's endpoint reaches other peers.
#[derive(Debug, Clone)]
pub struct IrohOptions {
    /// The secret key, created on first use and kept private to the user.
    /// While the endpoint listens, the UDP ports it bound are kept next to
    /// it in `p2p-port`, so the next start binds the same ones and stored
    /// addresses still reach it; `listen(None)` removes the file. An
    /// endpoint that only dials keeps none.
    pub key_file: PathBuf,
    /// Relay URLs such as `https://relay1.devlanes.run`. Empty for none: then
    /// peers connect only through the addresses they know of each other.
    /// They are also the only relays a dial uses: a relay from a `PeerAddr`
    /// or from a box's address record that is not in this list is dropped,
    /// and the dial tries the direct addresses alone. The daemon's config
    /// validates them; one that does not parse here fails the bind.
    pub relays: Vec<String>,
    /// Publish this endpoint's relay to n0's DNS while listening, and look
    /// peers up there when dialing. Its own option because iroh asks every
    /// lookup service on each dial, so tests and machines that must stay off
    /// third-party services turn it off while still using a relay. Ignored
    /// without relays, since the published record holds only the relay URL.
    pub address_lookup: bool,
    /// Ask the router for a port mapping (UPnP, NAT-PMP, PCP). Read when the
    /// endpoint binds.
    pub port_mapping: bool,
}

/// This machine's peer-to-peer endpoint over iroh.
///
/// Needs a tokio runtime, as every async part of this crate does: its
/// methods spawn the bind, the accept loop and blocking file work there.
pub struct IrohEndpoint {
    inner: Arc<Inner>,
}

/// What an [`IrohEndpoint`] is, shared with the task that binds it, so a
/// caller that stops waiting does not stop the bind.
struct Inner {
    options: IrohOptions,
    /// `options.relays` parsed: the only relays a dial or a lookup result
    /// may lead to.
    own_relays: Arc<[RelayUrl]>,
    key: OnceCell<SecretKey>,
    /// Whether the endpoint is bound, binding or being released. Every
    /// change goes through this lock, which is what keeps a call that
    /// arrives as the idle task releases from getting a closed endpoint.
    phase: Mutex<Phase>,
    /// Calls between getting the endpoint and returning, each held by an
    /// [`InUse`]: a call still waiting for an address or a handshake keeps
    /// the endpoint as an open connection does. Changed only under `phase`,
    /// which the release decision holds too.
    calls: AtomicUsize,
    shared: Arc<Shared>,
    /// Where the accept task queues admitted connections; `None` once
    /// closed, which ends `accept` after the queues drain.
    queues: Mutex<Option<Queues>>,
    incoming: tokio::sync::Mutex<Incoming>,
    /// Held for the whole of `listen`, so concurrent calls apply in turn;
    /// holds whether a publisher is registered, so it is added once.
    listening: tokio::sync::Mutex<bool>,
    /// Held while `p2p-port` is written or removed, by the tasks `listen`
    /// starts for it, so a late write cannot follow a removal.
    port_file: tokio::sync::Mutex<()>,
    /// Those tasks, so `close` returns only after the file is as the last
    /// `listen` left it.
    port_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// How long the endpoint may go unused before it is released:
    /// [`IDLE_RELEASE`], shorter only in this module's tests.
    idle: Duration,
}

/// The sending halves of the accept queues, one per protocol.
#[derive(Clone)]
struct Queues {
    tunnel: mpsc::Sender<PeerIncoming>,
    pair: mpsc::Sender<PeerIncoming>,
}

impl Queues {
    /// Queues an admitted connection, or drops it when its queue is full.
    fn push(&self, arrived: PeerIncoming) {
        let queue = match arrived.alpn {
            Alpn::Tunnel => &self.tunnel,
            Alpn::Pair => &self.pair,
        };
        let peer = arrived.peer.short();
        if queue.try_send(arrived).is_err() {
            tracing::debug!(%peer, "dropping a peer connection: too many waiting");
        }
    }
}

/// The receiving halves of the accept queues.
struct Incoming {
    tunnel: mpsc::Receiver<PeerIncoming>,
    pair: mpsc::Receiver<PeerIncoming>,
}

/// The bound iroh endpoint and its accept task.
struct Bound {
    endpoint: Endpoint,
    accepting: tokio::task::JoinHandle<()>,
    /// Until when `addr` may wait for the relay and the first local
    /// addresses: once per bind, so a relay that does not answer costs the
    /// first ticket the wait and no later one.
    settled_by: tokio::time::Instant,
    /// When a call last needed the endpoint or ended, for the idle release.
    /// Set when the bind made it, and after that written only under
    /// `Inner::phase`.
    last_use: Mutex<tokio::time::Instant>,
}

impl Bound {
    /// When a call last needed the endpoint.
    fn last_use(&self) -> tokio::time::Instant {
        *self.last_use.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records that a call needs the endpoint now.
    fn touch(&self) {
        *self.last_use.lock().unwrap_or_else(PoisonError::into_inner) = tokio::time::Instant::now();
    }
}

/// A call to the endpoint in progress, counted in `Inner::calls` from the
/// moment it asks for the endpoint until it is dropped, also when its caller
/// stops waiting.
struct InUse {
    inner: Arc<Inner>,
}

impl InUse {
    /// Counts a call; the caller holds `inner`'s phase lock.
    fn start(inner: &Arc<Inner>) -> Self {
        inner.calls.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: inner.clone(),
        }
    }
}

impl Drop for InUse {
    /// Ends the call and marks the endpoint used now, so the idle time
    /// starts when the call ended.
    fn drop(&mut self) {
        let phase = self.inner.phase();
        self.inner.calls.fetch_sub(1, Ordering::Relaxed);
        if let Phase::Bound(bound) = &*phase {
            bound.touch();
        }
    }
}

/// Where the endpoint is in its life between binds.
enum Phase {
    /// Never bound, or released after it went idle.
    Unbound,
    /// A bind in progress, which every caller that needs the endpoint awaits.
    Binding(BindTask),
    /// Bound and in use, or idle for less than [`IDLE_RELEASE`].
    Bound(Arc<Bound>),
    /// Released and closing; a bind that starts meanwhile waits for it, so
    /// it finds the ports free again.
    Releasing(ReleaseTask),
    /// `close` ran: nothing binds again.
    Closed,
}

/// The bind in progress, shared by everyone waiting for it. Its error is
/// shared as text with its class: `PeerError` cannot be cloned.
type BindTask = futures_util::future::Shared<
    futures_util::future::BoxFuture<'static, Result<Arc<Bound>, SharedBindError>>,
>;

/// A release in progress: the iroh endpoint closing.
type ReleaseTask = futures_util::future::Shared<futures_util::future::BoxFuture<'static, ()>>;

/// How long a bound endpoint that does not listen and has no open
/// connection may go unused before it is released: its two sockets closed,
/// its relay connection and network checks stopped, its router mapping
/// given up, until the next call binds again. Long enough that a person who
/// pairs, fails and tries again within a few minutes pays one bind, short
/// enough that an endpoint used once for a ticket does not sit on the relay
/// for the daemon's life. A client retrying a box dials at least every 2
/// minutes, and a box with clients listens, so neither is ever released.
const IDLE_RELEASE: Duration = Duration::from_secs(5 * 60);

/// How often a bind right after a release tries the saved ports while the
/// old sockets are still being freed.
const REBIND_TRIES: u32 = 20;

/// The pause between those tries: 20 of them wait about 1 s in all.
const REBIND_PAUSE: Duration = Duration::from_millis(50);

/// How soon the idle task looks again at an endpoint that is listening or
/// has open connections, which could go idle any time; sooner when the idle
/// time itself is shorter.
const BUSY_RECHECK: Duration = Duration::from_secs(60);

/// What the idle task does next with a bound endpoint.
#[derive(Debug, PartialEq, Eq)]
enum IdleStep {
    /// Release it now.
    Release,
    /// Look again at this time.
    CheckAt(tokio::time::Instant),
}

/// The idle rule: release only an endpoint that does not listen, has no
/// open connection and no call in progress, and has not been needed for
/// `idle` ([`IDLE_RELEASE`]).
fn idle_step(
    idle: Duration,
    listening: bool,
    open_connections: usize,
    calls: usize,
    last_use: tokio::time::Instant,
    now: tokio::time::Instant,
) -> IdleStep {
    let due = last_use + idle;
    if listening || open_connections > 0 || calls > 0 {
        IdleStep::CheckAt(due.max(now + idle.min(BUSY_RECHECK)))
    } else if now >= due {
        IdleStep::Release
    } else {
        IdleStep::CheckAt(due)
    }
}

/// A failed bind as every waiter gets it.
#[derive(Clone, Debug)]
enum SharedBindError {
    Closed,
    Key {
        path: PathBuf,
        kind: std::io::ErrorKind,
        text: String,
    },
    Other(String),
}

impl SharedBindError {
    /// The shareable form of `error`.
    fn of(error: &PeerError) -> Self {
        match error {
            PeerError::Closed => Self::Closed,
            PeerError::Key { path, source } => Self::Key {
                path: path.clone(),
                kind: source.kind(),
                text: source.to_string(),
            },
            PeerError::Bind(text) => Self::Other(text.clone()),
            other => Self::Other(other.to_string()),
        }
    }

    /// The `PeerError` each waiter returns, with the class the bind's own
    /// error had.
    fn to_peer_error(&self) -> PeerError {
        match self {
            Self::Closed => PeerError::Closed,
            Self::Key { path, kind, text } => PeerError::Key {
                path: path.clone(),
                source: std::io::Error::new(*kind, text.clone()),
            },
            Self::Other(text) => PeerError::Bind(text.clone()),
        }
    }
}

impl Drop for Bound {
    /// Stops the accept task, which holds a clone of the endpoint.
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// Whether the connection behind `handle` is still there and not closed.
fn is_open(handle: &WeakConnectionHandle) -> bool {
    handle
        .upgrade()
        .is_some_and(|connection| connection.close_reason().is_none())
}

/// Weak handles of one peer's session connections, by a number unique to
/// the endpoint.
type Sessions = HashMap<u64, WeakConnectionHandle>;

/// What the handshake hook and the accept task share with the endpoint.
/// Never holds the iroh `Endpoint`: the hook lives inside it, and a strong
/// reference would keep it alive forever.
#[derive(Default)]
struct Shared {
    admission: RwLock<Option<Arc<dyn Admission>>>,
    /// Admitted session connections by peer, for `close_peer`: weak so they
    /// stay closable without being kept open, and each removed when its
    /// connection closes.
    peers: Mutex<HashMap<PeerId, Sessions>>,
    /// Numbers recorded session connections, as the keys of `Sessions`.
    next_session: AtomicU64,
    /// Every connection dialed or accepted, of either protocol, weak, so
    /// the idle task can tell whether any is still open. Pruned on each
    /// insert, so it holds about as many as are open.
    open: Mutex<Vec<WeakConnectionHandle>>,
}

impl Shared {
    /// Counts `connection` as open until it closes.
    fn track(&self, connection: &endpoint::Connection) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.retain(is_open);
        open.push(connection.weak_handle());
    }

    /// How many tracked connections are still open. Open streams keep a
    /// connection open after its session dropped it, so they count too.
    fn open_connections(&self) -> usize {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.retain(is_open);
        open.len()
    }

    /// Whether an admission is set, as after `listen(Some(..))`.
    fn listening(&self) -> bool {
        self.admission
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Whether the current admission lets `peer` in for `alpn`.
    fn admits(&self, peer: &PeerId, alpn: Alpn) -> bool {
        // Called without the lock held: the daemon may hold its own lock on
        // the key set while it calls `listen`, which takes this one for
        // writing, and `admit` takes the daemon's lock.
        let admission = self
            .admission
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        admission.is_some_and(|admission| admission.admit(peer, alpn))
    }

    /// Replaces the admission.
    fn set_admission(&self, admission: Option<Arc<dyn Admission>>) {
        *self
            .admission
            .write()
            .unwrap_or_else(PoisonError::into_inner) = admission;
    }

    /// The admitted session connections by peer.
    fn peers(&self) -> MutexGuard<'_, HashMap<PeerId, Sessions>> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records a session connection of `peer` until it closes.
    ///
    /// Pairing connections are not recorded: any key may open one while a
    /// ticket is outstanding, so recording them would let strangers grow
    /// this map, and nothing closes a pairing connection by peer.
    fn record(self: &Arc<Self>, peer: PeerId, connection: &endpoint::Connection) {
        let handle = connection.weak_handle();
        // Not `stable_id`: that is the address of the connection's
        // allocation, which a later connection of the same peer can reuse
        // once this one is freed, and this connection's cleanup task, running
        // late after `close_peer`, would then remove the new entry.
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        // Registered while the connection is alive, so it resolves however
        // the connection ends.
        let closed = handle.closed();
        self.peers().entry(peer).or_default().insert(id, handle);
        let shared = Arc::downgrade(self);
        tokio::spawn(async move {
            closed.await;
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let mut peers = shared.peers();
            if let Some(sessions) = peers.get_mut(&peer) {
                sessions.remove(&id);
                if sessions.is_empty() {
                    peers.remove(&peer);
                }
            }
        });
    }
}

/// Decides right after the TLS handshake, before any stream, which incoming
/// connections stay open.
struct Gatekeeper {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Gatekeeper {
    /// No fields worth showing: the admission is the daemon's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gatekeeper")
    }
}

impl EndpointHooks for Gatekeeper {
    /// Accepts every connection this side dialed, and for incoming ones asks
    /// the admission. An admitted connection gets its stream credit here, 640
    /// streams for a session and 1 for pairing: the box's endpoint grants
    /// none in the handshake, so the credit is the client's sign that it was
    /// admitted, sent before any refusal could be.
    async fn after_handshake<'a>(
        &'a self,
        connection: &'a endpoint::Connection,
    ) -> AfterHandshakeOutcome {
        if connection.side() == Side::Client {
            return AfterHandshakeOutcome::Accept;
        }
        let peer = peer_id(&connection.remote_id());
        let admitted =
            Alpn::from_bytes(connection.alpn()).filter(|alpn| self.shared.admits(&peer, *alpn));
        let Some(alpn) = admitted else {
            tracing::debug!(peer = %peer.short(), "refused a peer in the handshake");
            return AfterHandshakeOutcome::Reject {
                error_code: VarInt::from_u32(REFUSED),
                reason: b"refused".to_vec(),
            };
        };
        let streams = match alpn {
            Alpn::Tunnel => BOX_STREAMS,
            // The exchange is one stream, and while a ticket is outstanding
            // any key gets here, so a stranger gets credit for that stream
            // and no more. The receive windows stay as the handshake
            // announced them: noq 1.5.0 cannot take back credit already
            // granted (`set_receive_window` only stops later grants until
            // the shrink is paid off), so one stream bounds what the peer
            // can make the box buffer to its 4 MiB `STREAM_WINDOW`.
            Alpn::Pair => PAIR_STREAMS,
        };
        connection.set_max_concurrent_bi_streams(VarInt::from_u32(streams));
        AfterHandshakeOutcome::Accept
    }
}

/// The devlanes form of an iroh key.
fn peer_id(key: &PublicKey) -> PeerId {
    PeerId(*key.as_bytes())
}

/// The transport settings of every connection, dialed or accepted. Path
/// keep-alive and path idle timeout keep iroh's values, which iroh tunes for
/// hole punching.
fn transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        // The peer may open no stream: on a client only the client opens
        // streams, which replaces yamux's limit on streams a box opens, and a
        // box raises it once the hook admitted the peer: to `BOX_STREAMS` for
        // a session, to `PAIR_STREAMS` for pairing.
        .max_concurrent_bidi_streams(VarInt::from_u32(0))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .stream_receive_window(VarInt::from_u32(STREAM_WINDOW))
        .receive_window(VarInt::from_u32(CONNECTION_WINDOW))
        .send_window(SEND_WINDOW)
        // The protocol sends no datagrams. noq buffers about 1.25 MB of
        // received ones per connection by default, which any peer, also an
        // unpaired one on the pairing protocol, could fill; `None` tells the
        // peer it may send none. Nothing here sends one, so the send buffer
        // stays empty.
        .datagram_receive_buffer_size(None)
        .max_idle_timeout(Some(VarInt::from_u32(IDLE_TIMEOUT_MS).into()))
        .keep_alive_interval(KEEP_ALIVE)
        .build()
}

/// Orders addresses for a ticket: public ones first, then private, link-local
/// and loopback ones, each group in iroh's order.
fn reach_rank(addr: &SocketAddr) -> u8 {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_loopback() => 3,
        IpAddr::V4(ip) if ip.is_link_local() => 2,
        IpAddr::V4(ip) if ip.is_private() => 1,
        IpAddr::V6(ip) if ip.is_loopback() => 3,
        IpAddr::V6(ip) if ip.is_unicast_link_local() => 2,
        IpAddr::V6(ip) if ip.is_unique_local() => 1,
        _ => 0,
    }
}

impl IrohEndpoint {
    /// An endpoint that binds nothing and reads no file until it is used.
    pub fn new(options: IrohOptions) -> Self {
        Self {
            inner: Arc::new(Inner::new(options)),
        }
    }
}

impl Inner {
    /// The state of an endpoint that is not bound yet.
    fn new(options: IrohOptions) -> Self {
        Self::with_idle(options, IDLE_RELEASE)
    }

    /// [`Inner::new`] with `idle` as the time before an idle release. Only
    /// this module calls it with another time, in its tests, which cannot
    /// wait 5 minutes for a release.
    fn with_idle(options: IrohOptions, idle: Duration) -> Self {
        let (tunnel, tunnel_rx) = mpsc::channel(TUNNEL_QUEUE);
        let (pair, pair_rx) = mpsc::channel(PAIR_QUEUE);
        Self {
            own_relays: options
                .relays
                .iter()
                .filter_map(|url| url.parse().ok())
                .collect(),
            options,
            key: OnceCell::new(),
            phase: Mutex::new(Phase::Unbound),
            calls: AtomicUsize::new(0),
            shared: Arc::default(),
            queues: Mutex::new(Some(Queues { tunnel, pair })),
            incoming: tokio::sync::Mutex::new(Incoming {
                tunnel: tunnel_rx,
                pair: pair_rx,
            }),
            listening: tokio::sync::Mutex::new(false),
            port_file: tokio::sync::Mutex::new(()),
            port_tasks: Mutex::new(Vec::new()),
            idle,
        }
    }

    /// The accept queues; `None` once `close` ran.
    fn queues(&self) -> Option<Queues> {
        self.queues
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether this endpoint publishes and looks up through n0's DNS.
    fn looks_up(&self) -> bool {
        self.options.address_lookup && !self.options.relays.is_empty()
    }

    /// The secret key, read or created off the async threads the first time.
    async fn key(&self) -> Result<&SecretKey, PeerError> {
        self.key
            .get_or_try_init(|| async {
                let path = self.options.key_file.clone();
                tokio::task::spawn_blocking(move || key::load_or_create(&path))
                    .await
                    .map_err(|error| PeerError::Key {
                        path: self.options.key_file.clone(),
                        source: std::io::Error::other(error),
                    })?
            })
            .await
    }

    /// The bound endpoint, binding it the first time.
    ///
    /// The bind runs in a task of its own: a caller that stops waiting (a
    /// request with a timeout) leaves it running, the next caller waits for
    /// the same bind, and a bind is never cut off after it took its sockets.
    /// A failed bind is reported to everyone waiting for it, and the next
    /// call starts a new one. A call marks the endpoint used under the phase
    /// lock, which the idle task holds to release it, so a call either gets
    /// the endpoint before the release decision (which then sees the call
    /// and keeps it) or finds it released and binds a new one.
    ///
    /// `close` sets the phase to closed under the same lock, so no bind
    /// starts after it, and a bind already running sees it when it is done.
    ///
    /// The returned [`InUse`] counts the call as in progress, also while it
    /// waits for the bind; the caller holds it until it returns, so the
    /// endpoint is not released under a call that is still waiting.
    async fn bound(self: &Arc<Self>) -> Result<(Arc<Bound>, InUse), PeerError> {
        let (task, in_use) = {
            let mut phase = self.phase();
            let task = match &*phase {
                Phase::Closed => return Err(PeerError::Closed),
                Phase::Bound(bound) => {
                    bound.touch();
                    return Ok((bound.clone(), InUse::start(self)));
                }
                Phase::Binding(task) => task.clone(),
                Phase::Unbound => self.start_bind(&mut phase, None),
                Phase::Releasing(release) => {
                    let release = release.clone();
                    self.start_bind(&mut phase, Some(release))
                }
            };
            (task, InUse::start(self))
        };
        // The bind set the endpoint's last use when it made it.
        let bound = task.await.map_err(|error| error.to_peer_error())?;
        Ok((bound, in_use))
    }

    /// The phase, locked.
    fn phase(&self) -> MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a bind in a task of its own and records it in `phase`. After a
    /// release, the bind first waits for the old endpoint to close.
    fn start_bind(self: &Arc<Self>, phase: &mut Phase, after: Option<ReleaseTask>) -> BindTask {
        let inner = self.clone();
        let spawned = tokio::spawn(async move {
            let released = after.is_some();
            if let Some(release) = after {
                release.await;
            }
            // A panic in the bind must not leave the phase binding for good,
            // with every later call getting this task's cached failure.
            let outcome = std::panic::AssertUnwindSafe(inner.bind(released))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(PeerError::Bind("the bind panicked".into())));
            let bound = match outcome {
                Ok(bound) => Arc::new(bound),
                Err(error) => {
                    // Only one bind runs at a time, so a binding phase is this
                    // one's: a later call starts a new bind. A closed one stays.
                    let mut phase = inner.phase();
                    if matches!(*phase, Phase::Binding(_)) {
                        *phase = Phase::Unbound;
                    }
                    return Err(SharedBindError::of(&error));
                }
            };
            {
                let mut phase = inner.phase();
                if matches!(*phase, Phase::Binding(_)) {
                    *phase = Phase::Bound(bound.clone());
                    drop(phase);
                    tokio::spawn(release_when_idle(
                        Arc::downgrade(&inner),
                        Arc::downgrade(&bound),
                    ));
                    return Ok(bound);
                }
            }
            // `close` ran while this bound: it waits for this task, so
            // closing here leaves no socket open when `close` returns.
            bound.endpoint.close().await;
            Err(SharedBindError::Closed)
        });
        let task = async move {
            spawned
                .await
                .unwrap_or_else(|error| Err(SharedBindError::Other(error.to_string())))
        }
        .boxed()
        .shared();
        *phase = Phase::Binding(task.clone());
        task
    }

    /// Releases `bound` if it is still the bound endpoint and still idle,
    /// deciding under the phase lock, so a call that arrived first keeps it.
    /// Returns the release in progress, or when to look again.
    fn release_if_idle(
        &self,
        bound: &Arc<Bound>,
    ) -> Result<ReleaseTask, Option<tokio::time::Instant>> {
        let mut phase = self.phase();
        let Phase::Bound(current) = &*phase else {
            return Err(None);
        };
        if !Arc::ptr_eq(current, bound) {
            return Err(None);
        }
        let step = idle_step(
            self.idle,
            self.shared.listening(),
            self.shared.open_connections(),
            self.calls.load(Ordering::Relaxed),
            bound.last_use(),
            tokio::time::Instant::now(),
        );
        if let IdleStep::CheckAt(at) = step {
            return Err(Some(at));
        }
        let bound = bound.clone();
        let release = async move {
            tracing::debug!("releasing the idle peer-to-peer endpoint");
            bound.endpoint.close().await;
            // The last `Bound` goes with this task, and with it the accept
            // task and the endpoint's sockets.
            drop(bound);
        }
        .boxed()
        .shared();
        *phase = Phase::Releasing(release.clone());
        Ok(release)
    }

    /// Marks a finished `release` as unbound, unless a bind started since.
    fn released(&self, release: &ReleaseTask) {
        let mut phase = self.phase();
        if let Phase::Releasing(current) = &*phase
            && current.ptr_eq(release)
        {
            *phase = Phase::Unbound;
        }
    }

    /// Binds the iroh endpoint with this machine's key and starts accepting.
    /// `released` says an endpoint of this machine closed just before, whose
    /// sockets iroh may still be freeing, so a saved port is tried again for
    /// a moment before another is taken.
    async fn bind(&self, released: bool) -> Result<Bound, PeerError> {
        let key = self.key().await?.clone();
        let relays = self
            .options
            .relays
            .iter()
            .map(|url| {
                url.parse::<RelayUrl>()
                    .map_err(|error| PeerError::Bind(format!("relay {url}: {error}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Ports are asked for only when a bind that listened saved them; a
        // missing file, as on a machine that only dials, means any ports.
        let port_file = ports::path(&self.options.key_file);
        let saved = tokio::task::spawn_blocking(move || ports::load(&port_file))
            .await
            .unwrap_or_default();
        // iroh binds a port it is given or fails the whole bind (`is_required`
        // sockets; a socket that may fail is left out instead, which would
        // drop that family), and each family's socket binds on its own. So a
        // taken port is retried without it, family by family, ending with
        // any ports.
        let mut attempts = vec![
            (saved.v4, saved.v6),
            (saved.v4, None),
            (None, saved.v6),
            (None, None),
        ];
        attempts.dedup();
        let mut endpoint = None;
        let mut last_error = PeerError::Bind("no bind was tried".into());
        // After a release the old sockets close in iroh's tasks shortly after
        // `close` returns; the saved ports are worth waiting about 1 s for.
        let tries_first = if released && !saved.is_empty() {
            REBIND_TRIES
        } else {
            1
        };
        'attempts: for (index, (v4, v6)) in attempts.into_iter().enumerate() {
            let tries = if index == 0 { tries_first } else { 1 };
            for attempt in 0..tries {
                if attempt > 0 {
                    tokio::time::sleep(REBIND_PAUSE).await;
                }
                let bound = match self.builder(key.clone(), &relays, v4, v6) {
                    Ok(builder) => builder
                        .bind()
                        .await
                        .map_err(|error| PeerError::Bind(error.to_string())),
                    Err(error) => Err(error),
                };
                match bound {
                    Ok(bound) => {
                        endpoint = Some(bound);
                        break 'attempts;
                    }
                    Err(error) => {
                        tracing::debug!(?v4, ?v6, %error, "binding the saved peer-to-peer ports failed");
                        last_error = error;
                    }
                }
            }
        }
        let Some(endpoint) = endpoint else {
            return Err(last_error);
        };
        tracing::debug!(sockets = ?endpoint.bound_sockets(), "peer-to-peer endpoint bound");
        let bound = ports::Ports::of(&endpoint.bound_sockets());
        // Saved only by `listen(Some)`: a box needs stable ports, while a
        // client that only dials would gain nothing and use the same source
        // port on every network it visits.
        if !saved.is_empty() && bound != saved {
            tracing::info!(
                ?saved,
                ?bound,
                "the peer-to-peer port changed; clients on a network without a relay must pair again"
            );
        }
        if self.looks_up() {
            // Added after the bind rather than on the builder, so they can be
            // wrapped in `OwnRelays`; nothing dials in between.
            self.add_resolvers(&endpoint)?;
        }
        let Some(queues) = self.queues() else {
            // Closed while binding: dropping an endpoint without closing it
            // aborts its connections and iroh logs that as an error.
            endpoint.close().await;
            return Err(PeerError::Closed);
        };
        let accepting = tokio::spawn(accept_loop(endpoint.clone(), self.shared.clone(), queues));
        Ok(Bound {
            endpoint,
            accepting,
            settled_by: tokio::time::Instant::now() + ONLINE_WAIT,
            last_use: Mutex::new(tokio::time::Instant::now()),
        })
    }

    /// Saves the bound ports in `p2p-port`, for a bind that listens: the next
    /// start of a box then binds the same ports. Written on every
    /// `listen(Some)`, which is rare, instead of trusting a copy of the file
    /// that a removal may have made stale. Runs in a task of its own, so
    /// `listen` has nothing left to await once it changed state, and a
    /// caller's timeout cannot leave it half done. A write that runs after
    /// the endpoint stopped listening does nothing, so it cannot bring back a
    /// file `forget_ports` removed. Never fails anyone: without the file the
    /// next start binds any port.
    fn keep_ports(self: &Arc<Self>, bound: Arc<Bound>) {
        let inner = self.clone();
        self.port_task(async move {
            let _file = inner.port_file.lock().await;
            if !inner.shared.listening() {
                return;
            }
            let ports = ports::Ports::of(&bound.endpoint.bound_sockets());
            let path = ports::path(&inner.options.key_file);
            match tokio::task::spawn_blocking(move || ports::save(&path, ports)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "saving the peer-to-peer ports failed"),
                Err(error) => tracing::warn!(%error, "saving the peer-to-peer ports failed"),
            }
        });
    }

    /// Runs a change to `p2p-port` in a task of its own, kept for `close`.
    /// Nothing starts once the endpoint is closed: the check and the push
    /// happen under the phase lock, which `close` takes to close the
    /// endpoint before it collects the tasks, so none is left unawaited.
    fn port_task(&self, change: impl Future<Output = ()> + Send + 'static) {
        let phase = self.phase();
        if matches!(*phase, Phase::Closed) {
            return;
        }
        let mut tasks = self
            .port_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tasks.retain(|task| !task.is_finished());
        tasks.push(tokio::spawn(change));
    }

    /// Removes `p2p-port` once the endpoint stopped listening: the daemon
    /// stops when no ticket is outstanding and no client is paired, so
    /// nothing depends on the ports any more, and keeping them would give a
    /// machine that listened once a fixed source port for every later dial.
    /// A detached task, like [`Inner::keep_ports`], in the same order.
    fn forget_ports(self: &Arc<Self>) {
        let inner = self.clone();
        self.port_task(async move {
            let _file = inner.port_file.lock().await;
            if inner.shared.listening() {
                return;
            }
            let path = ports::path(&inner.options.key_file);
            match tokio::task::spawn_blocking(move || ports::remove(&path)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "removing the peer-to-peer ports failed"),
                Err(error) => tracing::warn!(%error, "removing the peer-to-peer ports failed"),
            }
        });
    }

    /// A builder for this endpoint, binding IPv4 and IPv6 on `v4` and `v6`
    /// when given and on ports the system picks otherwise.
    fn builder(
        &self,
        key: SecretKey,
        relays: &[RelayUrl],
        v4: Option<u16>,
        v6: Option<u16>,
    ) -> Result<iroh::endpoint::Builder, PeerError> {
        let relay_mode = if relays.is_empty() {
            RelayMode::Disabled
        } else {
            RelayMode::Custom(relays.iter().cloned().collect())
        };
        let port_mapping = if self.options.port_mapping {
            PortmapperConfig::default()
        } else {
            PortmapperConfig::Disabled
        };
        // `Empty` sets nothing, so every choice is made here: ring like
        // crates/pki and crates/proxy, never a process-wide default.
        let mut builder = Endpoint::builder(presets::Empty)
            .crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .secret_key(key)
            .relay_mode(relay_mode)
            .portmapper_config(port_mapping)
            .transport_config(transport_config())
            .hooks(Gatekeeper {
                shared: self.shared.clone(),
            });
        // A given address replaces iroh's own socket of that family on all
        // interfaces, so these bind what iroh would, on a chosen port.
        //
        // On Windows the sockets lack `SO_EXCLUSIVEADDRUSE`, and neither iroh
        // nor netwatch lets a caller set it, so another local program could
        // bind the same port on one address and take its datagrams. QUIC with
        // pinned keys turns that into a denial of service by a program that
        // already runs on this machine, which a known port only makes easier;
        // accepted for the reachability a stable port gives a box.
        let wanted = [
            v4.map(|port| SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))),
            v6.map(|port| SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))),
        ];
        for addr in wanted.into_iter().flatten() {
            // Only a second address of one family fails here, which this
            // never adds; the caller then tries the next ports.
            builder = builder
                .bind_addr(addr)
                .map_err(|error| PeerError::Bind(error.to_string()))?;
        }
        Ok(builder)
    }

    /// Starts or stops publishing this endpoint's relay to n0's DNS;
    /// `publishing` is whether it does now.
    fn publish(
        &self,
        endpoint: &Endpoint,
        publishing: &mut bool,
        on: bool,
    ) -> Result<(), PeerError> {
        if !self.looks_up() || *publishing == on {
            return Ok(());
        }
        let lookup = endpoint.address_lookup().map_err(|_| PeerError::Closed)?;
        if on {
            let resolver = endpoint.dns_resolver().map_err(|_| PeerError::Closed)?;
            // Publishes the relay URL only (iroh's default filter), never the
            // IP addresses.
            lookup.add(
                PkarrPublisher::n0_dns()
                    .dns_resolver(resolver.clone())
                    .build(endpoint.secret_key().clone(), endpoint.tls_config().clone()),
            );
        } else {
            // No single removal exists, so the resolvers go back in after
            // the clear; dropping the publisher stops its task.
            lookup.clear();
            self.add_resolvers(endpoint)?;
        }
        *publishing = on;
        Ok(())
    }

    /// Adds n0's resolvers (dns.iroh.link), each behind [`OwnRelays`]. n0's
    /// docs allow production use "if you find performance acceptable", and
    /// tickets carry the relay and addresses, so pairing does not wait on
    /// it. Resolvers only: an endpoint publishes while it listens.
    fn add_resolvers(&self, endpoint: &Endpoint) -> Result<(), PeerError> {
        let lookup = endpoint.address_lookup().map_err(|_| PeerError::Closed)?;
        let resolver = endpoint.dns_resolver().map_err(|_| PeerError::Closed)?;
        let pkarr = PkarrResolver::n0_dns()
            .dns_resolver(resolver.clone())
            .build(endpoint.tls_config().clone());
        let dns = DnsAddressLookup::n0_dns()
            .dns_resolver(resolver.clone())
            .build();
        lookup.add(OwnRelays::new(pkarr, self.own_relays.clone()));
        lookup.add(OwnRelays::new(dns, self.own_relays.clone()));
        Ok(())
    }

    /// The iroh address of `addr`: its direct addresses, and its relay when
    /// that is one of this endpoint's own. The second value is the relay's
    /// cleaned host when it was dropped, for the log and the error.
    ///
    /// iroh opens a relay connection to any URL it is handed, also outside
    /// its relay map: a WebSocket upgrade to `<url>/relay` that carries this
    /// machine's public key, over plain `ws` for an `http` URL. A `PeerAddr`
    /// comes from a ticket someone pasted, so a foreign relay could point
    /// that request at localhost, a LAN device or an internal host.
    fn target(&self, addr: &PeerAddr) -> Result<(EndpointAddr, Option<String>), PeerError> {
        let id = PublicKey::from_bytes(&addr.id.0).map_err(|_| PeerError::InvalidId(addr.id))?;
        let mut target = EndpointAddr::new(id);
        let mut dropped = None;
        if let Some(relay) = &addr.relay {
            match own_relay(&self.own_relays, relay) {
                Some(url) => target = target.with_relay_url(url),
                None => {
                    let host = relay_host(relay);
                    tracing::debug!(%host, "not dialing a relay this machine does not use");
                    dropped = Some(host);
                }
            }
        }
        for direct in &addr.direct {
            target = target.with_ip_addr(*direct);
        }
        Ok((target, dropped))
    }
}

/// `error` with the relay the dial dropped named in its cause, so the caller
/// can tell the user why a box that is only reachable through its relay was
/// not reached.
fn with_dropped_relay(error: PeerError, dropped: Option<&str>) -> PeerError {
    match (error, dropped) {
        (PeerError::Unreachable(cause), Some(host)) => PeerError::Unreachable(format!(
            "{cause}; the box's relay {host} is not one this machine uses"
        )),
        (error, _) => error,
    }
}

/// `url` as one of `own`, the endpoint's configured relays, or `None`.
/// Compared as parsed relay URLs, so the host's case, a default port and a
/// trailing slash make no difference.
fn own_relay(own: &[RelayUrl], url: &str) -> Option<RelayUrl> {
    let url = url.parse::<RelayUrl>().ok()?;
    own.contains(&url).then_some(url)
}

/// The host (and port) of a relay URL from a `PeerAddr`, cleaned for a
/// message; the whole text, cleaned, when it does not parse.
fn relay_host(url: &str) -> String {
    let parsed = url.parse::<RelayUrl>().ok();
    let host = parsed.as_ref().and_then(|url| {
        let host = url.host_str()?;
        Some(match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        })
    });
    peer_text(&host.unwrap_or_else(|| url.to_owned()))
}

/// A resolver whose results name only this endpoint's own relays. A box
/// publishes its address record itself, so without this a hostile box found
/// by id could make a dial open a relay connection to a URL of its choice,
/// as a hostile ticket could (see `IrohEndpoint::target`). Direct addresses
/// and user data pass unchanged.
#[derive(Debug)]
struct OwnRelays<L> {
    inner: L,
    own: Arc<[RelayUrl]>,
}

impl<L> OwnRelays<L> {
    /// Wraps `inner`, letting through only relays in `own`.
    fn new(inner: L, own: Arc<[RelayUrl]>) -> Self {
        Self { inner, own }
    }
}

/// What a resolver yields, as iroh's `AddressLookup::resolve` names it.
type Resolved = Pin<Box<dyn Stream<Item = Result<LookupItem, LookupError>> + Send>>;

impl<L: AddressLookup> AddressLookup for OwnRelays<L> {
    /// Forwards to the inner service; the resolvers wrapped here publish nothing.
    fn publish(&self, data: &EndpointData) {
        self.inner.publish(data);
    }

    /// The inner resolver's items with foreign relay URLs removed.
    fn resolve(&self, endpoint_id: iroh::EndpointId) -> Option<Resolved> {
        let own = self.own.clone();
        let items = self.inner.resolve(endpoint_id)?;
        Some(Box::pin(items.map(move |item| {
            item.map(|item| without_foreign_relays(item, &own))
        })))
    }
}

/// `item` with the relay URLs that are not in `own` removed.
fn without_foreign_relays(item: LookupItem, own: &[RelayUrl]) -> LookupItem {
    let info = item.endpoint_info();
    let kept: Vec<TransportAddr> = info
        .data
        .addrs()
        .filter(|addr| match addr {
            TransportAddr::Relay(url) => own.contains(url),
            _ => true,
        })
        .cloned()
        .collect();
    let mut data = EndpointData::new(kept);
    data.set_user_data(info.data.user_data().cloned());
    LookupItem::new(
        EndpointInfo::from_parts(info.endpoint_id, data),
        item.provenance(),
        item.last_updated(),
    )
}

/// Whether `error` is the box refusing this machine's key.
fn is_refused(error: &endpoint::ConnectionError) -> bool {
    matches!(error, endpoint::ConnectionError::ApplicationClosed(close)
        if close.error_code.into_inner() == u64::from(REFUSED))
}

/// The typed failure of a dial whose connection closed with `error`. The
/// refusal is told by its close code; the text, which carries the box's
/// reason bytes, is cleaned.
fn closed_dial(error: &endpoint::ConnectionError) -> PeerError {
    if is_refused(error) {
        PeerError::Refused
    } else {
        PeerError::Unreachable(peer_text(&error.to_string()))
    }
}

/// The typed failure of a dial that never produced a connection: the box's
/// refusal when the close arrived during the handshake, else the whole chain
/// of causes, since iroh's outer message alone ("Unable to connect to
/// remote") says nothing. Each cause is cleaned on its own, since a close
/// among them carries the box's text.
fn dial_failed(error: endpoint::ConnectError) -> PeerError {
    let closed = match &error {
        endpoint::ConnectError::Connection { source, .. } => Some(source),
        endpoint::ConnectError::Connecting {
            source: endpoint::ConnectingError::ConnectionError { source, .. },
            ..
        } => Some(source),
        _ => None,
    };
    if closed.is_some_and(is_refused) {
        return PeerError::Refused;
    }
    let mut text = peer_text(&error.to_string());
    let mut cause = std::error::Error::source(&error);
    while let Some(next) = cause {
        let next_text = peer_text(&next.to_string());
        if !text.ends_with(&next_text) {
            text = format!("{text}: {next_text}");
        }
        cause = next.source();
    }
    PeerError::Unreachable(text)
}

/// Releases `bound` once [`idle_step`] says so, then ends; it also ends when
/// the endpoint, or this bind of it, is gone. Holds only weak references
/// while it waits, so it keeps neither alive.
async fn release_when_idle(inner: Weak<Inner>, bound: Weak<Bound>) {
    let Some(idle) = inner.upgrade().map(|inner| inner.idle) else {
        return;
    };
    let mut at = tokio::time::Instant::now() + idle;
    loop {
        tokio::time::sleep_until(at).await;
        let (Some(inner), Some(bound)) = (inner.upgrade(), bound.upgrade()) else {
            return;
        };
        match inner.release_if_idle(&bound) {
            Ok(release) => {
                drop(bound);
                release.clone().await;
                inner.released(&release);
                return;
            }
            Err(Some(next)) => at = next,
            Err(None) => return,
        }
    }
}

/// Accepts incoming connections until the endpoint closes, each in its own
/// task so a slow handshake holds up no other.
async fn accept_loop(endpoint: Endpoint, shared: Arc<Shared>, queues: Queues) {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        let queues = queues.clone();
        tokio::spawn(async move {
            // Refused peers end here: the hook closed their connection.
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(error) => {
                    let error = peer_text(&error.to_string());
                    tracing::debug!(%error, "incoming peer connection failed");
                    return;
                }
            };
            let Some(alpn) = Alpn::from_bytes(connection.alpn()) else {
                return;
            };
            let peer = peer_id(&connection.remote_id());
            shared.track(&connection);
            if alpn == Alpn::Tunnel {
                shared.record(peer, &connection);
            }
            // `close_peer` may have run between the hook and the line above,
            // and missed this connection; the admission tells.
            if !shared.admits(&peer, alpn) {
                connection.close(VarInt::from_u32(REFUSED), b"refused");
                return;
            }
            queues.push(PeerIncoming {
                peer,
                alpn,
                connection: Box::new(IrohServerConnection::new(connection)),
            });
        });
    }
}

#[async_trait]
impl PeerEndpoint for IrohEndpoint {
    /// The public half of the key file's key.
    async fn id(&self) -> Result<PeerId, PeerError> {
        self.inner.id().await
    }

    /// Binds, and reports the home relay or the first configured one and up
    /// to 8 socket addresses. Waits for the relay only until 5 s after the
    /// bind, so later calls return at once.
    async fn addr(&self) -> Result<PeerAddr, PeerError> {
        self.inner.addr().await
    }

    /// Dials `addr` and returns the connection once the box admitted this
    /// machine and a direct path is selected.
    async fn connect(&self, addr: &PeerAddr, alpn: Alpn) -> Result<BoxConnection, TransportError> {
        self.inner.connect(addr, alpn).await
    }

    /// Sets the admission and the accepted ALPNs; `Some` also keeps the
    /// bound ports for the next start, and `None` forgets them.
    async fn listen(&self, admission: Option<Arc<dyn Admission>>) -> Result<(), PeerError> {
        self.inner.listen(admission).await
    }

    /// The next admitted connection, sessions before pairing exchanges.
    async fn accept(&self) -> Option<PeerIncoming> {
        self.inner.accept().await
    }

    /// Closes `peer`'s session connections with the refused-key code.
    fn close_peer(&self, peer: &PeerId) {
        self.inner.close_peer(peer);
    }

    /// Closes the endpoint and its connections, after a bind in progress.
    async fn close(&self) {
        self.inner.close().await;
    }
}

impl Inner {
    /// The public half of the key file's key.
    async fn id(&self) -> Result<PeerId, PeerError> {
        Ok(peer_id(&self.key().await?.public()))
    }

    /// Binds, and reports the home relay (or the first configured one when
    /// none answered yet) and up to 8 socket addresses, public ones first.
    ///
    /// Waits for the relay and for the first local addresses only until
    /// [`ONLINE_WAIT`] after the bind: the first call after a bind may wait,
    /// later ones return at once with what is known, so a relay that never
    /// answers does not cost every ticket 5 seconds. The endpoint keeps
    /// trying the relay, so a later call names the home relay once reached.
    async fn addr(self: &Arc<Self>) -> Result<PeerAddr, PeerError> {
        // Held to the end: the waits below are part of the call.
        let (bound, _in_use) = self.bound().await?;
        let endpoint = &bound.endpoint;
        let relays = !self.options.relays.is_empty();
        let _ = tokio::time::timeout_at(bound.settled_by, async {
            if relays {
                endpoint.online().await;
            }
            // Right after binding, iroh may not have read the interfaces yet.
            let mut watcher = endpoint.watch_addr();
            while watcher.get().ip_addrs().next().is_none() {
                if watcher.updated().await.is_err() {
                    break;
                }
            }
        })
        .await;
        let current = endpoint.addr();
        let relay = current
            .relay_urls()
            .next()
            .map(ToString::to_string)
            .or_else(|| self.options.relays.first().cloned());
        let mut direct: Vec<SocketAddr> = current.ip_addrs().copied().collect();
        direct.sort_by_key(reach_rank);
        direct.truncate(MAX_DIRECT_ADDRS);
        Ok(PeerAddr {
            id: peer_id(&endpoint.id()),
            relay,
            direct,
        })
    }

    /// Dials `addr`, then waits up to [`DIRECT_PATH_WAIT`] for the box to
    /// admit this machine and for a direct path to be selected. Only then
    /// is the connection handed out; its relay watcher then closes it once
    /// it has had no direct path for `RELAY_GRACE`.
    ///
    /// A refused key does not fail the dial itself: the connection opens
    /// and the box closes it with the refused code, which both waits see.
    async fn connect(
        self: &Arc<Self>,
        addr: &PeerAddr,
        alpn: Alpn,
    ) -> Result<BoxConnection, TransportError> {
        // Held to the end: until the connection is tracked, only this keeps
        // the endpoint from a release during the handshake and the waits.
        let (bound, _in_use) = self.bound().await?;
        let endpoint = &bound.endpoint;
        let (target, dropped) = self.target(addr)?;
        let connection = endpoint
            .connect(target, alpn.bytes())
            .await
            .map_err(|error| with_dropped_relay(dial_failed(error), dropped.as_deref()))?;
        // An open connection keeps the endpoint from an idle release.
        self.shared.track(&connection);
        // Subscribed before the gate: the subscription delivers only later
        // events, so a relay selected after the gate's snapshot is not missed.
        let events = connection.path_events();
        let deadline = tokio::time::Instant::now() + DIRECT_PATH_WAIT;
        // The box grants stream credit only once it admitted this key, so a
        // stream that opens means no refusal is on its way.
        let first = match tokio::time::timeout_at(deadline, connection.open_bi()).await {
            Ok(Ok(first)) => first,
            Ok(Err(error)) => return Err(closed_dial(&error).into()),
            Err(_) => {
                connection.close(VarInt::from_u32(NO_DIRECT_PATH), b"not admitted in time");
                return Err(PeerError::Unreachable(format!(
                    "the box did not admit this machine within {} seconds",
                    DIRECT_PATH_WAIT.as_secs()
                ))
                .into());
            }
        };
        let direct = tokio::time::timeout_at(deadline, async {
            let mut paths = connection.paths_stream();
            paths
                .wait_any(|path| path.is_ip() && path.is_selected())
                .await
                .is_some()
        })
        .await;
        match direct {
            Ok(true) => Ok(Box::new(IrohConnection::new(connection, first, events))),
            // The paths end when the connection closes.
            Ok(false) => Err(connection
                .close_reason()
                .map_or(PeerError::Closed, |error| closed_dial(&error))
                .into()),
            Err(_) => {
                connection.close(VarInt::from_u32(NO_DIRECT_PATH), b"no direct path");
                Err(PeerError::NoDirectPath {
                    waited: DIRECT_PATH_WAIT,
                }
                .into())
            }
        }
    }

    /// Sets the admission and the accepted ALPNs, and publishes or stops.
    /// `None` on an endpoint that is not bound only clears the admission:
    /// binding to stop listening would contact the relay and the router for
    /// nothing. `Some` keeps the bound ports for the next start, `None`
    /// forgets them.
    async fn listen(
        self: &Arc<Self>,
        admission: Option<Arc<dyn Admission>>,
    ) -> Result<(), PeerError> {
        let mut publishing = self.listening.lock().await;
        if self.queues().is_none() {
            return Err(PeerError::Closed);
        }
        // An endpoint binding for a dial or released while idle starts with no
        // ALPN, so it accepts nothing anyway; stopping needs no bind.
        // Read under one lock, so a release between a check and the use
        // cannot make stopping bind a new endpoint.
        let bound_now = match &*self.phase() {
            Phase::Bound(bound) => Some(bound.clone()),
            _ => None,
        };
        // Stopping needs no guard: it has no await, and a released endpoint
        // has nothing to stop.
        let (bound, _in_use) = match (admission.is_some(), bound_now) {
            (false, None) => {
                self.shared.set_admission(None);
                self.forget_ports();
                return Ok(());
            }
            (false, Some(bound)) => (bound, None),
            // The only await once the lock is held: a caller dropped here
            // has changed nothing. Everything below runs to its end without
            // one, and the port file is written or removed in a task of its
            // own.
            (true, _) => {
                let (bound, in_use) = self.bound().await?;
                (bound, Some(in_use))
            }
        };
        let endpoint = &bound.endpoint;
        let listening = admission.is_some();
        // The admission is in place before the first handshake can arrive,
        // and gone only after the last one that could use it.
        if listening {
            self.shared.set_admission(admission);
            endpoint.set_alpns(vec![TUNNEL_ALPN.to_vec(), PAIR_ALPN.to_vec()]);
        } else {
            // With no ALPN, rustls refuses every handshake, so a stopped box
            // looks unreachable instead of refusing keys.
            endpoint.set_alpns(Vec::new());
            self.shared.set_admission(None);
        }
        let published = self.publish(endpoint, &mut publishing, listening);
        if listening {
            self.keep_ports(bound.clone());
        } else {
            self.forget_ports();
        }
        published
    }

    /// The next admitted connection, sessions before pairing exchanges.
    async fn accept(&self) -> Option<PeerIncoming> {
        let mut incoming = self.incoming.lock().await;
        let Incoming { tunnel, pair } = &mut *incoming;
        tokio::select! {
            biased;
            Some(arrived) = tunnel.recv() => Some(arrived),
            Some(arrived) = pair.recv() => Some(arrived),
            else => None,
        }
    }

    /// Closes `peer`'s session connections with the refused-key code.
    fn close_peer(&self, peer: &PeerId) {
        let Some(sessions) = self.shared.peers().remove(peer) else {
            return;
        };
        for connection in sessions.values().filter_map(WeakConnectionHandle::upgrade) {
            connection.close(VarInt::from_u32(REFUSED), b"refused");
        }
    }

    /// Closes the iroh endpoint for good, unlike an idle release: it closes
    /// every connection with code 0 and waits, bounded, for the peers to
    /// learn it, and later calls fail with `Closed`. A bind in progress is
    /// waited for (it sees the endpoint closed and closes what it bound
    /// itself), and so is a release, so `close` returns with no socket left
    /// open; so are pending changes to `p2p-port`, so the next start finds
    /// the file as the last `listen` left it.
    async fn close(&self) {
        self.queues
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let phase = std::mem::replace(&mut *self.phase(), Phase::Closed);
        match phase {
            Phase::Unbound | Phase::Closed => {}
            // The bind sees the closed phase when it is done and closes what
            // it bound itself.
            Phase::Binding(task) => {
                let _ = task.await;
            }
            Phase::Bound(bound) => bound.endpoint.close().await,
            Phase::Releasing(release) => release.await,
        }
        let port_tasks = std::mem::take(
            &mut *self
                .port_tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for task in port_tasks {
            let _ = task.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use iroh::RelayUrlParseError;
    use iroh::address_lookup::MemoryLookup;
    use tempfile::TempDir;

    use super::*;

    /// The endpoint is released only when nothing needs it: not while it
    /// listens or has an open connection, however long since the last call,
    /// and not before `IDLE_RELEASE` has passed since the last call.
    #[test]
    fn only_an_unused_endpoint_is_released() {
        let start = tokio::time::Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let release = IDLE_RELEASE.as_secs();
        // (listening, open connections, calls in progress, last call, now)
        // and the step.
        let cases = [
            (
                "idle long enough",
                false,
                0,
                0,
                0,
                release,
                IdleStep::Release,
            ),
            (
                "idle, not long enough",
                false,
                0,
                0,
                10,
                release,
                IdleStep::CheckAt(at(10 + release)),
            ),
            (
                "listening",
                true,
                0,
                0,
                0,
                10 * release,
                IdleStep::CheckAt(at(10 * release) + BUSY_RECHECK),
            ),
            (
                "a connection open",
                false,
                1,
                0,
                0,
                10 * release,
                IdleStep::CheckAt(at(10 * release) + BUSY_RECHECK),
            ),
            (
                "a call in progress",
                false,
                0,
                1,
                0,
                10 * release,
                IdleStep::CheckAt(at(10 * release) + BUSY_RECHECK),
            ),
            (
                "busy, called lately",
                false,
                2,
                0,
                100,
                110,
                IdleStep::CheckAt(at(100 + release)),
            ),
        ];
        for (name, listening, open, calls, last_use, now, step) in cases {
            assert_eq!(
                idle_step(IDLE_RELEASE, listening, open, calls, at(last_use), at(now)),
                step,
                "{name}"
            );
        }
    }

    /// A relay URL counts as one of the endpoint's own when it names the same
    /// relay in another spelling, and only then.
    #[test]
    fn a_relay_is_own_in_any_spelling_and_foreign_otherwise() -> Result<(), RelayUrlParseError> {
        let own: Vec<RelayUrl> = vec![
            "https://relay1.devlanes.run".parse()?,
            "http://127.0.0.1:3340".parse()?,
        ];
        let cases = [
            ("https://relay1.devlanes.run/", true),
            ("https://RELAY1.devlanes.run:443", true),
            ("http://127.0.0.1:3340/", true),
            ("https://relay2.devlanes.run", false),
            ("http://relay1.devlanes.run", false),
            ("http://127.0.0.1:3341", false),
            ("https://relay1.devlanes.run/elsewhere", false),
            ("not a url", false),
        ];
        for (url, is_own) in cases {
            assert_eq!(own_relay(&own, url).is_some(), is_own, "{url}");
        }
        Ok(())
    }

    /// An address record a box published reaches the dial without the
    /// relays this machine does not use, and with its own relay and the
    /// direct addresses as they were.
    #[tokio::test]
    async fn lookup_results_lose_foreign_relays_only() -> Result<(), Box<dyn std::error::Error>> {
        let own: RelayUrl = "https://relay1.devlanes.run".parse()?;
        let foreign: RelayUrl = "http://192.168.1.1:8080".parse()?;
        let direct: SocketAddr = "192.0.2.7:4433".parse()?;
        let id = SecretKey::generate().public();
        let records = MemoryLookup::new();
        records.add_endpoint_info(EndpointAddr::from_parts(
            id,
            [
                TransportAddr::Relay(foreign),
                TransportAddr::Relay(own.clone()),
                TransportAddr::Ip(direct),
            ],
        ));
        let lookup = OwnRelays::new(records, Arc::from([own.clone()]));
        let mut items = lookup.resolve(id).ok_or("no lookup")?;
        let item = items.next().await.ok_or("no item")??;
        let relays: Vec<_> = item.relay_urls().cloned().collect();
        let ips: Vec<_> = item.ip_addrs().copied().collect();
        assert_eq!(relays, [own]);
        assert_eq!(ips, [direct]);
        Ok(())
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The idle time of the release tests: real sockets and the real clock,
    /// so a release takes a fraction of a second instead of 5 minutes.
    const IDLE: Duration = Duration::from_millis(200);

    /// How long any release test may take.
    const RELEASE_TEST: Duration = Duration::from_secs(10);

    /// Lets every peer in.
    struct Everyone;

    impl Admission for Everyone {
        /// Admits `_peer` for any protocol.
        fn admit(&self, _peer: &PeerId, _alpn: Alpn) -> bool {
            true
        }
    }

    /// An endpoint named `name` in `home` that is released after [`IDLE`],
    /// with no relay, lookup or port mapping, so nothing leaves the machine.
    fn short_lived(home: &TempDir, name: &str) -> Arc<Inner> {
        lasting(home, name, IDLE)
    }

    /// An endpoint like [`short_lived`] that is released after `idle`.
    fn lasting(home: &TempDir, name: &str, idle: Duration) -> Arc<Inner> {
        Arc::new(Inner::with_idle(
            IrohOptions {
                key_file: home.path().join(name).join("p2p-key"),
                relays: Vec::new(),
                address_lookup: false,
                port_mapping: false,
            },
            idle,
        ))
    }

    /// The ports the endpoint has bound, or none while it is not bound.
    fn bound_ports(inner: &Inner) -> ports::Ports {
        match &*inner.phase() {
            Phase::Bound(bound) => ports::Ports::of(&bound.endpoint.bound_sockets()),
            _ => ports::Ports::default(),
        }
    }

    /// Whether other sockets can bind both of `ports` again.
    fn ports_free(ports: ports::Ports) -> bool {
        ports.v4.is_none_or(port_free)
            && ports
                .v6
                .is_none_or(|port| std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, port)).is_ok())
    }

    /// The IPv4 port the endpoint reports for a ticket, binding it if needed.
    async fn v4_port(inner: &Arc<Inner>) -> Result<u16, Box<dyn std::error::Error>> {
        let addr = inner.addr().await?;
        let port = addr.direct.iter().find(|addr| addr.is_ipv4());
        Ok(port.ok_or("no IPv4 address reported")?.port())
    }

    /// Whether the endpoint was released (or never bound).
    fn released(inner: &Inner) -> bool {
        matches!(*inner.phase(), Phase::Unbound)
    }

    /// Whether another socket can bind `port`, which iroh's socket held.
    fn port_free(port: u16) -> bool {
        std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
    }

    /// Polls `done` until it holds, failing after [`RELEASE_TEST`].
    async fn until(what: &str, mut done: impl FnMut() -> bool) -> TestResult {
        let deadline = tokio::time::Instant::now() + RELEASE_TEST;
        while !done() {
            if tokio::time::Instant::now() > deadline {
                return Err(format!("{what} did not happen").into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    /// Fails the test instead of hanging.
    async fn bounded(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::time::timeout(RELEASE_TEST, test).await?
    }

    /// [`bounded`] with 30 s, for the test whose release follows an
    /// abandoned dial: iroh's close waits about 3 s for that handshake, on
    /// top of the bind and the dial, which a slow runner can stretch past
    /// [`RELEASE_TEST`].
    async fn bounded_long(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), test).await?
    }

    /// An endpoint that only dialed is released once unused: its socket can
    /// be bound by another program again, and the next call binds a new one
    /// that works.
    #[tokio::test]
    async fn an_unused_endpoint_is_released_and_binds_again() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let inner = short_lived(&home, "client");
            let port = v4_port(&inner).await?;
            until("the release", || released(&inner) && port_free(port)).await?;
            v4_port(&inner).await?;
            assert!(matches!(*inner.phase(), Phase::Bound(_)));
            inner.close().await;
            Ok(())
        })
        .await
    }

    /// An endpoint that stopped listening forgets its ports and is released
    /// like one that never listened, then binds again on the next call.
    #[tokio::test]
    async fn an_endpoint_that_stopped_listening_is_released() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let inner = short_lived(&home, "box");
            let port_file = ports::path(&inner.options.key_file);
            inner.listen(Some(Arc::new(Everyone))).await?;
            let port = v4_port(&inner).await?;
            until("the saved ports", || port_file.exists()).await?;
            inner.listen(None).await?;
            until("forgetting the ports", || !port_file.exists()).await?;
            until("the release", || released(&inner) && port_free(port)).await?;
            v4_port(&inner).await?;
            assert!(!port_file.exists(), "a dial brought the saved ports back");
            inner.close().await;
            Ok(())
        })
        .await
    }

    /// Neither a listening endpoint nor one with an open connection is
    /// released. The endpoint used last, which only dialed, is released
    /// first, which shows the others had the time to be.
    #[tokio::test]
    async fn listening_and_connected_endpoints_are_kept() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let listening = short_lived(&home, "box");
            let connected = short_lived(&home, "client");
            let anchor = short_lived(&home, "idle");
            listening.listen(Some(Arc::new(Everyone))).await?;
            let addr = listening.addr().await?;
            let _connection = connected.connect(&addr, Alpn::Tunnel).await?;
            let _incoming = listening.accept().await.ok_or("no connection")?;
            v4_port(&anchor).await?;
            until("the unused endpoint's release", || released(&anchor)).await?;
            assert!(
                matches!(*listening.phase(), Phase::Bound(_)),
                "the box was released"
            );
            assert!(
                matches!(*connected.phase(), Phase::Bound(_)),
                "the client was released"
            );
            for inner in [listening, connected, anchor] {
                inner.close().await;
            }
            Ok(())
        })
        .await
    }

    /// `close` while a release runs, or right after, returns, and every
    /// later call fails with `Closed` instead of binding again.
    #[tokio::test]
    async fn close_during_a_release_is_final() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let inner = short_lived(&home, "client");
            let port = v4_port(&inner).await?;
            until("the release to start", || {
                !matches!(*inner.phase(), Phase::Bound(_))
            })
            .await?;
            inner.close().await;
            assert!(matches!(inner.addr().await, Err(PeerError::Closed)));
            assert!(matches!(inner.listen(None).await, Err(PeerError::Closed)));
            assert!(matches!(*inner.phase(), Phase::Closed));
            until("the socket to be freed", || port_free(port)).await?;
            Ok(())
        })
        .await
    }

    /// A call made around the moment of a release, from a little before the
    /// idle time ends to a little after, always gets a working endpoint:
    /// the old one before it closes, or a new one.
    #[tokio::test]
    async fn a_call_at_the_release_gets_a_working_endpoint() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let inner = short_lived(&home, "client");
            v4_port(&inner).await?;
            for offset in [-30i64, -20, -10, -5, 0, 2, 5, 10, 20, 30] {
                let last_use = match &*inner.phase() {
                    Phase::Bound(bound) => bound.last_use(),
                    _ => return Err("not bound after a call".into()),
                };
                let at = if offset < 0 {
                    last_use + IDLE - Duration::from_millis(offset.unsigned_abs())
                } else {
                    last_use + IDLE + Duration::from_millis(offset.unsigned_abs())
                };
                tokio::time::sleep_until(at).await;
                v4_port(&inner)
                    .await
                    .map_err(|error| format!("offset {offset} ms: {error}"))?;
            }
            inner.close().await;
            Ok(())
        })
        .await
    }

    /// A call still in progress keeps the endpoint bound over several idle
    /// times, and the endpoint is released once it ended. The call is a dial
    /// to a socket that never answers, cut off by the test after 5 idle times.
    #[tokio::test]
    async fn a_call_in_progress_keeps_the_endpoint() -> TestResult {
        bounded_long(async {
            let home = tempfile::tempdir()?;
            let inner = short_lived(&home, "client");
            let silent = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
            let addr = PeerAddr {
                id: short_lived(&home, "box").id().await?,
                relay: None,
                direct: vec![silent.local_addr()?],
            };
            let dialing = {
                let inner = inner.clone();
                tokio::spawn(async move {
                    tokio::time::timeout(5 * IDLE, inner.connect(&addr, Alpn::Tunnel))
                        .await
                        .is_err()
                })
            };
            let mut was_bound = false;
            while !dialing.is_finished() {
                let bound = matches!(*inner.phase(), Phase::Bound(_));
                assert!(!was_bound || bound, "released during the dial");
                was_bound |= bound;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(dialing.await?, "the dial ended before the test cut it off");
            assert!(was_bound, "the dial never bound the endpoint");
            // iroh's close then waits about 3 s for the abandoned handshake,
            // so the release starting is what this waits for.
            until("the release after the dial", || {
                !matches!(*inner.phase(), Phase::Bound(_))
            })
            .await?;
            inner.close().await;
            Ok(())
        })
        .await
    }

    /// `listen(Some)` writes `p2p-port` even when the bind found the same
    /// ports in it: a removal since then (a `listen(None)` while the endpoint
    /// was not yet bound) must not leave a listening box without the file.
    #[tokio::test]
    async fn listening_writes_the_ports_after_a_removal() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let first = lasting(&home, "box", IDLE_RELEASE);
            let port_file = ports::path(&first.options.key_file);
            first.listen(Some(Arc::new(Everyone))).await?;
            until("the saved ports", || port_file.exists()).await?;
            let saved = bound_ports(&first);
            first.close().await;
            until("the old sockets to be freed", || ports_free(saved)).await?;
            let second = lasting(&home, "box", IDLE_RELEASE);
            second.addr().await?;
            assert_eq!(
                bound_ports(&second),
                saved,
                "the bind did not reuse the ports"
            );
            second.forget_ports();
            until("the removal", || !port_file.exists()).await?;
            second.listen(Some(Arc::new(Everyone))).await?;
            until("the ports saved again", || port_file.exists()).await?;
            assert_eq!(ports::load(&port_file), saved);
            second.close().await;
            Ok(())
        })
        .await
    }

    /// `listen(None)` on an endpoint that never bound, as the daemon calls it
    /// at startup when nothing needs a listener, removes a `p2p-port` left
    /// behind and does nothing else: no key file and no bind.
    #[tokio::test]
    async fn stopping_an_unbound_endpoint_only_removes_the_ports() -> TestResult {
        bounded(async {
            let home = tempfile::tempdir()?;
            let inner = short_lived(&home, "box");
            let port_file = ports::path(&inner.options.key_file);
            std::fs::create_dir_all(home.path().join("box"))?;
            ports::save(
                &port_file,
                ports::Ports {
                    v4: Some(40_000),
                    v6: None,
                },
            )?;
            inner.listen(None).await?;
            until("the removal", || !port_file.exists()).await?;
            assert!(!inner.options.key_file.exists(), "a key file was created");
            assert!(released(&inner), "the endpoint was bound");
            inner.close().await;
            Ok(())
        })
        .await
    }
}
