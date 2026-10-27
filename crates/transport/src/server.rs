//! Box side: accepts client streams and answers them from injected services.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use crate::mux::{self, MuxServer, MuxStream};
use crate::pairing::{PairingCheck, PairingClaim, PairingGate};
use crate::registry::{AuthorizedTarget, Registry, RegistrySource, RegistryUpdate};
use crate::session::Timing;
use crate::sync::{self, SyncSource};
use crate::transport::{BoxDuplex, BoxError, Duplex};
use crate::wire::{
    self, BoxControl, BoxInfo, Channel, ClientControl, DialTarget, FEATURES, MIRROR_REPORT,
    OpenReply, OpenRequest, PROTOCOL_VERSION, RejectReason, SYNC_ZSTD, WireError, negotiate,
};

/// Connects to an authorized endpoint on the box.
#[async_trait]
pub trait Dialer: Send + Sync {
    /// Opens a byte stream to `target`, which already passed registry authorization.
    async fn dial(&self, target: &AuthorizedTarget) -> io::Result<BoxDuplex>;
}

/// Dials `127.0.0.1:<port>`, then `[::1]:<port>`, matching how services bind;
/// a target that names its address ([`AuthorizedTarget::address`]) only there.
pub struct LoopbackDialer;

#[async_trait]
impl Dialer for LoopbackDialer {
    async fn dial(&self, target: &AuthorizedTarget) -> io::Result<BoxDuplex> {
        let port = target.port();
        if let Some(address) = target.address() {
            return Ok(Box::new(TcpStream::connect((address, port)).await?));
        }
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => Ok(Box::new(stream)),
            Err(v4) => match TcpStream::connect(("::1", port)).await {
                Ok(stream) => Ok(Box::new(stream)),
                Err(_) => Err(v4),
            },
        }
    }
}

/// The box daemon's pieces that `serve` delegates to.
#[derive(Clone)]
pub struct BoxServices {
    /// Registry snapshot and changes; also the authorization source for dials.
    pub registry: Arc<dyn RegistrySource>,
    /// Connects authorized data streams.
    pub dialer: Arc<dyn Dialer>,
    /// Sync feeds; use [`crate::NoSync`] when unsupported.
    pub sync: Arc<dyn SyncSource>,
    /// Which clients are served; use [`crate::NoPairing`] to serve every one.
    pub pairing: Arc<dyn PairingGate>,
}

/// Box-side session settings.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Version label sent to clients for diagnostics, such as `devlanes 0.1.0`.
    pub server: String,
    /// Features this box implements; defaults to [`FEATURES`].
    pub features: Vec<String>,
    /// What clients learn about the box in the handshake.
    pub box_info: BoxInfo,
    /// Ping cadence; the box closes after `ping_interval + ping_timeout` of silence.
    pub timing: Timing,
    /// Limit for connecting a data stream's target.
    pub dial_timeout: Duration,
    /// How long a client asked to pair may take to send its code: a person
    /// types it, so this is longer than the handshake limit.
    pub pairing_timeout: Duration,
}

impl ServeOptions {
    /// Default timing, this build's features, no box info and 150 seconds to
    /// pair: the two minutes devlanes' client gives a person to type the code,
    /// and time for the code to travel. Shorter than a client's whole
    /// connection attempt, so an unpaired client holds the tunnel no longer
    /// than it needs to. With the given version label.
    pub fn new(server: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            features: FEATURES.iter().map(|name| (*name).to_owned()).collect(),
            box_info: BoxInfo::default(),
            timing: Timing::default(),
            dial_timeout: Duration::from_secs(10),
            pairing_timeout: Duration::from_secs(150),
        }
    }
}

/// A connected client as its handshake describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
    /// Version label, such as `devlanes 0.5.0`.
    pub label: String,
    /// Random id the client machine keeps across restarts; empty when it sent none.
    pub id: String,
    /// The client machine's host name; empty when it sent none.
    pub name: String,
    /// Features both ends implement, which this session may use.
    pub features: Vec<String>,
}

impl ClientInfo {
    /// Whether this session may use `feature`.
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|name| name == feature)
    }
}

