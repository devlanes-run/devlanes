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
//!
//! Video: each window has one stream per codec its viewers selected, with one
//! encoder and one [`devlanes_gui_proto::Pacer`]. Encoding runs on the
//! blocking pool, one frame at a time per stream. The last frame of every
//! window is kept for keyframes and screenshots; right after a resize it is
//! held back as the pacer documents.
//!
//! Input: one [`InputArbiter`] for the display. What a holder that went
//! silent or disconnected held is released on the backend. Input to a
//! locked window is dropped, and all input counts as activity: the
//! [`ServeOptions::on_activity`] callback and [`HostMessage::Activity`] to
//! control connections, at most once per [`HEARTBEAT_INTERVAL`].
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
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use devlanes_gui_codec::{EncoderBackend, RawBackend};
use devlanes_gui_proto::Token;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub use backend::{
    Backend, BackendControl, BackendError, CapturedFrame, Command, Event, EventSink, HostGone, Rect,
};
pub use test_pattern::{APP_ID, Echo, TestPattern, TestPatternHandle, TestPatternOptions};

/// How long a new connection has to send its hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The default cap of a connection's write queue: room for the frames the
/// pacer lets a viewer have in flight at a large size, and more.
pub const DEFAULT_WRITE_QUEUE_CAP: usize = 64 * 1024 * 1024;

/// How long a [`devlanes_gui_proto::ClientMessage::Sync`] waits for the
/// backend's frame before it is answered anyway, so a tool never hangs on a
/// backend that missed one.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(3);

/// Screenshots one connection may have in progress; a tool waits for each
/// answer, so more close the connection.
pub const MAX_SCREENSHOTS_IN_FLIGHT: usize = 4;

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
    /// Stops the host when cancelled.
    pub shutdown: CancellationToken,
}

impl ServeOptions {
    /// Options for `service` with the defaults: `raw` only, no control
    /// token, [`DEFAULT_WRITE_QUEUE_CAP`], no callback.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            control_token: None,
            encoders: vec![Arc::new(RawBackend)],
            write_queue_cap: DEFAULT_WRITE_QUEUE_CAP,
            on_activity: None,
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
            .finish_non_exhaustive()
    }
}

/// Why [`serve`] ended other than through [`ServeOptions::shutdown`].
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

/// Starts `backend` and serves its windows on the connections `listener`
/// accepts, until [`ServeOptions::shutdown`] is cancelled or the backend
/// stops. On the way out the connections get [`SHUTDOWN_GRACE`] to send what
/// is queued, and the backend is dropped, which stops it.
pub async fn serve<L: Listener, B: Backend>(
    mut listener: L,
    backend: B,
    options: ServeOptions,
) -> Result<(), ServeError> {
    let real_desktop = backend.real_desktop();
    let (sink, events) = EventSink::channel();
    let start = move || Box::new(backend).start(sink);
    let control = tokio::task::spawn_blocking(start)
        .await
        .map_err(|_| ServeError::BackendPanicked)??;

    let settings = Arc::new(connection::Settings {
        token: options.control_token.clone(),
        encoders: options.encoders.iter().map(|e| e.offer()).collect(),
        write_queue_cap: options.write_queue_cap,
    });
    let (inbox_tx, inbox_rx) = mpsc::channel(INBOX);
    let hub = hub::Hub::new(control, &options, real_desktop);
    let mut hub = pin!(hub.run(inbox_rx, events));
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
                    connections.spawn(connection::run(stream, inbox_tx.clone(), settings.clone()));
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
