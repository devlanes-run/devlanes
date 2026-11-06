//! The MCP handler: tools, the cards resource, consent and results.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequest,
        ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, Implementation,
        InputRequest, InputRequests, InputRequiredResult, JsonObject, ListResourcesResult,
        ListToolsResult, MetaObject, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
        ResourceContents, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    Config, UI_EXTENSION, app,
    child::{Calls, Output, Runner},
    command::{Call, Rejected, Tool as Source},
    host,
    progress::{self, Phases},
    text, view,
};

/// The key of the consent question in an `InputRequiredResult`.
const CONSENT: &str = "consent";

/// Most bytes of JSON `structuredContent.data` carries; hosts keep the whole
/// result in the agent's context, and the text already says what happened.
const DATA_LIMIT: usize = 256 * 1024;

/// The MCP server of `devlanes mcp`. Cheap to clone; every clone serves the same
/// config and shares the commands in flight.
#[derive(Clone)]
pub struct Server {
    config: Arc<Config>,
    calls: Calls,
}

/// `{args, cwd?}`, the arguments of both model-facing tools.
#[derive(Deserialize)]
struct ToolArgs {
    args: Vec<String>,
    cwd: Option<String>,
}

/// The user's answer to a consent question.
pub(crate) enum Answer {
    Accepted,
    Declined,
    /// The question went away unanswered: the user dismissed it, or the call
    /// was cancelled.
    Cancelled,
    /// The client cannot ask the user.
    Unsupported,
    /// The 2026-07-28 protocol asks through the client: return this result,
    /// and the client calls again with the answer.
    Pending(InputRequiredResult),
}

