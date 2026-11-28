//! `devlanes-gui host`: one GUI host process for one GUI service replica.
//!
//! The daemon starts it before the unit. It starts the backend, listens on a
//! loopback address, prints the ready line on standard output and then serves
//! the GUI protocol until a termination signal (on Windows, the console
//! events the runner sends when it stops a unit) or until the backend stops.

use std::io::{IsTerminal, Write};
use std::net::SocketAddr;

use devlanes_gui_host::{
    RunningBackend, ServeError, ServeOptions, TestPattern, TestPatternOptions, serve, start_backend,
};
use devlanes_gui_proto::Token;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken as Shutdown;

/// The variable that carries the control token. Never a flag: other users
/// can read a process's arguments in a process listing, but not its
/// environment. It stays in this process's environment, since removing it
/// takes `unsafe` code (`std::env::remove_var`) outside the crates allowed
/// to have it; the host starts no process, and a backend that starts one
/// must take it out of that child's environment.
pub const TOKEN_ENV: &str = "DEVLANES_GUI_TOKEN";

/// The version of the ready line, its `ready` field. A reader refuses other
/// versions; fields added later keep the version, so a reader ignores
/// fields it does not know.
pub const READY_VERSION: u32 = 1;

/// The variable that sets the log filter, as for the daemon.
const LOG_ENV: &str = "DEVLANES_LOG";

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
    /// The termination signals could not be watched.
    #[error("cannot watch for termination signals")]
    Signals(#[source] std::io::Error),
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
    /// The ready line could not be written, such as when the daemon is gone.
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
    options.control_token = token;
    // Watched first, so a stop that comes while the backend starts still ends
    // the host cleanly.
    watch_signals(options.shutdown.clone()).map_err(HostError::Signals)?;
    let running = backend.start().await?;
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
    let line = ready_line(address, &running);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(HostError::Ready)?;
    drop(stdout);
    tracing::info!(%address, backend = ?backend, "GUI host ready");
    serve(listener, running, options).await?;
    tracing::info!("GUI host stopped");
    Ok(())
}

/// The ready line: one JSON object on one line.
#[derive(Debug, Serialize)]
struct Ready {
    /// [`READY_VERSION`].
    ready: u32,
    /// The address the host listens on, such as `127.0.0.1:41234`.
    address: SocketAddr,
    /// Variables the unit gets, in the backend's order.
    set: serde_json::Map<String, serde_json::Value>,
    /// Variables the unit must not inherit.
    remove: Vec<String>,
    /// Whether the display is the machine's real desktop, so a local viewer
    /// is not needed.
    real_desktop: bool,
}

/// The ready line for `running` listening on `address`. A variable the
/// backend names twice keeps its last value, and a removal after a value
/// drops the value.
fn ready_line(address: SocketAddr, running: &RunningBackend) -> String {
    let mut set = serde_json::Map::new();
    let mut remove: Vec<String> = Vec::new();
    for (name, value) in running.env() {
        match value {
            Some(value) => {
                remove.retain(|removed| removed != name);
                set.insert(name.clone(), serde_json::Value::String(value.clone()));
            }
            None => {
                set.remove(name);
                if !remove.contains(name) {
                    remove.push(name.clone());
                }
            }
        }
    }
    let ready = Ready {
        ready: READY_VERSION,
        address,
        set,
        remove,
        real_desktop: running.real_desktop(),
    };
    // A struct of strings, numbers and booleans always serializes.
    serde_json::to_string(&ready).unwrap_or_default()
}

/// Logs to standard error, filtered by `DEVLANES_LOG` (default `info`).
fn init_logging() {
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
