//! The GUI host core: serves the windows of one OS [`Backend`] to viewers,
//! tools and the daemon over the devlanes GUI protocol.
//!
//! [`serve`] accepts connections and runs one task that owns all state, so
//! nothing is locked across connections. Per connection:
//!
//! - The hello is read with `decode_hello`. Another protocol closes the
//!   connection; another version, a role this host does not serve
//!   ([`Role::Input`], unknown ones), a viewer with no codec in common and a
//!   control hello without the token from [`ServeOptions::control_token`]
//!   get a typed refusal. Every later message is checked with
//!   `ClientMessage::allowed_for`, and the first one the role may not send
//!   closes the connection.
//! - Writes go through a queue capped at [`ServeOptions::write_queue_cap`];
//!   a connection that does not read is closed past it.
//! - A heartbeat goes out when nothing else did for [`HEARTBEAT_INTERVAL`].
//! - At most [`MAX_CONNECTIONS`] are welcomed at once; a hello past it is
//!   refused. At most [`MAX_PENDING_HELLOS`] wait for their hello; a new
//!   connection past it closes the one that waited longest.
//!
//! Video: each window has one stream per codec its viewers selected, with one
//! encoder and one [`devlanes_gui_proto::Pacer`]. Encoding runs on the
//! blocking pool, one frame at a time per stream. An encoder that cannot
//! start moves its viewers to the next codec both ends have (`raw` at the
//! least). The last frame of every window is kept for keyframes; right after
//! a resize it is held back as the pacer documents. The backend is told when
//! a window gains its first viewer and loses its last ([`Command::Watch`]);
//! a screenshot of a window nobody watches asks the backend for a fresh frame.
//!
//! Input: one [`InputArbiter`] for the display. What a holder that went
//! silent or disconnected held is released on the backend. Input to a
//! locked window is dropped, except releases of held buttons and keys, which
//! always reach the app (a press that comes with a release is dropped). A
//! resize of a Frozen window is answered at once and its newest size applied
//! on thaw, while a close request still goes through. All input counts as
//! activity: the [`ServeOptions::on_activity`] callback and
//! [`HostMessage::Activity`] to control connections, at most once per
//! [`HEARTBEAT_INTERVAL`]. Clipboard text goes to the backend at most once
//! per [`CLIPBOARD_GAP`], the newest text winning.
//!
//! [`InputArbiter`]: devlanes_gui_proto::InputArbiter
//! [`HostMessage::Activity`]: devlanes_gui_proto::HostMessage::Activity
//! [`Role::Input`]: devlanes_gui_proto::Role::Input
//! [`HEARTBEAT_INTERVAL`]: devlanes_gui_proto::HEARTBEAT_INTERVAL

mod backend;
mod connection;
mod hub;
mod outbox;
pub mod pattern;
mod test_pattern;

#[cfg(feature = "test-support")]
pub mod conformance;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use devlanes_gui_codec::{EncoderBackend, RawBackend};
use devlanes_gui_proto::Token;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub use backend::{
    Backend, BackendControl, BackendError, CapturedFrame, Command, Event, EventSink, HostGone,
    Rect, Started, Unavailable,
};
pub use devlanes_gui_proto::MAX_SCREENSHOTS_IN_FLIGHT;
pub use test_pattern::{
    APP_ID, Echo, MAX_PATTERN_SIDE, TestPattern, TestPatternHandle, TestPatternOptions,
};

/// How long a new connection has to send its hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The default cap of a connection's write queue: room for the frames the
/// pacer lets a viewer have in flight at a large size, and more.
pub const DEFAULT_WRITE_QUEUE_CAP: usize = 64 * 1024 * 1024;

/// How long a [`devlanes_gui_proto::ClientMessage::Sync`] waits for the
/// backend's frame before it is answered anyway, so a tool never hangs on a
/// backend that missed one.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(3);

/// `Sync` requests one connection may have unanswered; a tool waits for each
/// answer, so more close the connection.
pub const MAX_PENDING_SYNCS: usize = 16;

/// Screenshots encoding at once across the whole host; past it a request is
/// answered as failed, so no connection is closed for others' load.
pub const MAX_SCREENSHOT_JOBS: usize = 16;

/// Welcomed connections open at once; a hello past it is refused with
/// "too many GUI connections".
pub const MAX_CONNECTIONS: usize = 64;

/// Connections that have not finished their hello yet, counted apart from
/// welcomed ones. A new connection past it closes the one that waited
/// longest, so peers that connect and say nothing cannot keep a viewer or
/// the daemon out.
pub const MAX_PENDING_HELLOS: usize = 16;

/// The shortest gap between two clipboard commands to the backend. Clipboard
/// text can be megabytes, and only the newest matters.
pub const CLIPBOARD_GAP: Duration = Duration::from_millis(200);

