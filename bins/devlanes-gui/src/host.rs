//! `devlanes-gui host`: one GUI host process for one GUI service replica.
//!
//! The daemon starts it before the unit. It starts the backend, listens on a
//! loopback address, prints the ready line on standard output and then serves
//! the GUI protocol until a termination signal (on Windows, the console
//! events the runner sends when it stops a unit) or until the backend stops.

use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::sync::Arc;

use devlanes_gui_host::{
    BackendError, DaemonWatch, RunningBackend, ServeError, ServeOptions, TestPattern,
    TestPatternOptions, Unavailable, serve, start_backend,
};
use devlanes_gui_proto::{
    Token,
    ready::{CodecSide, Ready, UnavailableReason, Unready},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken as Shutdown;

// The token travels in `TOKEN_ENV` and stays in this process's environment
// (removing it takes `unsafe` code), so a backend that starts a process
// should start it without the variable where the spawning API allows it.
use devlanes_gui_proto::ready::TOKEN_ENV;

/// The variable that sets the log filter, as for the daemon.
const LOG_ENV: &str = "DEVLANES_LOG";

/// How long a host the daemon started (one with a token) waits for the
/// unit's identity once it listens. The daemon sends it within seconds of the
/// ready line. Two cases reach the deadline: a daemon that could not tell the
/// unit's identity (its runner did not know it), and a daemon that died
/// without closing its control connection. It is measured on the monotonic
/// clock, which on Windows keeps counting while the machine sleeps.
const IDENTITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The arguments of `devlanes-gui host`.
#[derive(Debug, clap::Args)]
pub struct HostArgs {
    /// The backend that owns the display: `test-pattern`.
    #[arg(long, value_name = "NAME")]
    backend: String,
    /// The loopback address and port to listen on, such as 127.0.0.1:0 (port 0
    /// picks a free one; the ready line names it).
    #[arg(long, value_name = "ADDRESS")]
    listen: SocketAddr,
    /// The service this host belongs to, such as acme:desktop, which viewers show.
    #[arg(long, value_name = "LABEL")]
    service: String,
}

/// The backends this build has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    /// A window with a known colour grid, for tests.
    TestPattern,
}

impl BackendKind {
    /// Every backend by its `--backend` name.
    const ALL: [(&'static str, Self); 1] = [("test-pattern", Self::TestPattern)];

    /// The backend `--backend` names.
    fn parse(name: &str) -> Result<Self, HostError> {
        Self::ALL
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, kind)| *kind)
            .ok_or_else(|| HostError::UnknownBackend(name.to_owned()))
    }

    /// Starts the backend's display.
    async fn start(self) -> Result<RunningBackend, ServeError> {
        match self {
            Self::TestPattern => {
                let (backend, _handle) = TestPattern::new(TestPatternOptions::default());
                start_backend(backend).await
            }
        }
    }
}

