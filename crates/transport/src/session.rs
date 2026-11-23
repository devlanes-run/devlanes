//! Client side: one session over one transport connection.

use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::SinkExt;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use crate::pairing::PairingClient;
use crate::registry::ClientRegistry;
use crate::sync::SyncSubscription;
use crate::transport::{
    BoxConnection, BoxDuplex, Connection, ConnectionError, Transport, TransportError,
};
use crate::wire::{
    self, BoxControl, BoxInfo, Channel, ClientControl, DialTarget, FEATURES, MIRROR_REPORT,
    OpenReply, OpenRequest, PROTOCOL_VERSION, PairingReason, Prefixed, RejectReason, SYNC_ZSTD,
    Secret, WireError, negotiate,
};

/// Liveness and handshake limits shared by both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How often the client pings.
    pub ping_interval: Duration,
    /// How long a ping may stay unanswered before the session is dead.
    pub ping_timeout: Duration,
    /// Limit for handshakes and for the first frame of each stream.
    pub handshake_timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(10),
            ping_timeout: Duration::from_secs(20),
            handshake_timeout: Duration::from_secs(15),
        }
    }
}

/// Client-side session settings.
#[derive(Clone)]
pub struct SessionOptions {
    /// Version label sent to the box for diagnostics.
    pub client: String,
    /// Random id of this machine, kept across restarts, by which the box lists it.
    pub client_id: String,
    /// This machine's host name, shown next to the id on the box.
    pub client_name: String,
    /// Features this client implements; defaults to [`FEATURES`].
    pub features: Vec<String>,
    /// Ping and handshake limits.
    pub timing: Timing,
    /// State replaced by each session's snapshot; share it across reconnects.
    pub registry: ClientRegistry,
    /// Limit for one whole attempt in a [`crate::Link`]: transport connect
    /// plus handshake, including the time a person takes to answer SSH
    /// prompts and the pairing code prompt.
    pub connect_timeout: Duration,
    /// The remote token for the box and the person with a pairing code;
    /// without one the client sends no token and cannot pair.
    pub pairing: Option<Arc<dyn PairingClient>>,
}

impl SessionOptions {
    /// Default timing, this build's features, no identity, no pairing, a fresh
    /// registry and a 3-minute attempt limit, enough for a person to answer
    /// SSH prompts.
    pub fn new(client: impl Into<String>) -> Self {
        Self {
            client: client.into(),
            client_id: String::new(),
            client_name: String::new(),
            features: FEATURES.iter().map(|name| (*name).to_owned()).collect(),
            timing: Timing::default(),
            registry: ClientRegistry::new(),
            connect_timeout: Duration::from_secs(180),
            pairing: None,
        }
    }
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// [`Session::close`] was called or every handle was dropped.
    Closed,
    /// The box closed the control stream.
    Disconnected,
    /// A ping went unanswered for [`Timing::ping_timeout`].
    PingTimeout,
    /// The control stream failed.
    Failed(String),
}

impl fmt::Display for SessionEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("session closed"),
            Self::Disconnected => f.write_str("box closed the session"),
            Self::PingTimeout => f.write_str("box stopped answering pings"),
            Self::Failed(reason) => write!(f, "session failed: {reason}"),
        }
    }
}

/// Failure to establish or use a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The transport could not produce a connection.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The connection could not open a stream.
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    /// Every stream slot is in use; this open failed but the session lives on.
    #[error("too many open streams ({MAX_STREAMS}); close some connections and retry")]
    TooManyStreams,
    /// A stream failed or carried invalid frames.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The box did not complete the handshake in time.
    #[error("box did not complete the handshake in time")]
    HandshakeTimeout,
    /// The whole connection attempt took longer than its limit.
    #[error("connecting to the box timed out")]
    ConnectTimeout,
    /// The box sent something unexpected during the handshake.
    #[error("handshake failed: {0}")]
    Handshake(&'static str),
    /// Client and box speak different protocol revisions.
    #[error(
        "{} but this client speaks protocol {local}. Install the same release on both machines, \
         then run `devlanes daemon restart` on the box",
        box_protocol(server, *remote)
    )]
    IncompatibleProtocol {
        /// This client's revision.
        local: u32,
        /// The box's revision.
        remote: u32,
        /// The box's version label.
        server: String,
    },
    /// The box side broke the connection; the text is the connection's own
    /// failure reason, such as the stderr of a remote `devlanes tunnel --stdio`
    /// that exited.
    #[error("box closed the connection: {0}")]
    Closed(String),
    /// The box serves only paired clients and nobody gave a pairing code.
    #[error("{}", needs_pairing_text(*.0))]
    NeedsPairing(PairingReason),
    /// The box takes no connections (`remote.allow_connections: false` there);
    /// the text is the box's reason.
    #[error("{0}")]
    Refused(String),
    /// The box refused the pairing code.
    #[error("{message}")]
    PairingRefused {
        /// The box's reason.
        message: String,
        /// How long the box refuses every code.
        retry_after: Option<Duration>,
    },
    /// The session already ended.
    #[error("{0}")]
    Ended(SessionEnd),
}

