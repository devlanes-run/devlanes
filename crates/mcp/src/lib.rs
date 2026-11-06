//! The MCP server behind `devlanes mcp`. Tool calls name a devlanes command
//! line; the server checks it with the CLI's parser and runs it as a child
//! process of the devlanes binary, so every command behaves as in a shell and
//! nothing but the protocol reaches stdout.

mod app;
mod child;
mod command;
mod host;
mod progress;
mod server;
pub mod shell_env;
mod text;
mod view;

use std::{collections::BTreeMap, path::PathBuf};

pub use server::Server;

/// The MCP Apps extension clients advertise in their capabilities.
pub const UI_EXTENSION: &str = "io.modelcontextprotocol/ui";

/// The HTML resource of the cards: its URI, MIME type and contents, given by
/// the binary that builds it.
#[derive(Debug, Clone, Copy)]
pub struct UiResource {
    pub uri: &'static str,
    pub mime: &'static str,
    pub html: fn() -> &'static str,
}

impl UiResource {
    /// A minimal page at the cards' URI and MIME type, for tests that check the
    /// resource without depending on `devlanes-mcp-ui`.
    pub fn placeholder() -> Self {
        Self {
            uri: "ui://devlanes/lane.html",
            mime: "text/html;profile=mcp-app",
            html: || PLACEHOLDER_HTML,
        }
    }
}

/// The page [`UiResource::placeholder`] serves.
const PLACEHOLDER_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>devlanes</title></head><body><p>devlanes</p></body></html>";

/// What `devlanes mcp` was started with.
#[derive(Debug, Clone)]
pub struct Config {
    /// The devlanes executable each tool call runs.
    pub exe: PathBuf,
    /// Offer only `devlanes_read` (and the app tools that only read).
    pub read_only: bool,
    /// `--lane`: the lane of calls that name no cwd.
    pub lane: Option<String>,
    /// `--path`: the directory of calls that name no cwd.
    pub path: Option<PathBuf>,
    /// The cards resource, offered to clients with the MCP Apps extension.
    pub ui: Option<UiResource>,
    /// The environment the commands run with: the server's own.
    pub env: BTreeMap<String, String>,
    /// The directory the server was started in.
    pub cwd: PathBuf,
    /// Whether the environment already names an AI agent, so the client's name
    /// is not needed to tell the commands who runs them.
    pub agent_detected: bool,
    /// Reads the login shell's environment when a lane has none from a shell.
    pub shell_env: shell_env::Resolver,
}

impl Config {
    /// A config for `exe` from this process's environment and directory.
    pub fn from_process(exe: PathBuf) -> std::io::Result<Self> {
        Ok(Self {
            exe,
            read_only: false,
            lane: None,
            path: None,
            ui: None,
            env: std::env::vars_os()
                .filter_map(|(key, value)| {
                    Some((key.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
            cwd: std::env::current_dir()?,
            agent_detected: devlanes_cli::Signals::detect().detected_actor.is_some(),
            shell_env: shell_env::resolve,
        })
    }
}

/// Why the server stopped other than the client leaving.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the MCP handshake failed: {0}")]
    Initialize(Box<rmcp::service::ServerInitializeError>),
    #[error("the MCP server stopped: {0}")]
    Stopped(#[from] tokio::task::JoinError),
}

/// Serves MCP on stdin and stdout until the client closes stdin or the
/// process is told to stop (SIGTERM, SIGINT or SIGHUP; on Windows the console
/// closing or the system shutting down). Nothing connects to the daemon here;
/// each tool call runs its own devlanes process.
pub async fn serve(config: Config) -> Result<(), Error> {
    let (stdin, stdout) = rmcp::transport::stdio();
    serve_on(Server::new(config), stdin, stdout, stop_signal()).await
}

/// Serves `server` on `reader` and `writer` until `reader` ends or `stop`
/// resolves, then interrupts the commands of calls still running and waits
/// for them, so none outlives the server. rmcp waits up to 5 s for calls in
/// flight once the client leaves, longer than hosts wait before they kill
/// the server, so the commands stop as soon as the input ends.
pub async fn serve_on<R, W>(
    server: Server,
    reader: R,
    writer: W,
    stop: impl Future<Output = ()>,
) -> Result<(), Error>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use rmcp::ServiceExt as _;
    // A Ctrl-Break meant for a command's group never ends the server: the
    // console reports it to a registered listener instead.
    #[cfg(windows)]
    let _ctrl_break = tokio::signal::windows::ctrl_break();
    let ended = tokio_util::sync::CancellationToken::new();
    let reader = UntilEnd {
        inner: reader,
        ended: ended.clone(),
    };
    let running = server
        .clone()
        .serve((reader, writer))
        .await
        .map_err(|error| Error::Initialize(Box::new(error)))?;
    let service = running.cancellation_token();
    let waiting = running.waiting();
    tokio::pin!(waiting);
    let stopped = tokio::select! {
        waited = &mut waiting => {
            server.stop_commands().await;
            waited?;
            return Ok(());
        }
        () = ended.cancelled() => false,
        () = stop => true,
    };
    server.stop_commands().await;
    // The input is still open after a signal, so only this ends the service.
    if stopped {
        service.cancel();
    }
    waiting.await?;
    Ok(())
}

/// The input of the transport, which cancels `ended` when it ends or fails.
struct UntilEnd<R> {
    inner: R,
    ended: tokio_util::sync::CancellationToken,
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for UntilEnd<R> {
    /// Reads from the input; a read of nothing into room for something is its end.
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let (before, room) = (buf.filled().len(), buf.remaining() > 0);
        let polled = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        match &polled {
            std::task::Poll::Ready(Ok(())) if room && buf.filled().len() == before => {
                self.ended.cancel();
            }
            std::task::Poll::Ready(Err(_)) => self.ended.cancel(),
            _ => {}
        }
        polled
    }
}

/// Resolves when the process is told to stop: SIGTERM, SIGINT or SIGHUP, or
/// on Windows the console closing or the system shutting down. Never resolves
/// when the signals cannot be watched.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::hangup()),
        ) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_close, ctrl_shutdown};
        let (Ok(mut close), Ok(mut shutdown)) = (ctrl_close(), ctrl_shutdown()) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = close.recv() => {}
            _ = shutdown.recv() => {}
        }
    }
}
