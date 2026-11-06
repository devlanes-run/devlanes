//! The server in-process, against an rmcp client over an in-memory pipe. Tool
//! calls run `devlanes-mcp-fake`, which records each run and prints what it
//! was asked, so no daemon is involved.

mod calls;
mod consent;
mod listing;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use devlanes_mcp::{Config, Server, UI_EXTENSION, UiResource};
use rmcp::{
    ClientHandler, ErrorData as McpError, RoleClient,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig,
        ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationCapability,
        FormElicitationCapability, Implementation, JsonObject, ProgressNotificationParam,
        ProtocolVersion,
    },
    service::{
        ClientLifecycleMode, ClientServiceExt as _, NotificationContext, RequestContext,
        RunningService,
    },
};
use serde_json::{Value, json};

/// Upper bound for every test body.
pub const LIMIT: Duration = Duration::from_secs(30);

/// The fake devlanes binary.
const FAKE: &str = env!("CARGO_BIN_EXE_devlanes-mcp-fake");

/// A test client: its name and capabilities, its answer to consent questions,
/// and what the server sent it.
#[derive(Clone)]
pub struct TestClient {
    name: &'static str,
    /// `None`: no elicitation capability.
    answer: Option<ElicitationAction>,
    ui: bool,
    pub asked: Arc<Mutex<Vec<String>>>,
    pub progress: Arc<Mutex<Vec<String>>>,
}

impl Default for TestClient {
    /// Claude Code without elicitation or MCP Apps.
    fn default() -> Self {
        Self {
            name: "claude-code",
            answer: None,
            ui: false,
            asked: Arc::default(),
            progress: Arc::default(),
        }
    }
}

impl TestClient {
    /// Claude Code without elicitation or MCP Apps.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same client under another name.
    pub fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// A client that asks the user, who answers `action`.
    pub fn answering(mut self, action: ElicitationAction) -> Self {
        self.answer = Some(action);
        self
    }

    /// A client that shows MCP Apps.
    pub fn with_ui(mut self) -> Self {
        self.ui = true;
        self
    }

    /// The questions the server asked so far.
    pub fn questions(&self) -> Vec<String> {
        self.asked
            .lock()
            .map(|asked| asked.clone())
            .unwrap_or_default()
    }
}

impl ClientHandler for TestClient {
    /// The capabilities this client advertises.
    fn get_info(&self) -> ClientConfig {
        let mut capabilities = ClientCapabilities::default();
        if self.answer.is_some() {
            capabilities.elicitation =
                Some(ElicitationCapability::new().with_form(FormElicitationCapability::new()));
        }
        if self.ui {
            let settings: JsonObject = serde_json::from_value(json!({
                "mimeTypes": ["text/html;profile=mcp-app"]
            }))
            .unwrap_or_default();
            capabilities.extensions = Some(BTreeMap::from([(UI_EXTENSION.to_owned(), settings)]));
        }
        ClientConfig::new(capabilities, Implementation::new(self.name, "1.0.0"))
    }

    /// Records the question and gives the configured answer.
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, McpError> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request
            && let Ok(mut asked) = self.asked.lock()
        {
            asked.push(message.clone());
        }
        Ok(ElicitResult::new(
            self.answer.clone().unwrap_or(ElicitationAction::Decline),
        ))
    }

    /// Records the progress message.
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.push(params.message.unwrap_or_default());
        }
    }
}

/// A connected server and client, with the fake's run log.
pub struct Harness {
    pub client: RunningService<RoleClient, TestClient>,
    pub handler: TestClient,
    /// A clone of the server, which shares its commands in flight.
    pub server: Server,
    /// Ends once the server stopped serving and its commands ended.
    pub served: tokio::task::JoinHandle<()>,
    pub dir: tempfile::TempDir,
    pub log: PathBuf,
}

/// The lifecycle of a test client.
#[derive(Clone, Copy)]
pub enum Lifecycle {
    /// `initialize` with 2025-11-25.
    Initialize,
    /// `server/discover`, then 2026-07-28 requests.
    Discover,
}