impl Server {
    /// A server for `config`. It starts no process until a tool is called.
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            calls: Calls::default(),
        }
    }

    /// How many devlanes processes calls are running now.
    pub fn running_commands(&self) -> usize {
        self.calls.running()
    }

    /// Interrupts the commands of calls still in flight, as cancelling them
    /// would, and waits until they ended.
    pub(crate) async fn stop_commands(&self) {
        self.calls.stop().await;
    }

    /// The tools this client sees: `devlanes` unless `--read-only`, `devlanes_read`,
    /// and the app-only tools when it shows MCP Apps.
    fn tools(&self, ui: bool) -> Vec<Tool> {
        let schema = tool_schema();
        let mut meta = MetaObject::new();
        if ui && let Some(resource) = &self.config.ui {
            meta.insert("ui".into(), json!({"resourceUri": resource.uri}));
        }
        let with_meta = |tool: Tool| {
            if meta.is_empty() {
                tool
            } else {
                tool.with_meta(meta.clone())
            }
        };
        let mut tools = Vec::new();
        if !self.config.read_only {
            tools.push(with_meta(
                Tool::new("devlanes", text::DEVLANES, schema.clone()).with_annotations(
                    ToolAnnotations::with_title("devlanes")
                        .read_only(false)
                        .destructive(true)
                        .open_world(false),
                ),
            ));
        }
        let read = if self.config.read_only {
            text::DEVLANES_READ_ONLY
        } else {
            text::DEVLANES_READ
        };
        tools.push(with_meta(
            Tool::new("devlanes_read", read, schema).with_annotations(
                ToolAnnotations::with_title("devlanes (read only)")
                    .read_only(true)
                    .open_world(false),
            ),
        ));
        if ui && self.config.ui.is_some() {
            tools.extend(app::tools(self.config.read_only));
        }
        tools
    }

    /// Runs a call of `devlanes` or `devlanes_read`.
    async fn command(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let read_only_tool = request.name == "devlanes_read";
        let source = match (read_only_tool, self.config.read_only) {
            (false, _) => Source::Devlanes,
            (true, false) => Source::Read,
            (true, true) => Source::ReadOnly,
        };
        let arguments: ToolArgs = arguments(request)?;
        let mut call = match Call::parse(&arguments.args, source) {
            Ok(call) => call,
            Err(Rejected::Help(text)) => return Ok(text_result(text, false).into()),
            Err(Rejected::Error(text)) => return Ok(text_result(text, true).into()),
        };
        let (cwd, default_lane) = self.cwd(arguments.cwd.as_deref());
        if default_lane && let Some(lane) = &self.config.lane {
            call.default_lane(lane);
        }
        let client = client_name(context);
        let limit = call.limit(self.default_timeout(&client));
        let runner = self.runner(&client);
        // An answer the client gives on its second call (2026-07-28) comes first.
        match consent_answer(request) {
            Some((Some(true), flag)) => call.confirm(&flag),
            Some((answer, _)) => return Ok(declined(&call, answer.is_none()).into()),
            None => {
                if let Some(question) = call.consent() {
                    // The dry run is only worth its process when the user will see it.
                    let message = if can_ask(context) {
                        consent_message(&runner, &call, &cwd, &question, context).await
                    } else {
                        question.clone()
                    };
                    match ask(context, message, "--yes").await {
                        Answer::Accepted => call.confirm("--yes"),
                        Answer::Declined => return Ok(declined(&call, false).into()),
                        Answer::Cancelled => return Ok(declined(&call, true).into()),
                        Answer::Unsupported => {
                            return Ok(text_result(
                                format!(
                                    "{question}\nThis needs the user's consent and this client \
                                     cannot ask: ask the user, then rerun with --yes"
                                ),
                                true,
                            )
                            .into());
                        }
                        Answer::Pending(result) => return Ok(result.into()),
                    }
                }
            }
        }
        let (mut output, mut notes) = match execute(&runner, &call, &cwd, limit, context).await {
            Ok(ran) => ran,
            Err(error) => return Ok(text_result(error, true).into()),
        };
        // devlanes_read runs nothing that asks; a yes there would change something.
        if !read_only_tool
            && !call.cli.yes
            && let Some(error) = error_of(&output.data, "confirmation_required")
        {
            let flag = error
                .pointer("/data/flag")
                .and_then(Value::as_str)
                .unwrap_or("--yes")
                .to_owned();
            let message = format!("devlanes {}\n{}", call.args.join(" "), output.stderr.trim());
            match ask(context, message, &flag).await {
                Answer::Accepted => {
                    call.confirm(&flag);
                    (output, notes) = match execute(&runner, &call, &cwd, limit, context).await {
                        Ok(ran) => ran,
                        Err(error) => return Ok(text_result(error, true).into()),
                    };
                }
                Answer::Declined => return Ok(declined(&call, false).into()),
                Answer::Cancelled => return Ok(declined(&call, true).into()),
                Answer::Unsupported => {
                    notes.push(format!("ask the user, then rerun with {flag}"));
                }
                Answer::Pending(result) => return Ok(result.into()),
            }
        }
        Ok(result(&call, &cwd, &output, &notes, self.config.read_only).into())
    }

    /// The directory a call runs in, and whether `--lane` applies: the call's
    /// `cwd`, else `--path`, else (with `--lane`) the project directory, else
    /// `$CLAUDE_PROJECT_DIR`, else the server's own.
    pub(crate) fn cwd(&self, cwd: Option<&str>) -> (PathBuf, bool) {
        if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
            return (self.config.cwd.join(cwd), false);
        }
        if let Some(path) = &self.config.path {
            return (path.clone(), false);
        }
        let project = self
            .config
            .env
            .get("CLAUDE_PROJECT_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        (
            project.unwrap_or_else(|| self.config.cwd.clone()),
            self.config.lane.is_some(),
        )
    }

    /// The runner for one call: the server's environment, plus `AI_AGENT` from the
    /// client's name when the environment names no agent, so units started here
    /// carry the same actor as the agent's shell commands.
    pub(crate) fn runner(&self, client: &str) -> Runner {
        let mut env = self.config.env.clone();
        if !self.config.agent_detected
            && let Some(agent) = host::agent_id(client)
        {
            env.insert("AI_AGENT".into(), agent.into());
        }
        Runner {
            exe: self.config.exe.clone(),
            env,
            calls: self.calls.clone(),
        }
    }
}

