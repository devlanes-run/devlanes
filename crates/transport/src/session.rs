//! Client side: one session over one transport connection.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures_util::SinkExt;
use serde_json::Value;
use tokio::sync::watch;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use crate::mux::{self, MuxClient, MuxError, MuxStream};
use crate::pairing::PairingClient;
use crate::registry::ClientRegistry;
use crate::sync::SyncSubscription;
use crate::transport::{BoxDuplex, Transport, TransportError};
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
    /// (including SSH prompts) plus handshake.
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
    /// The transport could not produce a pipe.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The mux failed.
    #[error(transparent)]
    Mux(#[from] MuxError),
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
        "{} but this client speaks protocol {local}. Install the matching devlanes release on \
         the box, then run `devlanes daemon restart` there when its managed processes may stop. \
         Box daemons are never upgraded automatically",
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
    /// The box side broke the connection; the text is the pipe's own report, such
    /// as the stderr of a remote `devlanes tunnel --stdio` that exited.
    #[error("box closed the connection: {0}")]
    Closed(String),
    /// The box serves only paired clients and nobody gave a pairing code.
    #[error("{}", needs_pairing_text(*.0))]
    NeedsPairing(PairingReason),
    /// The box refused the pairing code.
    #[error("the box refused the pairing code: {message}")]
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
            Self::IncompatibleProtocol { .. } => true,
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
            "the box serves only paired clients; get a code with `devlanes daemon pair` on the box"
        }
        PairingReason::Invalid => {
            "the box no longer accepts this machine's pairing (it was revoked, or dropped as the \
             oldest of too many); get a new code with `devlanes daemon pair` on the box"
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
pub type DataStream = Prefixed<MuxStream>;

/// A live client session. Clones share it; dropping the last clone closes it.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    mux: MuxClient,
    welcome: Welcome,
    registry: ClientRegistry,
    timing: Timing,
    end: watch::Receiver<Option<SessionEnd>>,
    cancel: CancellationToken,
    /// The latest mirror report, which the control task sends once.
    report: watch::Sender<Option<Value>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.mux.shutdown();
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
        let io = transport.connect().await?;
        Self::establish(io, options).await
    }

    /// Runs the handshake over an existing pipe.
    ///
    /// Returns after the box's snapshot replaced `options.registry`, so callers
    /// see current state immediately.
    pub async fn establish(io: BoxDuplex, options: SessionOptions) -> Result<Self, SessionError> {
        let mux = mux::client(io);
        match handshake(&mux, &options).await {
            Ok((control, welcome)) => {
                let (end_tx, end) = watch::channel(None);
                let cancel = CancellationToken::new();
                let report = watch::Sender::new(None);
                tokio::spawn(run_control(
                    control,
                    Control {
                        registry: options.registry.clone(),
                        timing: options.timing,
                        cancel: cancel.clone(),
                        end_tx,
                        mux: mux.clone(),
                        reports: report.subscribe(),
                    },
                ));
                Ok(Self {
                    inner: Arc::new(Inner {
                        mux,
                        welcome,
                        registry: options.registry,
                        timing: options.timing,
                        end,
                        cancel,
                        report,
                    }),
                })
            }
            Err(error) => {
                mux.shutdown();
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

    /// Opens a mux stream and sends its open frame.
    async fn open_stream(&self, open: &OpenRequest) -> Result<Channel<MuxStream>, SessionError> {
        if let Some(end) = self.end() {
            return Err(SessionError::Ended(end));
        }
        let stream = match self.inner.mux.open().await {
            Ok(stream) => stream,
            Err(error) => return Err(self.end().map_or(error.into(), SessionError::Ended)),
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
    mux: &MuxClient,
    options: &SessionOptions,
) -> Result<(Channel<MuxStream>, Welcome), SessionError> {
    let limit = options.timing.handshake_timeout;
    let token = match &options.pairing {
        Some(pairing) => pairing.token().await,
        None => None,
    };
    let (mut control, first) = within(mux, limit, async {
        let mut control = wire::channel(mux.open().await?);
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
            pair(mux, &mut control, options, reason).await?;
            None
        }
        other => Some(other),
    };
    let welcome = within(mux, limit, async {
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
    mux: &MuxClient,
    control: &mut Channel<MuxStream>,
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
    let token = within(mux, options.timing.handshake_timeout, async {
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
/// box's own answer (a protocol mismatch, a refused pairing code) is replaced
/// by the pipe's error when the pipe died, which usually says why.
async fn within<T>(
    mux: &MuxClient,
    limit: Duration,
    exchange: impl std::future::Future<Output = Result<T, SessionError>>,
) -> Result<T, SessionError> {
    match timeout(limit, exchange).await {
        Err(_) => Err(SessionError::HandshakeTimeout),
        Ok(Err(
            error @ (SessionError::IncompatibleProtocol { .. }
            | SessionError::PairingRefused { .. }),
        )) => Err(error),
        Ok(Err(error)) => match mux.failure_within(FAILURE_GRACE).await {
            Some(reason) => Err(SessionError::Closed(reason)),
            None => Err(error),
        },
        Ok(Ok(value)) => Ok(value),
    }
}

/// How long a lost stream waits for the mux driver to report the pipe's error.
const FAILURE_GRACE: Duration = Duration::from_secs(1);

/// Ends a session that lost its control stream, preferring the pipe's own error.
async fn lost(mux: &MuxClient) -> SessionEnd {
    match mux.failure_within(FAILURE_GRACE).await {
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
    mux: MuxClient,
    /// Mirror reports to send, latest only.
    reports: watch::Receiver<Option<Value>>,
}

/// Applies registry changes, pings and sends mirror reports until the
/// session ends.
async fn run_control(mut control: Channel<MuxStream>, parts: Control) {
    let Control {
        registry,
        timing,
        cancel,
        end_tx,
        mux,
        mut reports,
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
            outstanding.map(|(_, sent)| sent.max(mux.last_inbound()) + timing.ping_timeout);
        tokio::select! {
            () = cancel.cancelled() => break SessionEnd::Closed,
            () = mux.closed() => break lost(&mux).await,
            message = wire::recv::<_, BoxControl>(&mut control) => match message {
                Ok(Some((BoxControl::Snapshot { snapshot }, _))) => registry.replace(snapshot),
                Ok(Some((BoxControl::Event { event }, _))) => registry.apply(event),
                Ok(Some((BoxControl::Pong { nonce: answered }, _))) => {
                    if outstanding.is_some_and(|(sent, _)| sent == answered) {
                        outstanding = None;
                    }
                }
                Ok(Some(_)) => break SessionEnd::Failed("unexpected control message".into()),
                Ok(None) => break lost(&mux).await,
                Err(error) => break match mux.failure_within(FAILURE_GRACE).await {
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
                let silent_since = outstanding.map(|(_, sent)| sent.max(mux.last_inbound()));
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
    mux.shutdown();
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
    use crate::registry::{RegistrySnapshot, SharedRegistry};
    use crate::server::{BoxServices, LoopbackDialer, ServeError, ServeOptions, serve};
    use crate::sync::NoSync;

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

    #[tokio::test]
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
            let session = Session::establish(Box::new(client_io), options).await?;
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
    async fn box_refuses_other_protocol_revisions() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let (client_io, box_io) = tokio::io::duplex(64 * 1024);
            let server = tokio::spawn(serve(box_io, services(), ServeOptions::new("box 9")));
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
            let result = Session::establish(Box::new(DeadTunnel), SessionOptions::new("c")).await;
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
        let server = tokio::spawn(serve(box_io, services, ServeOptions::new("box")));
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
            let session = Session::establish(io, holding(&holder)).await?;
            assert_eq!(session.registry().current().lanes().count(), 1);
            assert_eq!(holder.token().await.as_deref(), Some(TOKEN));
            assert_eq!(holder.asked(), [PairingReason::Missing]);
            session.close();
            server.abort();

            let (io, server) = paired_box(gate.clone());
            let again = Session::establish(io, holding(&holder)).await?;
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
                let Err(error) = Session::establish(io, options).await else {
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
                        assert!(error.to_string().contains("revoked"), "{error}");
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
            let server = tokio::spawn(serve(box_io, services, ServeOptions::new("box")));
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
            let result =
                Session::establish(Box::new(client_io), SessionOptions::new("client")).await;
            let Err(error) = result else {
                panic!("handshake with an incompatible box succeeded");
            };
            assert!(error.is_permanent());
            assert!(error.to_string().contains("never upgraded automatically"));
            fake_box.abort();
            Ok(())
        })
        .await?
    }
}
