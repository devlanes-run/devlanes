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
    /// A stand-in page until the cards are built: `ui://devlanes/lane.html`
    /// served as `text/html;profile=mcp-app`.
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

/// Serves MCP on stdin and stdout until the client closes them. Nothing connects
/// to the daemon here; each tool call runs its own devlanes process.
pub async fn serve(config: Config) -> Result<(), Error> {
    use rmcp::ServiceExt as _;
    let running = Server::new(config)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| Error::Initialize(Box::new(error)))?;
    running.waiting().await?;
    Ok(())
}
