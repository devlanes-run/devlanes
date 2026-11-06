//! Tools only the MCP Apps cards call (`_meta.ui.visibility: ["app"]`): a
//! snapshot of the lanes, log lines, and the cards' buttons.

use rmcp::{
    ErrorData as McpError, RoleServer,
    model::{CallToolRequestParams, CallToolResponse, JsonObject, MetaObject, Tool},
    service::RequestContext,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    command::{Call, Rejected},
    host,
    server::{Server, arguments, client_name, execute, text_result},
    text,
};

/// The app-only tool names.
const NAMES: [&str; 3] = ["app_snapshot", "app_logs", "app_action"];

/// Whether `name` is an app-only tool.
pub fn is_app_tool(name: &str) -> bool {
    NAMES.contains(&name)
}

/// The app-only tools; `app_action` only without `--read-only`.
pub fn tools(read_only: bool) -> Vec<Tool> {
    let mut meta = MetaObject::new();
    meta.insert("ui".into(), json!({"visibility": ["app"]}));
    let tool = |name: &'static str, description: &'static str, schema: Value| {
        let schema = match schema {
            Value::Object(schema) => schema,
            _ => JsonObject::new(),
        };
        Tool::new(name, description, schema).with_meta(meta.clone())
    };
    let cwd = json!({"type": "string", "description": text::CWD});
    let mut tools = vec![
        tool(
            "app_snapshot",
            text::APP_SNAPSHOT,
            json!({
                "type": "object",
                "properties": {
                    "cwd": cwd,
                    "lane": {"type": "string"},
                    "metrics": {"type": "boolean"},
                },
            }),
        ),
        tool(
            "app_logs",
            text::APP_LOGS,
            json!({
                "type": "object",
                "properties": {
                    "cwd": cwd,
                    "units": {"type": "array", "items": {"type": "string"}},
                    "tail": {"type": "integer", "minimum": 1},
                    "cursor": {"type": "string"},
                    "grep": {"type": "string"},
                    "since": {"type": "string"},
                },
                "required": ["units"],
            }),
        ),
    ];
    if !read_only {
        tools.push(tool(
            "app_action",
            text::APP_ACTION,
            json!({
                "type": "object",
                "properties": {
                    "cwd": cwd,
                    "action": {
                        "type": "string",
                        "enum": ["start", "stop", "restart", "reload", "clean", "clean_plan", "pair"],
                    },
                    "target": {"type": "string"},
                },
                "required": ["action"],
            }),
        ));
    }
    tools
}

/// `app_snapshot` arguments.
#[derive(Deserialize)]
struct Snapshot {
    cwd: Option<String>,
    lane: Option<String>,
    #[serde(default)]
    metrics: bool,
}

/// `app_logs` arguments.
#[derive(Deserialize)]
struct Logs {
    cwd: Option<String>,
    units: Vec<String>,
    tail: Option<u64>,
    cursor: Option<String>,
    grep: Option<String>,
    since: Option<String>,
}

/// `app_action` arguments.
#[derive(Deserialize)]
struct Action {
    cwd: Option<String>,
    action: String,
    target: Option<String>,
}