/// How long connections get to send what is queued when the host stops.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long accepting pauses after a failed accept, such as when the process
/// is out of file descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Messages waiting for the host task, from every connection together; a
/// reader that finds it full stops reading its socket.
const INBOX: usize = 256;

/// Called on viewer input, at most once per heartbeat interval. It runs on
/// the host task, so it must return at once.
pub type ActivityCallback = Arc<dyn Fn() + Send + Sync>;

/// How the host serves its backend.
#[derive(Clone)]
pub struct ServeOptions {
    /// The service name for the welcome, such as `acme:desktop`.
    pub service: String,
    /// The token a control connection must present; without one the control
    /// role is refused.
    pub control_token: Option<Token>,
    /// The encoders to offer; `raw` by default.
    pub encoders: Vec<Arc<dyn EncoderBackend>>,
    /// The most bytes a connection's write queue may hold.
    pub write_queue_cap: usize,
    /// Called on input from viewers and tools.
    pub on_activity: Option<ActivityCallback>,
    /// For a host the daemon started: when it stops by itself because its
    /// daemon is gone. `None` serves until shutdown.
    pub daemon: Option<DaemonWatch>,
    /// Stops the host when cancelled. Connections are closed without
    /// reporting windows Closed, so viewers reconnect to the next host of a
    /// restarted unit.
    pub shutdown: CancellationToken,
}

/// Whether the unit with a process id and start time still runs. The host
/// calls it on the blocking pool, so it may block, such as on reading the
/// process table. A check that panics counts as running.
pub type UnitAlive = Arc<dyn Fn(u32, u64) -> bool + Send + Sync>;

/// How a host the daemon started notices that its daemon is gone, so it
/// never runs on with nobody to stop it:
///
/// - No unit identity within `identity_timeout` after [`serve`] began:
///   [`ServeError::NoUnitIdentity`]. A daemon that cannot tell the unit's
///   identity leaves its host to stop this way. The deadline is on the
///   monotonic clock, which on Windows keeps counting while the machine
///   sleeps.
/// - A control connection that closes before the identity came:
///   [`ServeError::NoUnitIdentity`] at once.
/// - Once the identity came and every control connection closed, the host
///   checks the unit with `unit_alive` [`UNIT_POLL`] after each answer and
///   ends without an error when it is gone.
#[derive(Clone)]
pub struct DaemonWatch {
    /// How long the host waits for the unit's identity.
    pub identity_timeout: Duration,
    /// Whether the unit still runs.
    pub unit_alive: UnitAlive,
}

/// How often a host whose control connection closed checks its unit.
pub const UNIT_POLL: Duration = Duration::from_secs(2);

impl ServeOptions {
    /// Options for `service` with the defaults: `raw` only, no control
    /// token, [`DEFAULT_WRITE_QUEUE_CAP`], no callback, no daemon watch.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            control_token: None,
            encoders: vec![Arc::new(RawBackend)],
            write_queue_cap: DEFAULT_WRITE_QUEUE_CAP,
            on_activity: None,
            daemon: None,
            shutdown: CancellationToken::new(),
        }
    }
}

impl std::fmt::Debug for ServeOptions {
    /// The settings, encoders by offer and the token redacted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let encoders: Vec<_> = self.encoders.iter().map(|e| e.offer()).collect();
        f.debug_struct("ServeOptions")
            .field("service", &self.service)
            .field("control_token", &self.control_token)
            .field("encoders", &encoders)
            .field("write_queue_cap", &self.write_queue_cap)
            .field("on_activity", &self.on_activity.is_some())
            .field(
                "identity_timeout",
                &self.daemon.as_ref().map(|daemon| daemon.identity_timeout),
            )
            .finish_non_exhaustive()
    }
}

/// Why [`start_backend`] or [`serve`] failed, or why [`serve`] ended other
/// than through [`ServeOptions::shutdown`].
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The backend did not start.
    #[error("the GUI backend did not start")]
    Backend(#[from] BackendError),
    /// The backend panicked while starting.
    #[error("the GUI backend panicked while starting")]
    BackendPanicked,
    /// The backend stopped sending events, such as when its display died.
    /// Every window was reported Closed and the connections were closed.
    #[error("the GUI backend stopped")]
    BackendStopped,
    /// The unit's identity did not come in time, or the control connection
    /// closed before it did ([`DaemonWatch`]). The connections were closed.
    #[error("the daemon that started this host went away before its unit ran")]
    NoUnitIdentity,
}

/// Where [`serve`] gets its connections.
pub trait Listener: Send {
    /// An accepted connection.
    type Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static;

    /// The next connection, or `None` once no more will come; the host then
    /// keeps serving the ones it has. Must be cancel safe: [`serve`] drops it
    /// when something else happens first.
    fn accept(&mut self) -> impl Future<Output = std::io::Result<Option<Self::Stream>>> + Send;
}

impl Listener for TcpListener {
    type Stream = TcpStream;

