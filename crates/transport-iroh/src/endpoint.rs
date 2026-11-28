//! The daemon's one iroh endpoint: bound on first use, listening while the
//! daemon asks it to, and dialing boxes over direct paths only.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{
    Admission, Alpn, BoxConnection, PAIR_ALPN, PeerAddr, PeerEndpoint, PeerError, PeerId,
    PeerIncoming, TUNNEL_ALPN, TransportError,
};
use iroh::address_lookup::{DnsAddressLookup, PkarrPublisher, PkarrResolver};
use iroh::endpoint::{
    self, AfterHandshakeOutcome, EndpointHooks, PortmapperConfig, QuicTransportConfig, Side,
    VarInt, WeakConnectionHandle, presets,
};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey, Watcher};
use tokio::sync::{OnceCell, mpsc};

use crate::connection::{IrohConnection, IrohServerConnection, NO_DIRECT_PATH, REFUSED};
use crate::key;

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
    pub key_file: PathBuf,
    /// Relay URLs such as `https://relay1.devlanes.run`. Empty for none: then
    /// peers connect only through the addresses they know of each other.
    /// The daemon's config validates them; one that does not parse here
    /// fails the bind.
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
pub struct IrohEndpoint {
    options: IrohOptions,
    key: OnceCell<SecretKey>,
    bound: OnceCell<Bound>,
    shared: Arc<Shared>,
    /// Where the accept task queues admitted connections; `None` once
    /// closed, which ends `accept` after the queues drain.
    queues: Mutex<Option<Queues>>,
    incoming: tokio::sync::Mutex<Incoming>,
    /// Held for the whole of `listen`, so concurrent calls apply in turn;
    /// holds whether a publisher is registered, so it is added once.
    listening: tokio::sync::Mutex<bool>,
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
}

impl Drop for Bound {
    /// Stops the accept task, which holds a clone of the endpoint.
    fn drop(&mut self) {
        self.accepting.abort();
    }
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
}

impl Shared {
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
    /// the admission. An admitted connection gets its stream credit here:
    /// the box's endpoint grants none in the handshake, so the credit is the
    /// client's sign that it was admitted, sent before any refusal could be.
    async fn after_handshake<'a>(
        &'a self,
        connection: &'a endpoint::Connection,
    ) -> AfterHandshakeOutcome {
        if connection.side() == Side::Client {
            return AfterHandshakeOutcome::Accept;
        }
        let peer = peer_id(&connection.remote_id());
        let admitted =
            Alpn::from_bytes(connection.alpn()).is_some_and(|alpn| self.shared.admits(&peer, alpn));
        if !admitted {
            tracing::debug!(peer = %peer.short(), "refused a peer in the handshake");
            return AfterHandshakeOutcome::Reject {
                error_code: VarInt::from_u32(REFUSED),
                reason: b"refused".to_vec(),
            };
        }
        connection.set_max_concurrent_bi_streams(VarInt::from_u32(BOX_STREAMS));
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
        // box raises it to `BOX_STREAMS` once the hook admitted the peer.
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
        let (tunnel, tunnel_rx) = mpsc::channel(TUNNEL_QUEUE);
        let (pair, pair_rx) = mpsc::channel(PAIR_QUEUE);
        Self {
            options,
            key: OnceCell::new(),
            bound: OnceCell::new(),
            shared: Arc::default(),
            queues: Mutex::new(Some(Queues { tunnel, pair })),
            incoming: tokio::sync::Mutex::new(Incoming {
                tunnel: tunnel_rx,
                pair: pair_rx,
            }),
            listening: tokio::sync::Mutex::new(false),
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
    async fn bound(&self) -> Result<&Endpoint, PeerError> {
        if self.queues().is_none() {
            return Err(PeerError::Closed);
        }
        let bound = self.bound.get_or_try_init(|| self.bind()).await?;
        // `close` may have run while this call or another was binding, and
        // found nothing to close then. Closing twice is a no-op in iroh.
        if self.queues().is_none() {
            bound.endpoint.close().await;
            return Err(PeerError::Closed);
        }
        Ok(&bound.endpoint)
    }

    /// Binds the iroh endpoint with this machine's key and starts accepting.
    async fn bind(&self) -> Result<Bound, PeerError> {
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
        let relay_mode = if relays.is_empty() {
            RelayMode::Disabled
        } else {
            RelayMode::Custom(relays.into_iter().collect())
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
        if self.looks_up() {
            // n0's DNS (dns.iroh.link); n0's docs allow production use "if you
            // find performance acceptable", and tickets carry the relay and
            // addresses, so pairing does not wait on it. Resolvers only: an
            // endpoint publishes while it listens.
            builder = builder
                .address_lookup(PkarrResolver::n0_dns())
                .address_lookup(DnsAddressLookup::n0_dns());
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|error| PeerError::Bind(error.to_string()))?;
        tracing::debug!(sockets = ?endpoint.bound_sockets(), "peer-to-peer endpoint bound");
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
        })
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
        let resolver = endpoint.dns_resolver().map_err(|_| PeerError::Closed)?;
        let tls = endpoint.tls_config().clone();
        if on {
            // Publishes the relay URL only (iroh's default filter), never the
            // IP addresses.
            lookup.add(
                PkarrPublisher::n0_dns()
                    .dns_resolver(resolver.clone())
                    .build(endpoint.secret_key().clone(), tls),
            );
        } else {
            // No single removal exists, so the resolvers go back in after
            // the clear; dropping the publisher stops its task.
            lookup.clear();
            lookup.add(
                PkarrResolver::n0_dns()
                    .dns_resolver(resolver.clone())
                    .build(tls),
            );
            lookup.add(
                DnsAddressLookup::n0_dns()
                    .dns_resolver(resolver.clone())
                    .build(),
            );
        }
        *publishing = on;
        Ok(())
    }

    /// The iroh address of `addr`: its relay and direct addresses.
    fn target(addr: &PeerAddr) -> Result<EndpointAddr, PeerError> {
        let id = PublicKey::from_bytes(&addr.id.0).map_err(|_| PeerError::InvalidId(addr.id))?;
        let mut target = EndpointAddr::new(id);
        if let Some(relay) = &addr.relay {
            match relay.parse::<RelayUrl>() {
                Ok(url) => target = target.with_relay_url(url),
                Err(error) => {
                    tracing::debug!(%relay, %error, "ignoring a relay URL that does not parse")
                }
            }
        }
        for direct in &addr.direct {
            target = target.with_ip_addr(*direct);
        }
        Ok(target)
    }
}