impl SessionError {
    /// Reports whether reconnecting without user action is pointless.
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Transport(error) => error.is_permanent(),
            // Retrying cannot help until someone changes the box's config.
            Self::IncompatibleProtocol { .. } | Self::Refused(_) => true,
            _ => false,
        }
    }

    /// Reports whether a person has to sign in again before reconnecting.
    pub fn is_needs_auth(&self) -> bool {
        matches!(self, Self::Transport(error) if error.is_needs_auth())
    }

    /// Reports whether a person has to pair this client with the box (with a
    /// new code) before reconnecting.
    pub fn is_needs_pairing(&self) -> bool {
        matches!(self, Self::NeedsPairing(_) | Self::PairingRefused { .. })
    }
}

/// The box's side of a protocol mismatch: its version label when it sent
/// one (a welcome of another revision), else the revision alone, which is
/// all a box's refusal before pairing says.
fn box_protocol(server: &str, remote: u32) -> String {
    if server.is_empty() {
        format!("the box speaks protocol {remote}")
    } else {
        format!("the box runs {server} (protocol {remote})")
    }
}

/// What [`SessionError::NeedsPairing`] says for `reason`.
fn needs_pairing_text(reason: PairingReason) -> &'static str {
    match reason {
        PairingReason::Missing => {
            "the box needs a pairing code from `devlanes daemon pair` on the box"
        }
        PairingReason::Invalid => {
            "the box no longer accepts this machine's pairing and needs a new code from \
             `devlanes daemon pair` on the box"
        }
    }
}

/// Failure to open a data stream.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The box refused the target.
    #[error("box refused the stream ({reason:?}): {message}")]
    Rejected {
        /// Refusal category.
        reason: RejectReason,
        /// Box-provided detail.
        message: String,
    },
    /// The session could not open a stream.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The open exchange failed.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The box did not answer in time.
    #[error("box did not answer the open in time")]
    Timeout,
    /// The stream closed before the box answered.
    #[error("stream closed before the box answered")]
    Interrupted,
}

/// A raw byte stream to a box endpoint.
pub type DataStream = Prefixed<BoxDuplex>;

/// Streams per session on every transport; browsers plus database pools rarely
/// exceed this. Transports keep at least this many open at once.
pub const MAX_STREAMS: usize = 512;

/// The session's connection and the streams it counts against [`MAX_STREAMS`].
#[derive(Clone)]
struct Streams {
    connection: Arc<dyn Connection>,
    open: Arc<AtomicUsize>,
}