impl ServerHandler for Server {
    /// The server's identity, capabilities and instructions.
    fn get_info(&self) -> ServerConfig {
        let mut capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        if self.config.ui.is_some() {
            capabilities.extensions = Some(BTreeMap::from([(
                UI_EXTENSION.to_owned(),
                JsonObject::new(),
            )]));
        }
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("devlanes", devlanes_version::VERSION))
            .with_instructions(if self.config.read_only {
                text::READ_ONLY_INSTRUCTIONS
            } else {
                text::INSTRUCTIONS
            })
    }

    /// The tools, per [`Server::tools`] for this request's client.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            self.tools(self.shows_ui(&context)),
        ))
    }

    /// Runs the named tool; failures of the command are results with `isError`.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        match request.name.as_ref() {
            "devlanes" if !self.config.read_only => self.command(&request, &context).await,
            "devlanes_read" => self.command(&request, &context).await,
            // Only the cards call these, so a client that shows none has no use for them.
            name if app::is_app_tool(name) && self.shows_ui(&context) => {
                app::call(self, name, &request, &context).await
            }
            name => Err(McpError::invalid_params(
                format!("unknown tool {name}"),
                None,
            )),
        }
    }

    /// The cards resource, for clients that show MCP Apps.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = match &self.config.ui {
            Some(ui) if self.shows_ui(&context) => vec![
                Resource::new(ui.uri, "devlanes lanes")
                    .with_description(
                        "Cards for devlanes results: lanes, failures, clean plans and pairing codes",
                    )
                    .with_mime_type(ui.mime),
            ],
            _ => Vec::new(),
        };
        Ok(ListResourcesResult::with_all_items(resources))
    }

    /// The HTML of the cards.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        match &self.config.ui {
            Some(ui) if request.uri == ui.uri => {
                let mut meta = MetaObject::new();
                // The card draws its own window, so a host border would frame it twice.
                meta.insert("ui".into(), json!({"prefersBorder": false}));
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text((ui.html)(), ui.uri)
                        .with_mime_type(ui.mime)
                        .with_meta(meta),
                ])
                .into())
            }
            _ => Err(McpError::resource_not_found(
                format!("no resource {}", request.uri),
                None,
            )),
        }
    }
}

impl Server {
    /// `--read-only`.
    pub(crate) fn read_only(&self) -> bool {
        self.config.read_only
    }

    /// The `--timeout` for blocking commands of `client` that name none.
    pub(crate) fn default_timeout(&self, client: &str) -> std::time::Duration {
        host::default_timeout(client, &self.config.env)
    }

    /// `--lane`.
    pub(crate) fn lane(&self) -> Option<&str> {
        self.config.lane.as_deref()
    }

    /// `["--lane", REF]` for `--lane`, or nothing.
    pub(crate) fn lane_flag(&self) -> Vec<String> {
        self.lane()
            .map(|lane| vec!["--lane".to_owned(), lane.to_owned()])
            .unwrap_or_default()
    }

    /// Whether this request's client shows MCP Apps in the cards' MIME type.
    fn shows_ui(&self, context: &RequestContext<RoleServer>) -> bool {
        let Some(ui) = &self.config.ui else {
            return false;
        };
        context
            .client_capabilities()
            .and_then(|capabilities| capabilities.extensions)
            .and_then(|extensions| extensions.get(UI_EXTENSION).cloned())
            .and_then(|settings| settings.get("mimeTypes").cloned())
            .and_then(|types| types.as_array().cloned())
            .is_some_and(|types| types.iter().any(|mime| mime == ui.mime))
    }
}

/// The input schema both model-facing tools share.
fn tool_schema() -> JsonObject {
    let schema = json!({
        "type": "object",
        "properties": {
            "args": {"type": "array", "items": {"type": "string"}, "description": text::ARGS},
            "cwd": {"type": "string", "description": text::CWD},
        },
        "required": ["args"],
        "additionalProperties": false,
    });
    match schema {
        Value::Object(schema) => schema,
        _ => JsonObject::new(),
    }
}

/// The call's arguments as `T`, or an invalid-params error.
pub(crate) fn arguments<T: serde::de::DeserializeOwned>(
    request: &CallToolRequestParams,
) -> Result<T, McpError> {
    let arguments = Value::Object(request.arguments.clone().unwrap_or_default());
    serde_json::from_value(arguments)
        .map_err(|error| McpError::invalid_params(format!("{}: {error}", request.name), None))
}