/// Whether `error` is the box refusing this machine's key.
fn is_refused(error: &endpoint::ConnectionError) -> bool {
    matches!(error, endpoint::ConnectionError::ApplicationClosed(close)
        if close.error_code.into_inner() == u64::from(REFUSED))
}

/// The typed failure of a dial whose connection closed with `error`.
fn closed_dial(error: &endpoint::ConnectionError) -> PeerError {
    if is_refused(error) {
        PeerError::Refused
    } else {
        PeerError::Unreachable(error.to_string())
    }
}

/// The typed failure of a dial that never produced a connection: the box's
/// refusal when the close arrived during the handshake, else the whole chain
/// of causes, since iroh's outer message alone ("Unable to connect to
/// remote") says nothing.
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
    let mut text = error.to_string();
    let mut cause = std::error::Error::source(&error);
    while let Some(next) = cause {
        let next_text = next.to_string();
        if !text.ends_with(&next_text) {
            text = format!("{text}: {next_text}");
        }
        cause = next.source();
    }
    PeerError::Unreachable(text)
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
                    tracing::debug!(%error, "incoming peer connection failed");
                    return;
                }
            };
            let Some(alpn) = Alpn::from_bytes(connection.alpn()) else {
                return;
            };
            let peer = peer_id(&connection.remote_id());
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
        Ok(peer_id(&self.key().await?.public()))
    }

    /// Binds, waits up to [`ONLINE_WAIT`] for the relay and for the first
    /// local addresses, and reports the home relay (or the first configured
    /// one when none answered yet) and up to 8 socket addresses, public ones
    /// first.
    async fn addr(&self) -> Result<PeerAddr, PeerError> {
        let endpoint = self.bound().await?;
        let relays = !self.options.relays.is_empty();
        let _ = tokio::time::timeout(ONLINE_WAIT, async {
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
    async fn connect(&self, addr: &PeerAddr, alpn: Alpn) -> Result<BoxConnection, TransportError> {
        let endpoint = self.bound().await?;
        let target = Self::target(addr)?;
        let connection = endpoint
            .connect(target, alpn.bytes())
            .await
            .map_err(dial_failed)?;
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
    /// `None` on an endpoint that never bound only clears the admission:
    /// binding to stop listening would contact the relay and the router for
    /// nothing.
    async fn listen(&self, admission: Option<Arc<dyn Admission>>) -> Result<(), PeerError> {
        let mut publishing = self.listening.lock().await;
        if self.queues().is_none() {
            return Err(PeerError::Closed);
        }
        if admission.is_none() && self.bound.get().is_none() {
            self.shared.set_admission(None);
            return Ok(());
        }
        let endpoint = self.bound().await?;
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
        self.publish(endpoint, &mut publishing, listening)
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

    /// Closes the iroh endpoint, which closes every connection with code 0
    /// and waits, bounded, for the peers to learn it.
    async fn close(&self) {
        self.queues
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(bound) = self.bound.get() {
            bound.endpoint.close().await;
        }
    }
}