/// Why a box-side session ended abnormally.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// Stream framing or I/O failed on the control stream.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The client did not complete the handshake in time.
    #[error("client did not complete the handshake in time")]
    HandshakeTimeout,
    /// The client broke the protocol during the handshake.
    #[error("handshake failed: {0}")]
    Handshake(&'static str),
    /// The client speaks another protocol revision; it was told so.
    #[error("client {client} speaks protocol {protocol}, this box speaks {PROTOCOL_VERSION}")]
    IncompatibleClient {
        /// The client's protocol revision.
        protocol: u32,
        /// The client's version label.
        client: String,
    },
    /// The registry source failed to subscribe or to deliver a change.
    #[error("registry source: {0}")]
    Registry(BoxError),
    /// The registry source ended its update stream.
    #[error("registry source closed")]
    RegistryClosed,
    /// No ping arrived within the idle limit, or a control write stalled.
    #[error("client stopped responding")]
    ClientTimeout,
    /// The pairing gate failed to check a client or to trade its code.
    #[error("pairing: {0}")]
    Pairing(BoxError),
    /// The client is not paired and left without sending a code.
    #[error("client {id} is not paired with this box and sent no code")]
    NotPaired {
        /// The client's id.
        id: String,
    },
    /// The client's code was refused; it was told why.
    #[error("refused the pairing code of client {id}: {message}")]
    PairingRefused {
        /// The client's id.
        id: String,
        /// What the client was told.
        message: String,
    },
}

/// State shared by per-stream tasks of one session.
struct StreamContext {
    registry: Arc<RwLock<Registry>>,
    services: BoxServices,
    handshake_timeout: Duration,
    dial_timeout: Duration,
    /// Whether sync chunks may go out compressed, as negotiated with this client.
    sync_zstd: bool,
    /// Open data streams, closed when their target stops being authorized.
    open: Mutex<HashMap<u64, (DialTarget, CancellationToken)>>,
    next_stream: AtomicU64,
}

impl StreamContext {
    /// Closes data streams whose unit was hidden, removed or stopped.
    fn revoke(&self) {
        let registry = self.registry.read().unwrap_or_else(PoisonError::into_inner);
        for (target, cancel) in self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            if registry.authorize(target).is_err() {
                cancel.cancel();
            }
        }
    }
}

/// Removes a data stream from [`StreamContext::open`] when it ends.
struct OpenGuard<'a> {
    context: &'a StreamContext,
    id: u64,
}