/// Runs the app-only tool `name`.
pub async fn call(
    server: &Server,
    name: &str,
    request: &CallToolRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResponse, McpError> {
    let client = client_name(context);
    let runner = server.runner(&client);
    match name {
        "app_snapshot" => {
            let args: Snapshot = arguments(request)?;
            let (cwd, default_lane) = server.cwd(args.cwd.as_deref());
            let mut status = vec!["status".to_owned()];
            status.extend(args.lane.clone());
            if args.lane.is_none() && default_lane {
                status.extend(server.lane_flag());
            }
            let output = runner.json(&cwd, &status, &context.ct).await.map_err(io)?;
            if output.exit_code != 0 {
                return Ok(text_result(output.stderr, true).into());
            }
            let top = if args.metrics {
                let mut top = vec!["debug".to_owned(), "top".to_owned()];
                if let Some(lane) = &args.lane {
                    top.extend(["--lane".to_owned(), lane.clone()]);
                } else if default_lane {
                    top.extend(server.lane_flag());
                }
                let output = runner.json(&cwd, &top, &context.ct).await.map_err(io)?;
                if output.exit_code == 0 {
                    output.data
                } else {
                    Value::Null
                }
            } else {
                Value::Null
            };
            let lanes = output
                .data
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let mut result = text_result(format!("{lanes} lanes"), false);
            result.structured_content = Some(json!({"status": output.data, "top": top}));
            Ok(result.into())
        }
        "app_logs" => {
            let args: Logs = arguments(request)?;
            if args.units.is_empty() {
                return Err(McpError::invalid_params("app_logs needs units", None));
            }
            let (cwd, default_lane) = server.cwd(args.cwd.as_deref());
            let mut logs = vec!["logs".to_owned()];
            logs.extend(args.units);
            for (flag, value) in [
                ("--tail", args.tail.map(|tail| tail.to_string())),
                ("--cursor", args.cursor),
                ("--grep", args.grep),
                ("--since", args.since),
            ] {
                if let Some(value) = value {
                    logs.extend([flag.to_owned(), value]);
                }
            }
            if default_lane {
                logs.extend(server.lane_flag());
            }
            let output = runner.json(&cwd, &logs, &context.ct).await.map_err(io)?;
            if output.exit_code != 0 {
                return Ok(text_result(output.stderr, true).into());
            }
            let mut result = text_result("log lines".to_owned(), false);
            result.structured_content = Some(json!({"logs": output.data}));
            Ok(result.into())
        }
        "app_action" if !server.read_only() => {
            let args: Action = arguments(request)?;
            let line = action_args(&args.action, args.target.as_deref()).ok_or_else(|| {
                McpError::invalid_params(format!("unknown action {}", args.action), None)
            })?;
            let mut call = match Call::parse(&line, false) {
                Ok(call) => call,
                Err(Rejected::Help(text) | Rejected::Error(text)) => {
                    return Ok(text_result(text, true).into());
                }
            };
            let (cwd, default_lane) = server.cwd(args.cwd.as_deref());
            if default_lane && let Some(lane) = server.lane() {
                call.default_lane(lane);
            }
            let limit = call.limit(host::default_timeout(&client));
            let (output, notes) = execute(&runner, &call, &cwd, limit, context).await?;
            let mut text = output.stdout.clone();
            for extra in std::iter::once(&output.stderr).chain(&notes) {
                if !extra.trim().is_empty() {
                    text.push_str(extra.trim_end());
                    text.push('\n');
                }
            }
            let mut result = text_result(text.clone(), output.exit_code != 0);
            result.structured_content = Some(json!({
                "exit_code": output.exit_code,
                "text": text,
                "data": output.data,
            }));
            Ok(result.into())
        }
        name => Err(McpError::invalid_params(
            format!("unknown tool {name}"),
            None,
        )),
    }
}

/// The command line of a card's button. A target with `:` names a unit, any
/// other a lane. `clean` passes `--yes`: the click in the card is the consent.
fn action_args(action: &str, target: Option<&str>) -> Option<Vec<String>> {
    let owned = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let units = |command: &str| match target {
        Some(unit) if unit.contains(':') => owned(&[command, unit]),
        Some(lane) => owned(&[command, "--lane", lane]),
        None => owned(&[command]),
    };
    Some(match action {
        "start" | "stop" | "restart" | "reload" => units(action),
        "clean" | "clean_plan" => {
            let mut args = owned(&["lane", "clean"]);
            args.extend(target.map(str::to_owned));
            args.push(
                if action == "clean" {
                    "--yes"
                } else {
                    "--dry-run"
                }
                .to_owned(),
            );
            args
        }
        "pair" => owned(&["daemon", "pair"]),
        _ => return None,
    })
}

/// A failure to start the devlanes process.
fn io(error: std::io::Error) -> McpError {
    McpError::internal_error(format!("could not run devlanes: {error}"), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Buttons map to the commands the CLI runs.
    #[test]
    fn button_commands() {
        let line = |action, target| action_args(action, target).map(|args| args.join(" "));
        assert_eq!(
            line("stop", Some("acme@1:api")).as_deref(),
            Some("stop acme@1:api")
        );
        assert_eq!(
            line("start", Some("acme@1")).as_deref(),
            Some("start --lane acme@1")
        );
        assert_eq!(line("restart", None).as_deref(), Some("restart"));
        assert_eq!(
            line("clean", Some("acme@4")).as_deref(),
            Some("lane clean acme@4 --yes")
        );
        assert_eq!(
            line("clean_plan", Some("acme@4")).as_deref(),
            Some("lane clean acme@4 --dry-run")
        );
        assert_eq!(line("pair", None).as_deref(), Some("daemon pair"));
        assert_eq!(line("delete", None), None);
    }
}