/// Why `devlanes-gui host` did not run or stopped with a failure.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// `--backend` names no backend of this build.
    #[error("unknown GUI backend {0:?}; this build has: test-pattern")]
    UnknownBackend(String),
    /// `--listen` is not a loopback address.
    #[error("{0} is not a loopback address; the GUI host listens on 127.0.0.1 or ::1 only")]
    NotLoopback(SocketAddr),
    /// The async runtime did not start.
    #[error("cannot start the async runtime")]
    Runtime(#[source] std::io::Error),
    /// Looking for the codecs failed with a bug.
    #[error("cannot look for the video codecs: {0}")]
    Codecs(String),
    /// The termination signals could not be watched.
    #[error("cannot watch for termination signals")]
    Signals(#[source] std::io::Error),
    /// The backend cannot run on this machine; standard output has the
    /// unready line with the reason.
    #[error("the GUI backend cannot run here: {0}")]
    Unavailable(String),
    /// The backend did not start, or it stopped while serving.
    #[error(transparent)]
    Serve(#[from] ServeError),
    /// The address could not be bound.
    #[error("cannot listen on {address}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    /// Writing the ready line failed, such as into a pipe whose reader is
    /// gone. A standard output that was never open is not caught: the
    /// standard library then drops what is written.
    #[error("cannot write the ready line")]
    Ready(#[source] std::io::Error),
}

/// Runs `devlanes-gui host`. The arguments are checked and the token read
/// before any thread starts.
pub fn run(args: HostArgs) -> Result<(), HostError> {
    let backend = BackendKind::parse(&args.backend)?;
    if !args.listen.ip().is_loopback() {
        return Err(HostError::NotLoopback(args.listen));
    }
    // An empty value means no token, so the control role stays refused.
    let token = std::env::var(TOKEN_ENV)
        .ok()
        .filter(|token| !token.is_empty())
        .map(Token::new);
    init_logging();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(HostError::Runtime)?;
    runtime.block_on(host(backend, args, token))
}

/// Starts the backend, binds the port, prints the ready line and serves.
async fn host(backend: BackendKind, args: HostArgs, token: Option<Token>) -> Result<(), HostError> {
    let mut options = ServeOptions::new(args.service);
    // Only the daemon passes a token, and only it sends the unit's identity.
    options.daemon = token.is_some().then(|| DaemonWatch {
        identity_timeout: IDENTITY_TIMEOUT,
        unit_alive: Arc::new(unit_alive),
    });
    options.control_token = token;
    // Watched first, so a stop that comes while the backend starts still ends
    // the host cleanly.
    watch_signals(options.shutdown.clone()).map_err(HostError::Signals)?;
    let running = match backend.start().await {
        Ok(running) => running,
        Err(ServeError::Backend(BackendError::Unavailable { reason, message })) => {
            let line = Unready::new(devlanes_version::VERSION, unready_reason(reason), &message);
            print_line(&line.to_line())?;
            return Err(HostError::Unavailable(message));
        }
        Err(error) => return Err(error.into()),
    };
    // Blocking: it loads OpenH264 or enumerates Media Foundation's transforms.
    let codecs = tokio::task::spawn_blocking(crate::codecs::find)
        .await
        .map_err(|error| HostError::Codecs(error.to_string()))?;
    // A host only encodes; what this machine cannot decode is the viewer's.
    let problems = codecs.problems_of(CodecSide::Encode);
    for problem in &problems {
        tracing::warn!(codec = %problem.codec, reason = problem.reason.word(), detail = %problem.detail, "a video encoder is missing; viewers get the next codec both ends have");
    }
    if let Some(version) = &codecs.version {
        tracing::info!(%version, "H.264 available");
    }
    options.encoders = codecs.encoders;
    let listener = TcpListener::bind(args.listen)
        .await
        .map_err(|source| HostError::Bind {
            address: args.listen,
            source,
        })?;
    let address = listener.local_addr().map_err(|source| HostError::Bind {
        address: args.listen,
        source,
    })?;
    if options.shutdown.is_cancelled() {
        return Ok(());
    }
    let mut ready = Ready::new(
        address,
        devlanes_version::VERSION,
        running.env(),
        running.real_desktop(),
    );
    ready.codecs = problems;
    let line = ready.to_line();
    print_line(&line)?;
    tracing::info!(%address, backend = ?backend, "GUI host ready");
    serve(listener, running, options).await?;
    tracing::info!("GUI host stopped");
    Ok(())
}

/// Whether the unit's process still runs, by pid and start time, for a host
/// whose daemon is gone. A process table that cannot be read counts as
/// running, so a passing error never stops a host under a live unit.
/// Blocking: the host calls it on the blocking pool.
fn unit_alive(pid: u32, start_time: u64) -> bool {
    let unit = devlanes_core::ProcessIdentity {
        pid,
        sid: 0,
        start_time,
    };
    devlanes_sys::matches(&unit).unwrap_or(true)
}

/// Writes `line` and a newline to standard output and flushes it, since the
/// daemon waits for it.
fn print_line(line: &str) -> Result<(), HostError> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(HostError::Ready)
}

/// The unready line's word for a backend's reason.
fn unready_reason(reason: Unavailable) -> UnavailableReason {
    match reason {
        Unavailable::NoDesktopSession => UnavailableReason::NoDesktopSession,
        Unavailable::Elevated => UnavailableReason::Elevated,
        Unavailable::MissingLibrary => UnavailableReason::MissingLibrary,
        Unavailable::UnsupportedOs => UnavailableReason::UnsupportedOs,
        // `Other`, and reasons added to the backend later.
        _ => UnavailableReason::Other,
    }
}

/// Logs to standard error, filtered by `DEVLANES_LOG` (default `info`).
pub(crate) fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env(LOG_ENV)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init();
}

/// Cancels `shutdown` on SIGINT, SIGTERM or SIGHUP.
#[cfg(unix)]
fn watch_signals(shutdown: Shutdown) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        let name = tokio::select! {
            _ = interrupt.recv() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
            _ = hangup.recv() => "SIGHUP",
        };
        tracing::info!(signal = name, "stopping");
        shutdown.cancel();
    });
    Ok(())
}

/// Cancels `shutdown` on Ctrl-C and Ctrl-Break, which the Windows runner
/// sends to stop a unit, and when the console closes.
#[cfg(windows)]
fn watch_signals(shutdown: Shutdown) -> std::io::Result<()> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};
    let mut interrupt = ctrl_c()?;
    let mut brk = ctrl_break()?;
    let mut close = ctrl_close()?;
    tokio::spawn(async move {
        let name = tokio::select! {
            _ = interrupt.recv() => "Ctrl-C",
            _ = brk.recv() => "Ctrl-Break",
            _ = close.recv() => "console closed",
        };
        tracing::info!(signal = name, "stopping");
        shutdown.cancel();
    });
    Ok(())
}