/// The client's name, from this request (2026-07-28) or the handshake.
pub(crate) fn client_name(context: &RequestContext<RoleServer>) -> String {
    context
        .client_info()
        .map(|info| info.name)
        .unwrap_or_default()
}

/// Runs `call` with the lane's env first; when the lane has none, again with the
/// server's, saying so in a note. Phases go to the client as progress while a
/// start runs, counted over both runs. Fails with the text to show when the
/// command could not be started, such as for a `cwd` that does not exist.
pub(crate) async fn execute(
    runner: &Runner,
    call: &Call,
    cwd: &Path,
    limit: Option<Duration>,
    context: &RequestContext<RoleServer>,
) -> Result<(Output, Vec<String>), String> {
    let mut notes = Vec::new();
    let mut phases = Phases::of(call.cli.lane.as_deref());
    let mut output = run(runner, call, cwd, limit, true, &mut phases, context).await?;
    if error_of(&output.data, "needs_env").is_some() && !output.cancelled {
        output = run(runner, call, cwd, limit, false, &mut phases, context).await?;
        notes.push(
            "note: the lane had no environment from a shell, so it got the environment of the \
             devlanes mcp server"
                .to_owned(),
        );
    }
    Ok((output, notes))
}

/// One run of `call`, with progress for starts when the request asked for it.
async fn run(
    runner: &Runner,
    call: &Call,
    cwd: &Path,
    limit: Option<Duration>,
    from_lane: bool,
    phases: &mut Phases,
    context: &RequestContext<RoleServer>,
) -> Result<Output, String> {
    let token = context.meta.get_progress_token();
    let follow = async {
        if let (true, Some(token), Some(limit)) = (call.starts(), token, limit) {
            progress::follow(runner, cwd, limit, context.peer.clone(), token, phases).await;
        }
        std::future::pending::<()>().await;
    };
    let run = runner.run(cwd, &call.args, from_lane, &context.ct);
    // `follow` never ends, so the command's end decides.
    let output = tokio::select! {
        output = run => output,
        () = follow => unreachable!("following progress never ends"),
    };
    output.map_err(|error| {
        format!(
            "could not run {} in {}: {error}",
            runner.exe.display(),
            cwd.display()
        )
    })
}

/// The `error` of an `{"error": ...}` document with this `code` the command
/// wrote, also as one line of a stream such as the one `with` writes.
pub(crate) fn error_of<'a>(data: &'a Value, code: &str) -> Option<&'a Value> {
    let is = |document: &&'a Value| {
        document.pointer("/error/code").and_then(Value::as_str) == Some(code)
    };
    match data {
        Value::Array(documents) => documents.iter().find(is),
        document => Some(document).filter(is),
    }
    .and_then(|document| document.get("error"))
}

/// The tool result of a finished command, for a server with `read_only`.
fn result(
    call: &Call,
    cwd: &Path,
    output: &Output,
    notes: &[String],
    read_only: bool,
) -> CallToolResult {
    let (data, truncated) = capped(&output.data);
    let mut text = output.stdout.clone();
    let mut line = |extra: &str| {
        if extra.trim().is_empty() {
            return;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(extra.trim_end());
        text.push('\n');
    };
    line(&output.stderr);
    for note in notes {
        line(note);
    }
    if output.cancelled {
        line(
            "cancelled, as Ctrl-C would in a terminal; services keep running (devlanes status \
             shows them)",
        );
    }
    if truncated {
        line(&format!(
            "note: the JSON output is over {} KiB, so structuredContent leaves it out",
            DATA_LIMIT / 1024
        ));
    }
    if text.trim().is_empty() {
        text = format!("devlanes {} exited {}\n", call.path, output.exit_code);
    }
    let mut result = text_result(text, output.exit_code != 0 || output.cancelled);
    result.structured_content = Some(json!({
        "command": call.path,
        "args": call.args,
        "cwd": cwd,
        "exit_code": output.exit_code,
        "view": view::select(&call.path, output.exit_code, &output.data),
        "data": data,
        "data_truncated": truncated,
        "read_only": read_only,
    }));
    result
}

/// `data`, or `Null` and `true` when its JSON is over [`DATA_LIMIT`].
pub(crate) fn capped(data: &Value) -> (Value, bool) {
    let size = serde_json::to_vec(data).map_or(0, |bytes| bytes.len());
    if size > DATA_LIMIT {
        (Value::Null, true)
    } else {
        (data.clone(), false)
    }
}

/// A result of one text block.
pub(crate) fn text_result(text: String, is_error: bool) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.is_error = Some(is_error);
    result
}