    /// Accepts with `TCP_NODELAY` set, since input and acknowledgements are
    /// small and must not wait for more data.
    async fn accept(&mut self) -> std::io::Result<Option<TcpStream>> {
        let (stream, _) = TcpListener::accept(self).await?;
        if let Err(error) = stream.set_nodelay(true) {
            tracing::debug!(%error, "cannot set TCP_NODELAY on a GUI connection");
        }
        Ok(Some(stream))
    }
}

/// Connections accepted elsewhere, such as in-memory pipes in tests or
/// streams a caller accepted on another transport.
impl<S> Listener for mpsc::Receiver<S>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    type Stream = S;

    /// The next stream sent; `None` once every sender is gone.
    async fn accept(&mut self) -> std::io::Result<Option<S>> {
        Ok(self.recv().await)
    }
}

/// A backend whose display is up, before [`serve`] takes it. Dropping it
/// stops the backend.
pub struct RunningBackend {
    control: Box<dyn BackendControl>,
    events: mpsc::UnboundedReceiver<Event>,
    env: Vec<(String, Option<String>)>,
    real_desktop: bool,
}

impl RunningBackend {
    /// The variables the unit needs to reach the display; `None` removes one.
    /// The caller reports them, such as on the host's ready line, once it
    /// listens.
    pub fn env(&self) -> &[(String, Option<String>)] {
        &self.env
    }

    /// Whether the display is the machine's real desktop.
    pub fn real_desktop(&self) -> bool {
        self.real_desktop
    }
}

impl std::fmt::Debug for RunningBackend {
    /// The variables and the flag.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningBackend")
            .field("env", &self.env)
            .field("real_desktop", &self.real_desktop)
            .finish_non_exhaustive()
    }
}

/// Starts `backend` on the blocking pool. Events it sends before [`serve`]
/// runs wait for it.
pub async fn start_backend<B: Backend>(backend: B) -> Result<RunningBackend, ServeError> {
    let (sink, events) = EventSink::channel();
    let start = move || Box::new(backend).start(sink);
    let started = tokio::task::spawn_blocking(start)
        .await
        .map_err(|_| ServeError::BackendPanicked)??;
    Ok(RunningBackend {
        control: started.control,
        events,
        env: started.env,
        real_desktop: started.real_desktop,
    })
}

/// Serves the windows of `backend` on the connections `listener` accepts,
/// until [`ServeOptions::shutdown`] is cancelled or the backend stops. A
/// connection past [`MAX_PENDING_HELLOS`] closes the oldest one still
/// without a hello, and one past [`MAX_CONNECTIONS`] is refused once its
/// hello is read. A control connection's `UnitExited` reports every window
/// Closed and ends serving without an error: a host serves one run of its
/// unit. On the way out the connections get two seconds to send what is
/// queued, and the backend is dropped, which stops it.
pub async fn serve<L: Listener>(
    mut listener: L,
    backend: RunningBackend,
    options: ServeOptions,
) -> Result<(), ServeError> {
    let RunningBackend {
        control,
        events,
        real_desktop,
        ..
    } = backend;
    let settings = Arc::new(connection::Settings {
        token: options.control_token.clone(),
        encoders: options.encoders.iter().map(|e| e.offer()).collect(),
        write_queue_cap: options.write_queue_cap,
        welcomed: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
    });
    let hellos = Arc::new(connection::Hellos::new(MAX_PENDING_HELLOS));
    let (inbox_tx, inbox_rx) = mpsc::channel(INBOX);
    let hub = hub::Hub::new(control, &options, real_desktop);
    // Boxed so it can be dropped before the connections drain.
    let mut hub = Box::pin(hub.run(inbox_rx, events));
    let mut connections = JoinSet::new();
    let mut accepting = true;
    let mut paused_until = None;
    let result = loop {
        let resume = async {
            match paused_until {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = &mut hub => break result,
            accepted = listener.accept(), if accepting && paused_until.is_none() => match accepted {
                Ok(Some(stream)) => {
                    let pending = hellos.admit();
                    let run = connection::run(stream, pending, inbox_tx.clone(), settings.clone());
                    connections.spawn(run);
                }
                Ok(None) => accepting = false,
                Err(error) => {
                    tracing::warn!(%error, "accepting a GUI connection failed");
                    paused_until = Some(tokio::time::Instant::now() + ACCEPT_BACKOFF);
                }
            },
            _ = resume => paused_until = None,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    };
    // Readers waiting to hand a message over then fail at once instead of
    // waiting on an inbox nobody reads.
    drop(hub);
    drop(inbox_tx);
    // The host task closed every write queue; writers send what is left and end.
    let drain = async { while connections.join_next().await.is_some() {} };
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, drain).await;
    connections.shutdown().await;
    result
}

// Compiles the README's example with the doc tests, so it keeps matching the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