impl Streams {
    /// Counts no streams yet on `connection`.
    fn new(connection: BoxConnection) -> Self {
        Self {
            connection: Arc::from(connection),
            open: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Opens a stream that holds a slot until it drops. A full table fails this
    /// open with [`SessionError::TooManyStreams`]; the connection and its other
    /// streams go on.
    async fn open(&self) -> Result<BoxDuplex, SessionError> {
        let taken = self.open.fetch_add(1, Ordering::AcqRel);
        let slot = Slot(self.open.clone());
        if taken >= MAX_STREAMS {
            return Err(SessionError::TooManyStreams);
        }
        let stream = self.connection.open().await?;
        Ok(Box::new(Counted {
            inner: stream,
            _slot: slot,
        }))
    }
}

/// A stream slot, released when the stream drops.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    /// Frees the slot.
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A stream that holds its [`Slot`]; reads and writes go straight through.
struct Counted {
    inner: BoxDuplex,
    _slot: Slot,
}

impl AsyncRead for Counted {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Counted {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    /// Half-closes the stream; reading continues until the peer closes.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// A live client session. Clones share it; dropping the last clone closes it.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    streams: Streams,
    welcome: Welcome,
    registry: ClientRegistry,
    timing: Timing,
    end: watch::Receiver<Option<SessionEnd>>,
    cancel: CancellationToken,
    /// The latest mirror report, which the control task sends once.
    report: watch::Sender<Option<Value>>,
    /// The latest answered ping's round trip.
    round_trip: watch::Receiver<Option<Duration>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.streams.connection.shutdown();
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("server", &self.inner.welcome.server)
            .field("features", &self.inner.welcome.features)
            .field("end", &*self.inner.end.borrow())
            .finish()
    }
}

/// What the box's welcome told this session.
struct Welcome {
    server: String,
    /// Features both ends list.
    features: Vec<String>,
    box_info: BoxInfo,
}

impl Session {
    /// Connects through `transport` and performs the handshake.
    pub async fn connect(
        transport: &dyn Transport,
        options: SessionOptions,
    ) -> Result<Self, SessionError> {
        let connection = transport.connect().await?;
        Self::establish(connection, options).await
    }

    /// Runs the handshake over an existing connection.
    ///
    /// Returns after the box's snapshot replaced `options.registry`, so callers
    /// see current state immediately.
    pub async fn establish(
        connection: BoxConnection,
        options: SessionOptions,
    ) -> Result<Self, SessionError> {
        let streams = Streams::new(connection);
        match handshake(&streams, &options).await {
            Ok((control, welcome)) => {
                let (end_tx, end) = watch::channel(None);
                let cancel = CancellationToken::new();
                let report = watch::Sender::new(None);
                let (round_trip_tx, round_trip) = watch::channel(None);
                tokio::spawn(run_control(
                    control,
                    Control {
                        registry: options.registry.clone(),
                        timing: options.timing,
                        cancel: cancel.clone(),
                        end_tx,
                        connection: streams.connection.clone(),
                        reports: report.subscribe(),
                        round_trip: round_trip_tx,
                    },
                ));
                Ok(Self {
                    inner: Arc::new(Inner {
                        streams,
                        welcome,
                        registry: options.registry,
                        timing: options.timing,
                        end,
                        cancel,
                        report,
                        round_trip,
                    }),
                })
            }
            Err(error) => {
                streams.connection.shutdown();
                Err(error)
            }
        }
    }

    /// The box's version label from the handshake.
    pub fn server(&self) -> &str {
        &self.inner.welcome.server
    }

    /// Features both ends list, which this session may use.
    pub fn features(&self) -> &[String] {
        &self.inner.welcome.features
    }

    /// Whether both ends list `feature`.
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features().iter().any(|name| name == feature)
    }

    /// What the box reported about itself in the handshake.
    pub fn box_info(&self) -> &BoxInfo {
        &self.inner.welcome.box_info
    }

    /// The registry this session keeps up to date.
    pub fn registry(&self) -> &ClientRegistry {
        &self.inner.registry
    }

    /// How long the latest answered ping took to come back; `None` before the
    /// first pong. A pong queued behind a large transfer reads high.
    pub fn round_trip(&self) -> Option<Duration> {
        *self.inner.round_trip.borrow()
    }

    /// Opens a raw byte stream to `target`; the box rejects unregistered targets.
    pub async fn open(&self, target: DialTarget) -> Result<DataStream, OpenError> {
        let mut channel = self.open_stream(&OpenRequest::Data { target }).await?;
        let reply = timeout(
            self.inner.timing.handshake_timeout,
            wire::recv::<_, OpenReply>(&mut channel),
        )
        .await
        .map_err(|_| OpenError::Timeout)??;
        match reply {
            Some((OpenReply::Accepted, _)) => Ok(wire::into_raw(channel)),
            Some((OpenReply::Rejected { reason, message }, _)) => {
                Err(OpenError::Rejected { reason, message })
            }
            None => Err(OpenError::Interrupted),
        }
    }

    /// Starts a sync subscription; the request is opaque to the transport.
    pub async fn subscribe_sync(&self, request: Value) -> Result<SyncSubscription, SessionError> {
        let channel = self.open_stream(&OpenRequest::Sync { request }).await?;
        Ok(SyncSubscription::new(channel, self.has_feature(SYNC_ZSTD)))
    }

    /// Tells the box what this client mirrors of it, when both ends list
    /// [`MIRROR_REPORT`]; false otherwise. The report is opaque to the
    /// transport. A report equal to the last one of this session is not sent
    /// again, and reports given faster than the control stream takes them
    /// replace each other, so callers may report after every change.
    pub fn report(&self, report: Value) -> bool {
        if !self.has_feature(MIRROR_REPORT) {
            return false;
        }
        self.inner.report.send_if_modified(|current| {
            if current.as_ref() == Some(&report) {
                return false;
            }
            *current = Some(report);
            true
        });
        true
    }

    /// Returns the end reason once the session ended.
    pub fn end(&self) -> Option<SessionEnd> {
        self.inner.end.borrow().clone()
    }

    /// Waits until the session ends.
    pub async fn closed(&self) -> SessionEnd {
        let mut end = self.inner.end.clone();
        match end.wait_for(Option::is_some).await {
            Ok(end) => end.clone().unwrap_or(SessionEnd::Closed),
            // The control task only drops the sender after publishing an end.
            Err(_) => SessionEnd::Closed,
        }
    }

    /// Ends the session and every stream on it.
    pub fn close(&self) {
        self.inner.cancel.cancel();
    }

    /// Opens a stream and sends its open frame.
    async fn open_stream(&self, open: &OpenRequest) -> Result<Channel<BoxDuplex>, SessionError> {
        if let Some(end) = self.end() {
            return Err(SessionError::Ended(end));
        }
        let stream = match self.inner.streams.open().await {
            Ok(stream) => stream,
            Err(error) => return Err(self.end().map_or(error, SessionError::Ended)),
        };
        let mut channel = wire::channel(stream);
        wire::send(&mut channel, open).await?;
        Ok(channel)
    }
}

/// Opens the control stream, checks the protocol, pairs when the box asks for
/// it, negotiates features and applies the first snapshot.
///
/// Each exchange with the box has the handshake limit. Asking a person for a
/// pairing code does not: the [`PairingClient`] bounds it, and a
/// [`crate::Link`] bounds the whole attempt.
async fn handshake(
    streams: &Streams,
    options: &SessionOptions,
) -> Result<(Channel<BoxDuplex>, Welcome), SessionError> {
    let connection = &*streams.connection;
    let limit = options.timing.handshake_timeout;
    let token = match &options.pairing {
        Some(pairing) => pairing.token().await,
        None => None,
    };
    let (mut control, first) = within(connection, limit, async {
        let mut control = wire::channel(streams.open().await?);
        let hello = OpenRequest::Control {
            protocol: PROTOCOL_VERSION,
            client: options.client.clone(),
            features: options.features.clone(),
            client_id: options.client_id.clone(),
            client_name: options.client_name.clone(),
            token: token.map(Secret),
        };
        wire::send(&mut control, &hello).await?;
        let first = wire::recv::<_, BoxControl>(&mut control).await?;
        Ok((control, first))
    })
    .await?;
    let first = match first {
        Some((BoxControl::PairingRequired { reason }, _)) => {
            pair(connection, &mut control, options, reason).await?;
            None
        }
        other => Some(other),
    };
    let welcome = within(connection, limit, async {
        let first = match first {
            Some(first) => first,
            None => wire::recv::<_, BoxControl>(&mut control).await?,
        };
        let welcome = match first {
            Some((
                BoxControl::Welcome {
                    protocol,
                    server,
                    features,
                    box_info,
                },
                _,
            )) if protocol == PROTOCOL_VERSION => Welcome {
                server,
                features: negotiate(&options.features, &features),
                box_info,
            },
            Some((
                BoxControl::Welcome {
                    protocol, server, ..
                }
                | BoxControl::Incompatible { protocol, server },
                _,
            )) => {
                return Err(SessionError::IncompatibleProtocol {
                    local: PROTOCOL_VERSION,
                    remote: protocol,
                    server,
                });
            }
            Some((BoxControl::Refused { message }, _)) => {
                return Err(SessionError::Refused(message));
            }
            Some(_) => return Err(SessionError::Handshake("expected welcome")),
            None => return Err(SessionError::Handshake("box closed the control stream")),
        };
        match wire::recv::<_, BoxControl>(&mut control).await? {
            Some((BoxControl::Snapshot { snapshot }, _)) => options.registry.replace(snapshot),
            Some(_) => return Err(SessionError::Handshake("expected registry snapshot")),
            None => return Err(SessionError::Handshake("box closed the control stream")),
        }
        Ok(welcome)
    })
    .await?;
    Ok((control, welcome))
}

/// Answers the box's request to pair: asks the [`PairingClient`] for a code,
/// sends it and keeps the token the box sends back. Without a code the
/// session needs pairing; a refused code says why.
async fn pair(
    connection: &dyn Connection,
    control: &mut Channel<BoxDuplex>,
    options: &SessionOptions,
    reason: PairingReason,
) -> Result<(), SessionError> {
    let Some(pairing) = &options.pairing else {
        return Err(SessionError::NeedsPairing(reason));
    };
    let Some(code) = pairing
        .code(reason)
        .await
        .filter(|code| !code.trim().is_empty())
    else {
        return Err(SessionError::NeedsPairing(reason));
    };
    let token = within(connection, options.timing.handshake_timeout, async {
        let message = ClientControl::Pair { code: Secret(code) };
        wire::send(control, &message).await?;
        match wire::recv::<_, BoxControl>(control).await? {
            Some((BoxControl::Paired { token }, _)) => Ok(token.0),
            Some((
                BoxControl::PairingRefused {
                    message,
                    retry_after_ms,
                },
                _,
            )) => Err(SessionError::PairingRefused {
                message,
                retry_after: retry_after_ms.map(Duration::from_millis),
            }),
            Some(_) => Err(SessionError::Handshake("expected the pairing answer")),
            None => Err(SessionError::Handshake("box closed the control stream")),
        }
    })
    .await?;
    // The box has the pairing now; losing the token only costs a new code
    // on the next connection, so this session goes on.
    if let Err(error) = pairing.store(token).await {
        tracing::warn!(%error, "cannot keep the box's remote token");
    }
    Ok(())
}

/// Runs one handshake exchange within `limit`. A failure other than the
/// box's own answer (a protocol mismatch, a refusal, a refused pairing code) is replaced
/// by the connection's failure reason when it failed, which usually says why.
async fn within<T>(
    connection: &dyn Connection,
    limit: Duration,
    exchange: impl std::future::Future<Output = Result<T, SessionError>>,
) -> Result<T, SessionError> {
    match timeout(limit, exchange).await {
        Err(_) => Err(SessionError::HandshakeTimeout),
        Ok(Err(
            error @ (SessionError::IncompatibleProtocol { .. }
            | SessionError::Refused(_)
            | SessionError::PairingRefused { .. }),
        )) => Err(error),
        Ok(Err(error)) => match connection.failure_within(FAILURE_GRACE).await {
            Some(reason) => Err(SessionError::Closed(reason)),
            None => Err(error),
        },
        Ok(Ok(value)) => Ok(value),
    }
}

/// How long a lost stream waits for the connection to report why it failed.
const FAILURE_GRACE: Duration = Duration::from_secs(1);

/// Ends a session that lost its control stream, preferring the connection's
/// failure reason.
async fn lost(connection: &dyn Connection) -> SessionEnd {
    match connection.failure_within(FAILURE_GRACE).await {
        Some(reason) => SessionEnd::Failed(reason),
        None => SessionEnd::Disconnected,
    }
}

/// What the control task of one session works with.
struct Control {
    registry: ClientRegistry,
    timing: Timing,
    cancel: CancellationToken,
    end_tx: watch::Sender<Option<SessionEnd>>,
    connection: Arc<dyn Connection>,
    /// Mirror reports to send, latest only.
    reports: watch::Receiver<Option<Value>>,
    /// Set from each answered ping.
    round_trip: watch::Sender<Option<Duration>>,
}

/// Applies registry changes, pings and sends mirror reports until the
/// session ends.
async fn run_control(mut control: Channel<BoxDuplex>, parts: Control) {
    let Control {
        registry,
        timing,
        cancel,
        end_tx,
        connection,
        mut reports,
        round_trip,
    } = parts;
    let mut ticker =
        tokio::time::interval_at(Instant::now() + timing.ping_interval, timing.ping_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut nonce = 0u64;
    let mut outstanding: Option<(u64, Instant)> = None;
    let end = loop {
        // Any inbound frame proves the box is alive: a pong queued behind a large
        // transfer must not end the session.
        let deadline =
            outstanding.map(|(_, sent)| sent.max(connection.last_inbound()) + timing.ping_timeout);
        tokio::select! {
            () = cancel.cancelled() => break SessionEnd::Closed,
            () = connection.closed() => break lost(&*connection).await,
            message = wire::recv::<_, BoxControl>(&mut control) => match message {
                Ok(Some((BoxControl::Snapshot { snapshot }, _))) => registry.replace(snapshot),
                Ok(Some((BoxControl::Event { event }, _))) => registry.apply(event),
                Ok(Some((BoxControl::Pong { nonce: answered }, _))) => {
                    if let Some((waiting, sent)) = outstanding
                        && waiting == answered
                    {
                        round_trip.send_replace(Some(sent.elapsed()));
                        outstanding = None;
                    }
                }
                Ok(Some(_)) => break SessionEnd::Failed("unexpected control message".into()),
                Ok(None) => break lost(&*connection).await,
                Err(error) => break match connection.failure_within(FAILURE_GRACE).await {
                    Some(reason) => SessionEnd::Failed(reason),
                    None => SessionEnd::Failed(error.to_string()),
                },
            },
            _ = ticker.tick(), if outstanding.is_none() => {
                nonce += 1;
                let sent = Instant::now();
                let ping = ClientControl::Ping { nonce };
                // A stalled write counts against the same budget as a missing pong.
                match timeout(timing.ping_timeout, wire::send(&mut control, &ping)).await {
                    Ok(Ok(())) => outstanding = Some((nonce, sent)),
                    Ok(Err(error)) => break SessionEnd::Failed(error.to_string()),
                    Err(_) => break SessionEnd::PingTimeout,
                }
            }
            Ok(()) = reports.changed() => {
                let report = reports.borrow_and_update().clone();
                let Some(report) = report else { continue };
                let message = ClientControl::Mirror { report };
                // A stalled write is as dead as a stalled ping.
                match timeout(timing.ping_timeout, wire::send(&mut control, &message)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => break SessionEnd::Failed(error.to_string()),
                    Err(_) => break SessionEnd::PingTimeout,
                }
            }
            () = sleep_until_opt(deadline) => {
                let silent_since = outstanding.map(|(_, sent)| sent.max(connection.last_inbound()));
                if silent_since.is_some_and(|since| since + timing.ping_timeout > Instant::now()) {
                    continue;
                }
                break SessionEnd::PingTimeout;
            }
        }
    };
    tracing::debug!("session ended: {end}");
    end_tx.send_replace(Some(end));
    let _ = timeout(Duration::from_millis(200), control.close()).await;
    connection.shutdown();
}

/// Sleeps until `deadline`, or forever when there is none.
async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::mux::{self, mux_client, mux_server};
    use crate::registry::{RegistrySnapshot, SharedRegistry};
    use crate::server::{BoxServices, LoopbackDialer, ServeError, ServeOptions, serve};
    use crate::sync::NoSync;
    use crate::transport::ServerConnection;

    fn services() -> BoxServices {
        BoxServices {
            registry: Arc::new(SharedRegistry::new(RegistrySnapshot::default(), 4)),
            dialer: Arc::new(LoopbackDialer),
            sync: Arc::new(NoSync),
            pairing: Arc::new(crate::NoPairing),
        }
    }

    /// A welcome for this protocol without features or box info.
    fn welcome() -> BoxControl {
        BoxControl::Welcome {
            protocol: PROTOCOL_VERSION,
            server: "box".into(),
            features: Vec::new(),
            box_info: BoxInfo::default(),
        }
    }

    /// The clock is paused so a descheduled runner cannot stretch a gap between
    /// two frames past the ping timeout. It moves only when every task waits,
    /// to the earliest timer, which is the box's next frame while it streams.
    /// The pipe is in memory, so a hang leaves every task waiting and the outer
    /// timeout fires at once.
    #[tokio::test(start_paused = true)]
    async fn data_keeps_a_session_alive_without_pongs() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(10), async {
            let (client_io, box_io) = tokio::io::duplex(1024 * 1024);
            // A box that never answers pings but streams one sync feed for a while,
            // like a large transfer that queues pongs behind data.
            let streaming = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let flag = streaming.clone();
            let fake_box = tokio::spawn(async move {
                let mut mux = mux::server(Box::new(box_io));
                let mut control = wire::channel(mux.accept().await.ok_or("no control")?);
                let _hello = wire::recv::<_, OpenRequest>(&mut control).await?;
                wire::send(&mut control, &welcome()).await?;
                let snapshot = BoxControl::Snapshot {
                    snapshot: RegistrySnapshot::default(),
                };
                wire::send(&mut control, &snapshot).await?;
                let mut feed = wire::channel(mux.accept().await.ok_or("no sync stream")?);
                let started = Instant::now();
                while started.elapsed() < Duration::from_millis(800) {
                    let item = crate::wire::SyncFrame::Manifest {
                        manifest: Value::Null,
                    };
                    wire::send(&mut feed, &item).await?;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                flag.store(false, std::sync::atomic::Ordering::SeqCst);
                // Hold everything open, reading pings without answering, until the
                // client gives up.
                while let Ok(Some(_)) = wire::recv::<_, Value>(&mut control).await {}
                drop(feed);
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
            });
            let mut options = SessionOptions::new("client");
            options.timing = Timing {
                ping_interval: Duration::from_millis(50),
                ping_timeout: Duration::from_millis(150),
                handshake_timeout: Duration::from_secs(5),
            };
            let session = Session::establish(mux_client(Box::new(client_io)), options).await?;
            let mut feed = session.subscribe_sync(Value::Null).await?;
            let reader = tokio::spawn(async move { while let Some(Ok(_)) = feed.next().await {} });
            while streaming.load(std::sync::atomic::Ordering::SeqCst) {
                assert_eq!(session.end(), None, "session died while data was flowing");
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let end = timeout(Duration::from_secs(2), session.closed()).await?;
            assert_eq!(end, SessionEnd::PingTimeout);
            reader.abort();
            fake_box.abort();
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_full_stream_table_fails_only_that_open() -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        timeout(Duration::from_secs(20), async {
            let (a, b) = tokio::io::duplex(1024 * 1024);
            let streams = Streams::new(mux_client(Box::new(a)));
            let mut server = mux::server(Box::new(b));
            // Like the real box, answer every stream: yamux stops opening streams
            // while too many wait for their first reply (ACK).
            let (held_tx, mut held) = tokio::sync::mpsc::unbounded_channel();
            let answering = tokio::spawn(async move {
                while let Some(mut stream) = server.accept().await {
                    if stream.write_all(b"k").await.is_err() {
                        break;
                    }
                    if held_tx.send(stream).is_err() {
                        break;
                    }
                }
            });
            let mut open = Vec::new();
            let refused = loop {
                match streams.open().await {
                    Ok(mut stream) => {
                        // yamux announces a stream with its first frame, as the open
                        // request does in the real protocol.
                        stream.write_all(b"o").await?;
                        let mut ack = [0u8; 1];
                        stream.read_exact(&mut ack).await?;
                        open.push(stream);
                    }
                    Err(error) => break error,
                }
                assert!(
                    open.len() <= MAX_STREAMS,
                    "the stream limit was not enforced"
                );
            };
            assert!(
                matches!(refused, SessionError::TooManyStreams),
                "{refused:?}"
            );
            // The first stream still carries data, and freed slots open again.
            open[0].write_all(b"alive").await?;
            let mut inbound = held.recv().await.ok_or("no inbound stream")?;
            let mut opened = [0u8; 1];
            inbound.read_exact(&mut opened).await?;
            let mut buf = [0u8; 5];
            inbound.read_exact(&mut buf).await?;
            assert_eq!(&buf, b"alive");
            // Both ends must close a stream before yamux frees its slot.
            drop(inbound);
            open.clear();
            while let Ok(Some(stream)) = timeout(Duration::from_millis(50), held.recv()).await {
                drop(stream);
            }
            let reopened = timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(mut stream) = streams.open().await
                        && stream.write_all(b"o").await.is_ok()
                    {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap_or(false);
            assert!(reopened, "a closed stream frees its slot");
            answering.abort();
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn box_refuses_other_protocol_revisions() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let (client_io, box_io) = tokio::io::duplex(64 * 1024);
            let server = tokio::spawn(serve(
                mux_server(Box::new(box_io)),
                services(),
                ServeOptions::new("box 9"),
            ));
            let mux = mux::client(Box::new(client_io));
            let mut control = wire::channel(mux.open().await?);
            let hello = OpenRequest::Control {
                protocol: PROTOCOL_VERSION + 1,
                client: "future".into(),
                features: Vec::new(),
                client_id: String::new(),
                client_name: String::new(),
                token: None,
            };
            wire::send(&mut control, &hello).await?;
            let reply = wire::recv::<_, BoxControl>(&mut control).await?;
            // The revision only: no version label before pairing.
            assert!(
                matches!(
                    &reply,
                    Some((
                        BoxControl::Incompatible {
                            protocol: PROTOCOL_VERSION,
                            server,
                        },
                        _
                    )) if server.is_empty()
                ),
                "{reply:?}"
            );
            let error = SessionError::IncompatibleProtocol {
                local: PROTOCOL_VERSION + 1,
                remote: PROTOCOL_VERSION,
                server: String::new(),
            };
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("the box speaks protocol {PROTOCOL_VERSION} but")),
                "{error}"
            );
            let result = server.await?;
            assert!(matches!(result, Err(ServeError::IncompatibleClient { .. })));
            Ok(())
        })
        .await?
    }

    /// A pipe whose reads fail like an SSH tunnel that exited with an error.
    struct DeadTunnel;

    impl tokio::io::AsyncRead for DeadTunnel {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "remote `devlanes tunnel --stdio` exited with 2: error: not_supported_yet",
            )))
        }
    }

    impl tokio::io::AsyncWrite for DeadTunnel {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn handshake_surfaces_the_pipe_error() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let result =
                Session::establish(mux_client(Box::new(DeadTunnel)), SessionOptions::new("c"))
                    .await;
            let Err(error) = result else {
                panic!("handshake over a dead pipe succeeded");
            };
            assert!(matches!(error, SessionError::Closed(_)), "{error:?}");
            assert!(error.to_string().contains("not_supported_yet"), "{error}");
            Ok(())
        })
        .await?
    }

    /// A box gate that serves `TOKEN` and trades `CODE` for it.
    #[derive(Default)]
    struct OneCode {
        claims: std::sync::atomic::AtomicU32,
    }

    const TOKEN: &str = "dl_rt_test";
    const CODE: &str = "ABCD-EFGH";

    #[async_trait::async_trait]
    impl crate::PairingGate for OneCode {
        async fn check(
            &self,
            _: &crate::ClientInfo,
            token: Option<&str>,
        ) -> Result<crate::PairingCheck, crate::BoxError> {
            Ok(match token {
                Some(TOKEN) => crate::PairingCheck::Allowed,
                Some(_) => crate::PairingCheck::Required(PairingReason::Invalid),
                None => crate::PairingCheck::Required(PairingReason::Missing),
            })
        }

        async fn claim(
            &self,
            _: &crate::ClientInfo,
            code: &str,
        ) -> Result<crate::PairingClaim, crate::BoxError> {
            self.claims
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(if code == CODE {
                crate::PairingClaim::Paired {
                    token: TOKEN.into(),
                }
            } else {
                crate::PairingClaim::Refused {
                    message: "the code is wrong".into(),
                    retry_after: Some(Duration::from_secs(5)),
                }
            })
        }
    }

    /// A client holding `token` that answers every code question with `code`.
    #[derive(Default)]
    struct Holder {
        token: std::sync::Mutex<Option<String>>,
        code: Option<&'static str>,
        asked: std::sync::Mutex<Vec<PairingReason>>,
    }

    impl Holder {
        /// Why the box asked for codes so far.
        fn asked(&self) -> Vec<PairingReason> {
            self.asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl PairingClient for Holder {
        async fn token(&self) -> Option<String> {
            self.token.lock().ok()?.clone()
        }

        async fn code(&self, reason: PairingReason) -> Option<String> {
            self.asked.lock().ok()?.push(reason);
            self.code.map(str::to_owned)
        }

        async fn store(&self, token: String) -> Result<(), crate::BoxError> {
            *self.token.lock().map_err(|_| "poisoned")? = Some(token);
            Ok(())
        }
    }

    /// A box with one lane behind `gate`, served once on a fresh pipe.
    fn paired_box(
        gate: Arc<OneCode>,
    ) -> (
        BoxDuplex,
        tokio::task::JoinHandle<Result<(), crate::ServeError>>,
    ) {
        let (client_io, box_io) = tokio::io::duplex(64 * 1024);
        let snapshot = RegistrySnapshot {
            lanes: vec![crate::LaneInfo {
                key: crate::LaneKey {
                    project: "acme".into(),
                    lane: "0".into(),
                },
                index: 0,
                domain: None,
            }],
            units: Vec::new(),
        };
        let services = BoxServices {
            registry: Arc::new(SharedRegistry::new(snapshot, 4)),
            pairing: gate,
            ..services()
        };
        let server = tokio::spawn(serve(
            mux_server(Box::new(box_io)),
            services,
            ServeOptions::new("box"),
        ));
        (Box::new(client_io), server)
    }

    /// Options with `holder` as the pairing client.
    fn holding(holder: &Arc<Holder>) -> SessionOptions {
        let mut options = SessionOptions::new("client");
        options.pairing = Some(holder.clone());
        options
    }

    /// A client without a token is asked for a code, gets the box's token for
    /// it and keeps it; the next session presents the token and is not asked.
    #[tokio::test]
    async fn a_code_is_traded_for_a_token_that_later_sessions_present()
    -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(10), async {
            let gate = Arc::new(OneCode::default());
            let holder = Arc::new(Holder {
                code: Some(CODE),
                ..Holder::default()
            });
            let (io, server) = paired_box(gate.clone());
            let session = Session::establish(mux_client(io), holding(&holder)).await?;
            assert_eq!(session.registry().current().lanes().count(), 1);
            assert_eq!(holder.token().await.as_deref(), Some(TOKEN));
            assert_eq!(holder.asked(), [PairingReason::Missing]);
            session.close();
            server.abort();

            let (io, server) = paired_box(gate.clone());
            let again = Session::establish(mux_client(io), holding(&holder)).await?;
            assert_eq!(holder.asked().len(), 1, "a held token needs no code");
            assert_eq!(gate.claims.load(std::sync::atomic::Ordering::SeqCst), 1);
            again.close();
            server.abort();
            Ok(())
        })
        .await?
    }

    /// Without a code, with a wrong one and with a token the box does not
    /// accept, the session needs pairing, the box ends it, and the client
    /// never sees the box's registry.
    #[tokio::test]
    async fn an_unpaired_client_learns_nothing_about_the_box()
    -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(10), async {
            let cases: [(Option<&str>, Option<&'static str>); 3] = [
                (None, None),
                (None, Some("WRNG-CODE")),
                (Some("dl_rt_old"), None),
            ];
            for (token, code) in cases {
                let gate = Arc::new(OneCode::default());
                let holder = Arc::new(Holder {
                    token: std::sync::Mutex::new(token.map(str::to_owned)),
                    code,
                    ..Holder::default()
                });
                let (io, server) = paired_box(gate);
                let options = holding(&holder);
                let registry = options.registry.clone();
                let Err(error) = Session::establish(mux_client(io), options).await else {
                    panic!("an unpaired client connected");
                };
                assert!(error.is_needs_pairing(), "{error:?}");
                assert!(!error.is_permanent(), "{error:?}");
                assert_eq!(registry.current().lanes().count(), 0);
                let served = server.await?;
                match (token, code) {
                    (_, Some(_)) => {
                        assert!(
                            matches!(&error, SessionError::PairingRefused { retry_after, .. }
                                if *retry_after == Some(Duration::from_secs(5))),
                            "{error:?}"
                        );
                        assert!(error.to_string().contains("the code is wrong"), "{error}");
                        assert!(
                            matches!(served, Err(crate::ServeError::PairingRefused { .. })),
                            "{served:?}"
                        );
                    }
                    (Some(_), None) => {
                        assert!(matches!(
                            error,
                            SessionError::NeedsPairing(PairingReason::Invalid)
                        ));
                        assert!(error.to_string().contains("no longer accepts"), "{error}");
                    }
                    (None, None) => {
                        assert!(matches!(
                            error,
                            SessionError::NeedsPairing(PairingReason::Missing)
                        ));
                        assert!(
                            matches!(served, Err(crate::ServeError::NotPaired { .. })),
                            "{served:?}"
                        );
                    }
                }
                assert_eq!(holder.token().await.as_deref(), token, "nothing stored");
            }
            Ok(())
        })
        .await?
    }

    /// Until the welcome the box answers nothing but the control stream: a
    /// data stream an unpaired client opens while the box waits for its code
    /// is reset unanswered, and pairing still works afterwards.
    #[tokio::test]
    async fn streams_before_the_welcome_are_reset() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(10), async {
            let (client_io, box_io) = tokio::io::duplex(64 * 1024);
            let services = BoxServices {
                pairing: Arc::new(OneCode::default()),
                ..services()
            };
            let server = tokio::spawn(serve(
                mux_server(Box::new(box_io)),
                services,
                ServeOptions::new("box"),
            ));
            let mux = mux::client(Box::new(client_io));
            let mut control = wire::channel(mux.open().await?);
            let hello = OpenRequest::Control {
                protocol: PROTOCOL_VERSION,
                client: "c".into(),
                features: Vec::new(),
                client_id: "dl_cid_x".into(),
                client_name: "laptop".into(),
                token: None,
            };
            wire::send(&mut control, &hello).await?;
            let first = wire::recv::<_, BoxControl>(&mut control).await?;
            assert!(
                matches!(first, Some((BoxControl::PairingRequired { .. }, _))),
                "{first:?}"
            );
            let mut data = wire::channel(mux.open().await?);
            // The box may reset the stream before this arrives.
            let _ = wire::send(
                &mut data,
                &OpenRequest::Data {
                    target: DialTarget::Port { port: 9 },
                },
            )
            .await;
            let reply = timeout(
                Duration::from_secs(5),
                wire::recv::<_, OpenReply>(&mut data),
            )
            .await?;
            assert!(!matches!(reply, Ok(Some(_))), "answered: {reply:?}");
            let pair = ClientControl::Pair {
                code: Secret(CODE.into()),
            };
            wire::send(&mut control, &pair).await?;
            let paired = wire::recv::<_, BoxControl>(&mut control).await?;
            assert!(
                matches!(paired, Some((BoxControl::Paired { .. }, _))),
                "{paired:?}"
            );
            server.abort();
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn client_reports_incompatible_box_as_permanent() -> Result<(), Box<dyn std::error::Error>>
    {
        timeout(Duration::from_secs(5), async {
            let (client_io, box_io) = tokio::io::duplex(64 * 1024);
            let fake_box = tokio::spawn(async move {
                let mut mux = mux::server(Box::new(box_io));
                let stream = mux.accept().await.ok_or("no control stream")?;
                let mut control = wire::channel(stream);
                let _hello = wire::recv::<_, OpenRequest>(&mut control).await?;
                let reply = BoxControl::Welcome {
                    protocol: PROTOCOL_VERSION + 1,
                    server: "devlanes 9.0.0".into(),
                    features: Vec::new(),
                    box_info: BoxInfo::default(),
                };
                wire::send(&mut control, &reply).await?;
                // Hold the stream until the client gives up.
                let _ = wire::recv::<_, Value>(&mut control).await;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
            });
            let result = Session::establish(
                mux_client(Box::new(client_io)),
                SessionOptions::new("client"),
            )
            .await;
            let Err(error) = result else {
                panic!("handshake with an incompatible box succeeded");
            };
            assert!(error.is_permanent());
            assert!(
                error
                    .to_string()
                    .contains("Install the same release on both machines")
            );
            fake_box.abort();
            Ok(())
        })
        .await?
    }

    /// A box that takes no connections turns the client away before the
    /// welcome with its reason, which the client keeps as a permanent error,
    /// and the client learns nothing about the box.
    #[tokio::test]
    async fn a_refusing_box_is_permanent_and_says_why() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(10), async {
            let (client_io, box_io) = tokio::io::duplex(64 * 1024);
            let reason =
                "the box does not accept connections. Set remote.allow_connections to true there";
            let server = tokio::spawn(crate::refuse(
                mux_server(Box::new(box_io)),
                reason.into(),
                Timing::default(),
            ));
            let options = SessionOptions::new("client");
            let registry = options.registry.clone();
            let Err(error) = Session::establish(mux_client(Box::new(client_io)), options).await
            else {
                panic!("a refusing box served the client");
            };
            assert!(matches!(&error, SessionError::Refused(_)), "{error:?}");
            assert!(error.is_permanent(), "{error:?}");
            assert_eq!(error.to_string(), reason);
            assert_eq!(registry.current().lanes().count(), 0);
            server.await??;
            Ok(())
        })
        .await?
    }
}