/// The result when the user said no, or when the question was `cancelled`
/// before the user answered.
fn declined(call: &Call, cancelled: bool) -> CallToolResult {
    let why = if cancelled {
        "the question to the user was cancelled"
    } else {
        "the user declined"
    };
    text_result(
        format!("{why}; devlanes {} did not run", call.args.join(" ")),
        true,
    )
}

/// The answer in a retried call (2026-07-28): yes, no, or `None` when the
/// question was cancelled, and the flag that accepting adds, kept in the
/// request state.
fn consent_answer(request: &CallToolRequestParams) -> Option<(Option<bool>, String)> {
    let answer = request.input_responses.as_ref()?.get(CONSENT)?;
    let accepted = match serde_json::from_value::<ElicitResult>(answer.clone()) {
        Ok(answer) => match answer.action {
            ElicitationAction::Accept => Some(true),
            ElicitationAction::Cancel => None,
            _ => Some(false),
        },
        Err(_) => Some(false),
    };
    let flag = request
        .request_state
        .clone()
        .filter(|flag| {
            flag.starts_with("--")
                && flag.len() > 2
                && !flag.contains(|c: char| c.is_whitespace() || c == '=')
        })
        .unwrap_or_else(|| "--yes".into());
    Some((accepted, flag))
}

/// The consent question with what is at stake: for `lane clean`, what its dry
/// run lists.
async fn consent_message(
    runner: &Runner,
    call: &Call,
    cwd: &Path,
    question: &str,
    context: &RequestContext<RoleServer>,
) -> String {
    let mut message = question.to_owned();
    if let Some(plan) = call.clean_plan()
        && let Ok(output) = runner.run(cwd, &plan, true, &context.ct).await
        && output.exit_code == 0
        && !output.stdout.trim().is_empty()
    {
        message.push_str("\n\n");
        message.push_str(output.stdout.trim_end());
    }
    message.push_str(&format!("\n\ndevlanes {}", call.args.join(" ")));
    message
}

/// Whether the client of this request can put a question to the user.
fn can_ask(context: &RequestContext<RoleServer>) -> bool {
    context
        .client_capabilities()
        .is_some_and(|capabilities| capabilities.elicitation.is_some())
}

/// Asks the user `message` through the client. Before 2026-07-28 that is an
/// `elicitation/create` request; from then on a result that makes the client ask
/// and call again, with `flag` in its state.
pub(crate) async fn ask(
    context: &RequestContext<RoleServer>,
    message: String,
    flag: &str,
) -> Answer {
    if !can_ask(context) {
        return Answer::Unsupported;
    }
    let params = ElicitRequestParams::FormElicitationParams {
        meta: None,
        message,
        requested_schema: ElicitationSchema::new(BTreeMap::new()),
    };
    let inline = context
        .protocol_version()
        .is_some_and(|version| version.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
    if inline {
        let mut requests = InputRequests::new();
        requests.insert(
            CONSENT.into(),
            InputRequest::Elicitation(ElicitRequest::new(params)),
        );
        return Answer::Pending(InputRequiredResult::new(
            Some(requests),
            Some(flag.to_owned()),
        ));
    }
    let asked = context.peer.create_elicitation(params);
    let answer = tokio::select! {
        answer = asked => answer,
        () = context.ct.cancelled() => return Answer::Cancelled,
    };
    match answer.map(|answer| answer.action) {
        Ok(ElicitationAction::Accept) => Answer::Accepted,
        Ok(ElicitationAction::Cancel) => Answer::Cancelled,
        Ok(_) => Answer::Declined,
        Err(error) => {
            tracing::debug!(%error, "elicitation failed");
            Answer::Unsupported
        }
    }
}