impl Drop for OpenGuard<'_> {
    /// Forgets the stream.
    fn drop(&mut self) {
        self.context
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// Runs one box-side session over `io` until the client leaves.
///
/// The first stream must be the control stream. Returns `Ok(())` when the
/// client closes the session; per-stream tasks are aborted on return.
pub async fn serve<S>(io: S, services: BoxServices, options: ServeOptions) -> Result<(), ServeError>
where
    S: Duplex + 'static,
{
    let timing = options.timing;
    let mut mux = mux::server(Box::new(io));
    let first = timeout(timing.handshake_timeout, mux.accept())
        .await
        .map_err(|_| ServeError::HandshakeTimeout)?
        .ok_or(ServeError::Handshake(
            "connection closed before the control stream",
        ))?;
    let mut control = wire::channel(first);
    let open = timeout(
        timing.handshake_timeout,
        wire::recv::<_, OpenRequest>(&mut control),
    )
    .await
    .map_err(|_| ServeError::HandshakeTimeout)??;
    let Some((
        OpenRequest::Control {
            protocol,
            client,
            features,
            client_id,
            client_name,
            token,
        },
        _,
    )) = open
    else {
        return Err(ServeError::Handshake(
            "first stream must be the control stream",
        ));
    };
    if protocol != PROTOCOL_VERSION {
        // No version label: an unpaired client learns the revision only.
        let reply = BoxControl::Incompatible {
            protocol: PROTOCOL_VERSION,
            server: String::new(),
        };
        let _ = timeout(timing.handshake_timeout, async {
            wire::send(&mut control, &reply).await?;
            control.close().await
        })
        .await;
        return Err(ServeError::IncompatibleClient { protocol, client });
    }
    let peer = ClientInfo {
        features: negotiate(&options.features, &features),
        label: client,
        id: client_id,
        name: client_name,
    };
    // Nothing about the box, not even its welcome, goes to a client it does
    // not serve, and nothing it opens before then is answered.
    refusing_streams(
        &mut mux,
        pair(
            &mut control,
            &services,
            &peer,
            token.map(|token| token.0),
            &options,
        ),
    )
    .await?;
    let feed = services
        .registry
        .subscribe(&peer)
        .await
        .map_err(ServeError::Registry)?;
    let registry = Arc::new(RwLock::new(Registry::from_snapshot(feed.snapshot.clone())));
    let welcome = BoxControl::Welcome {
        protocol: PROTOCOL_VERSION,
        server: options.server.clone(),
        features: options.features.clone(),
        box_info: options.box_info.clone(),
    };
    send_control(&mut control, &welcome, timing).await?;
    // The box's own copy above keeps hidden units, so "hidden wins" holds
    // in `authorize`; the client never learns of them.
    let snapshot = BoxControl::Snapshot {
        snapshot: feed.snapshot.without_hidden(),
    };
    send_control(&mut control, &snapshot, timing).await?;
    tracing::debug!(
        client = %peer.label,
        id = %peer.id,
        name = %peer.name,
        features = ?peer.features,
        "client connected"
    );

    let context = Arc::new(StreamContext {
        registry: registry.clone(),
        services,
        handshake_timeout: timing.handshake_timeout,
        dial_timeout: options.dial_timeout,
        sync_zstd: peer.has_feature(SYNC_ZSTD),
        open: Mutex::new(HashMap::new()),
        next_stream: AtomicU64::new(0),
    });
    let idle_limit = timing.ping_interval + timing.ping_timeout;
    let mut deadline = Instant::now() + idle_limit;
    let mut updates = feed.updates;
    let mut tasks = JoinSet::new();
    // Handed over by one task, latest first, so a slow source never holds up
    // pings and a burst of reports costs one hand-over.
    let reports = watch::Sender::new(None::<Value>);
    if peer.has_feature(MIRROR_REPORT) {
        tasks.spawn(forward_reports(
            reports.subscribe(),
            context.services.registry.clone(),
            peer.clone(),
        ));
    }
    loop {
        tokio::select! {
            stream = mux.accept() => match stream {
                Some(stream) => {
                    tasks.spawn(handle_stream(stream, context.clone()));
                }
                None => return Ok(()),
            },
            message = wire::recv::<_, ClientControl>(&mut control) => match message? {
                Some((ClientControl::Ping { nonce }, _)) => {
                    deadline = Instant::now() + idle_limit;
                    send_control(&mut control, &BoxControl::Pong { nonce }, timing).await?;
                }
                Some((ClientControl::Mirror { report }, _)) => {
                    // Without the feature nobody forwards it; the client should not send it.
                    reports.send_replace(Some(report));
                }
                Some((ClientControl::Pair { .. }, _)) => {
                    return Err(ServeError::Handshake("a pairing code after the welcome"));
                }
                None => return Ok(()),
            },
            update = updates.next() => {
                // Authorization switches to the new state before the client hears about it.
                let message = match update {
                    Some(Ok(RegistryUpdate::Event(event))) => {
                        write(&registry).apply(event.clone());
                        BoxControl::Event { event: event.without_hidden() }
                    }
                    Some(Ok(RegistryUpdate::Resync(snapshot))) => {
                        write(&registry).replace(snapshot.clone());
                        BoxControl::Snapshot { snapshot: snapshot.without_hidden() }
                    }
                    Some(Err(error)) => return Err(ServeError::Registry(error)),
                    None => return Err(ServeError::RegistryClosed),
                };
                context.revoke();
                send_control(&mut control, &message, timing).await?;
            }
            () = tokio::time::sleep_until(deadline) => {
                // Any inbound frame counts: pings can queue behind a large upload.
                let alive_until = mux.last_inbound() + idle_limit;
                if alive_until <= Instant::now() {
                    return Err(ServeError::ClientTimeout);
                }
                deadline = alive_until;
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

/// Runs `work` while resetting every stream the client opens meanwhile, by
/// dropping it unanswered: until the welcome a client may only use its
/// control stream, and a client that is not paired yet gets nothing else.
async fn refusing_streams<T>(mux: &mut MuxServer, work: impl Future<Output = T>) -> T {
    tokio::pin!(work);
    let mut open = true;
    loop {
        tokio::select! {
            done = &mut work => return done,
            stream = mux.accept(), if open => match stream {
                Some(stream) => {
                    tracing::debug!("reset a stream the client opened before the welcome");
                    drop(stream);
                }
                None => open = false,
            },
        }
    }
}

/// Lets the session go on once the pairing gate serves `peer`: at once for a
/// valid `token` (or a box that pairs nobody), else after the client traded a
/// one-time code, which it gets [`ServeOptions::pairing_timeout`] to send.
/// The new token goes to the client before the welcome. A refused code and a
/// client that leaves without one end the session.
async fn pair(
    control: &mut Channel<MuxStream>,
    services: &BoxServices,
    peer: &ClientInfo,
    token: Option<String>,
    options: &ServeOptions,
) -> Result<(), ServeError> {
    let timing = options.timing;
    let reason = match services
        .pairing
        .check(peer, token.as_deref())
        .await
        .map_err(ServeError::Pairing)?
    {
        PairingCheck::Allowed => return Ok(()),
        PairingCheck::Required(reason) => reason,
    };
    send_control(control, &BoxControl::PairingRequired { reason }, timing).await?;
    let answer = timeout(
        options.pairing_timeout,
        wire::recv::<_, ClientControl>(control),
    )
    .await
    .map_err(|_| ServeError::HandshakeTimeout)??;
    let code = match answer {
        Some((ClientControl::Pair { code }, _)) => code.0,
        Some(_) => return Err(ServeError::Handshake("expected a pairing code")),
        None => {
            return Err(ServeError::NotPaired {
                id: peer.id.clone(),
            });
        }
    };
    match services
        .pairing
        .claim(peer, &code)
        .await
        .map_err(ServeError::Pairing)?
    {
        PairingClaim::Paired { token } => {
            let paired = BoxControl::Paired {
                token: wire::Secret(token),
            };
            send_control(control, &paired, timing).await
        }
        PairingClaim::Refused {
            message,
            retry_after,
        } => {
            let refused = BoxControl::PairingRefused {
                message: message.clone(),
                retry_after_ms: retry_after
                    .map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
            };
            let _ = timeout(timing.handshake_timeout, async {
                wire::send(control, &refused).await?;
                control.close().await
            })
            .await;
            Err(ServeError::PairingRefused {
                id: peer.id.clone(),
                message,
            })
        }
    }
}

/// Hands each newest mirror report of `client` to `source` until the session ends.
async fn forward_reports(
    mut reports: watch::Receiver<Option<Value>>,
    source: Arc<dyn RegistrySource>,
    client: ClientInfo,
) {
    while reports.changed().await.is_ok() {
        let report = reports.borrow_and_update().clone();
        if let Some(report) = report {
            source.report(&client, report).await;
        }
    }
}

/// Writes a control message; a write stalled past the ping timeout ends the session.
async fn send_control(
    control: &mut Channel<MuxStream>,
    message: &BoxControl,
    timing: Timing,
) -> Result<(), ServeError> {
    timeout(timing.ping_timeout, wire::send(control, message))
        .await
        .map_err(|_| ServeError::ClientTimeout)??;
    Ok(())
}

/// Write-locks the registry; poisoning cannot leave it half-updated, so it is ignored.
fn write(registry: &RwLock<Registry>) -> std::sync::RwLockWriteGuard<'_, Registry> {
    registry.write().unwrap_or_else(PoisonError::into_inner)
}

/// Reads the open frame of a non-control stream and dispatches it.
async fn handle_stream(stream: MuxStream, context: Arc<StreamContext>) {
    let mut channel = wire::channel(stream);
    let open = match timeout(
        context.handshake_timeout,
        wire::recv::<_, OpenRequest>(&mut channel),
    )
    .await
    {
        Ok(Ok(Some((open, _)))) => open,
        _ => return,
    };
    match open {
        OpenRequest::Control { .. } => tracing::warn!("ignoring a second control stream"),
        OpenRequest::Sync { request } => {
            let source = context.services.sync.clone();
            sync::serve_subscription(channel, request, source, context.sync_zstd).await;
        }
        OpenRequest::Data { target } => serve_data(channel, target, &context).await,
    }
}

/// Authorizes, dials and splices one data stream.
async fn serve_data(mut channel: Channel<MuxStream>, target: DialTarget, context: &StreamContext) {
    let authorized = context
        .registry
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .authorize(&target);
    let target = match authorized {
        Ok(target) => target,
        Err(reason) => {
            let message = match reason {
                RejectReason::Hidden => "target belongs to a hidden unit".to_owned(),
                RejectReason::NotRunning => "target's unit is not running".to_owned(),
                _ => "target is not registered on this box".to_owned(),
            };
            reject(&mut channel, reason, message).await;
            return;
        }
    };
    let id = context.next_stream.fetch_add(1, Ordering::Relaxed);
    let cancel = CancellationToken::new();
    context
        .open
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            id,
            (
                DialTarget::Port {
                    port: target.port(),
                },
                cancel.clone(),
            ),
        );
    let _open = OpenGuard { context, id };
    let dialed = timeout(context.dial_timeout, context.services.dialer.dial(&target)).await;
    let upstream = match dialed {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(error)) => {
            let message = format!("dial port {}: {error}", target.port());
            reject(&mut channel, RejectReason::DialFailed, message).await;
            return;
        }
        Err(_) => {
            let message = format!("dial port {} timed out", target.port());
            reject(&mut channel, RejectReason::DialFailed, message).await;
            return;
        }
    };
    if wire::send(&mut channel, &OpenReply::Accepted)
        .await
        .is_err()
    {
        return;
    }
    let downstream = wire::into_raw(channel);
    tokio::select! {
        spliced = crate::splice::splice(downstream, upstream, crate::splice::HALF_CLOSE_IDLE) => {
            if let Err(error) = spliced {
                tracing::debug!("data stream to port {} ended: {error}", target.port());
            }
        }
        () = cancel.cancelled() => {
            tracing::debug!("data stream to port {} closed: its unit stopped", target.port());
        }
    }
}

/// Tells the client why its open failed, then closes the stream.
async fn reject(channel: &mut Channel<MuxStream>, reason: RejectReason, message: String) {
    let _ = wire::send(channel, &OpenReply::Rejected { reason, message }).await;
    let _ = channel.close().await;
}