/// Serves a server for `config` (adjusted by `adjust`) to `client`.
pub async fn connect(
    client: TestClient,
    lifecycle: Lifecycle,
    adjust: impl FnOnce(&mut Config),
) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = dir.path().join("runs.jsonl");
    let mut env: BTreeMap<String, String> =
        ["PATH", "SystemRoot", "SYSTEMROOT", "TEMP", "TMP", "TMPDIR"]
            .into_iter()
            .filter_map(|name| Some((name.to_owned(), std::env::var(name).ok()?)))
            .collect();
    env.insert("FAKE_LOG".into(), log.display().to_string());
    let mut config = Config {
        exe: PathBuf::from(FAKE),
        read_only: false,
        lane: None,
        path: None,
        ui: Some(UiResource::placeholder()),
        env,
        cwd: dir.path().to_path_buf(),
        agent_detected: false,
    };
    adjust(&mut config);
    let (server_side, client_side) = tokio::io::duplex(1 << 20);
    let server = Server::new(config);
    let served = tokio::spawn({
        let server = server.clone();
        async move {
            let _ = devlanes_mcp::serve_on(server, server_side).await;
        }
    });
    let mode = match lifecycle {
        Lifecycle::Initialize => ClientLifecycleMode::Initialize,
        Lifecycle::Discover => ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    };
    let handler = client.clone();
    let client = client
        .serve_with_lifecycle(client_side, mode)
        .await
        .expect("client connects");
    Harness {
        client,
        handler,
        server,
        served,
        dir,
        log,
    }
}

/// Lines in the file at `path`, or 0 when there is none.
pub fn ticks(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
}

/// Waits until the process ticking into `path` stopped: the count stays the
/// same for a second, well over the 50 ms between ticks. Panics after `within`.
pub async fn stops_ticking(path: &std::path::Path, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    let mut last = ticks(path);
    let mut steady_since = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = ticks(path);
        if now != last {
            last = now;
            steady_since = tokio::time::Instant::now();
        } else if steady_since.elapsed() >= Duration::from_secs(1) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} kept ticking",
            path.display()
        );
    }
}

/// Waits until `path` has at least `count` ticks.
pub async fn ticked(path: &std::path::Path, count: usize) {
    while ticks(path) < count {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until the server runs no command, as after every call settled.
/// Panics after `within`.
pub async fn settled(server: &Server, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    while server.running_commands() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "a command still runs"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

impl Harness {
    /// Calls `tool` with `args` (and `cwd`, when given).
    pub async fn call(&self, tool: &'static str, args: &[&str]) -> CallToolResult {
        self.call_with(tool, json!({"args": args})).await
    }

    /// Calls `tool` with these arguments.
    pub async fn call_with(&self, tool: &'static str, arguments: Value) -> CallToolResult {
        let arguments = match arguments {
            Value::Object(arguments) => arguments,
            _ => JsonObject::new(),
        };
        self.client
            .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
            .await
            .expect("tool call")
    }

    /// Every run of the fake so far, `{args, cwd, from_lane, ai_agent, json_out}`,
    /// without the `debug events` runs that follow a start for progress (rmcp's
    /// client asks for progress on every call).
    pub fn runs(&self) -> Vec<Value> {
        self.all_runs()
            .into_iter()
            .filter(|run| {
                !run["args"]
                    .as_array()
                    .is_some_and(|args| args.contains(&json!("events")))
            })
            .collect()
    }

    /// Every run of the fake so far.
    pub fn all_runs(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The args of every run, joined by spaces.
    pub fn lines(&self) -> Vec<String> {
        self.runs()
            .iter()
            .map(|run| {
                run["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }
}

/// The text of a result's content.
pub fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.clone()))
        .collect()
}

/// Whether the result is an error.
pub fn failed(result: &CallToolResult) -> bool {
    result.is_error == Some(true)
}
