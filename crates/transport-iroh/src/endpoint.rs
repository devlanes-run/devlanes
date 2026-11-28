//! The daemon's one iroh endpoint: bound on first use, listening while the
//! daemon asks it to, and dialing boxes over direct paths only.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{
    Admission, Alpn, BoxConnection, PAIR_ALPN, PeerAddr, PeerEndpoint, PeerError, PeerId,
    PeerIncoming, TUNNEL_ALPN, TransportError,
};
use futures_util::{Stream, StreamExt};
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
    /// The UDP ports the endpoint bound are kept next to it in `p2p-port`,
    /// so the next start binds the same ones and stored addresses still
    /// reach it.
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
pub struct IrohEndpoint {
    options: IrohOptions,
    /// `options.relays` parsed: the only relays a dial or a lookup result
    /// may lead to.
    own_relays: Arc<[RelayUrl]>,
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
        let port_file = ports::path(&self.options.key_file);
        let saved = {
            let port_file = port_file.clone();
            tokio::task::spawn_blocking(move || ports::load(&port_file))
                .await
                .unwrap_or_default()
        };
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
        for (v4, v6) in attempts {
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
                    break;
                }
                Err(error) => {
                    tracing::debug!(?v4, ?v6, %error, "binding the saved peer-to-peer ports failed");
                    last_error = error;
                }
            }
        }
        let Some(endpoint) = endpoint else {
            return Err(last_error);
        };
        tracing::debug!(sockets = ?endpoint.bound_sockets(), "peer-to-peer endpoint bound");
        let bound = ports::Ports::of(&endpoint.bound_sockets());
        if bound != saved {
            if !saved.is_empty() {
                tracing::info!(
                    ?saved,
                    ?bound,
                    "the peer-to-peer port changed; clients on a network without a relay must pair again"
                );
            }
            let saved = tokio::task::spawn_blocking(move || ports::save(&port_file, bound)).await;
            if let Ok(Err(error)) = saved {
                tracing::warn!(%error, "saving the peer-to-peer ports failed");
            }
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
        })
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
        let (target, dropped) = self.target(addr)?;
        let connection = endpoint
            .connect(target, alpn.bytes())
            .await
            .map_err(|error| with_dropped_relay(dial_failed(error), dropped.as_deref()))?;
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

#[cfg(test)]
mod tests {
    use iroh::RelayUrlParseError;
    use iroh::address_lookup::MemoryLookup;

    use super::*;

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
}
