//! Tool calls: what runs, with which flags, env and directory, and what comes
//! back.

use std::time::Duration;

use rmcp::{
    model::{
        CallToolRequestParams, ClientRequest, NumberOrString, ProgressToken, Request,
        RequestMetaObject,
    },
    service::PeerRequestOptions,
};
use serde_json::{Value, json};

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

#[tokio::test]
/// A call runs the binary with `--output agent` and `DEVLANES_ENV_FROM_LANE`,
/// returns its output as text and its JSON with the view in `structuredContent`.
async fn a_call_runs_the_command() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |_| {}).await;
        let result = harness.call("devlanes_read", &["--json", "status"]).await;
        assert!(!failed(&result), "{}", text(&result));
        assert_eq!(text(&result), "ran --output agent status\n");
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["command"], "status");
        assert_eq!(structured["args"], json!(["status"]));
        assert_eq!(structured["exit_code"], 0);
        assert_eq!(structured["view"], "lane");
        assert_eq!(structured["data"]["items"][0]["reference"], "acme@1");
        assert_eq!(
            structured["cwd"],
            json!(harness.dir.path().display().to_string())
        );
        let run = &harness.runs()[0];
        assert_eq!(run["from_lane"], "1");
        assert_eq!(run["json_out"], true);
        assert_eq!(run["ai_agent"], "claude-code");
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A nonzero exit is `isError`; stderr follows stdout in the text.
async fn a_failed_command_is_an_error() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_EXIT".into(), "3".into());
        })
        .await;
        let result = harness.call("devlanes", &["stop", "api"]).await;
        assert!(failed(&result));
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["exit_code"], 3);
        assert_eq!(structured["view"], Value::Null);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// Blocking commands get the host's timeout; codex waits less than Claude Code.
/// An agent named by the environment keeps its own name.
async fn blocking_commands_get_the_hosts_timeout() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().named("codex-mcp-client"),
            Lifecycle::Initialize,
            |config| {
                config.agent_detected = true;
            },
        )
        .await;
        harness.call("devlanes", &["restart", "api"]).await;
        harness
            .call("devlanes", &["with", "--", "pnpm", "test"])
            .await;
        harness
            .call("devlanes", &["wait", "api", "--timeout", "5s"])
            .await;
        assert_eq!(
            harness.lines(),
            [
                "--output agent restart api --timeout 50s",
                "--output agent with --timeout 50s -- pnpm test",
                "--output agent wait api --timeout 5s",
            ]
        );
        assert_eq!(harness.runs()[0]["ai_agent"], Value::Null);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// Claude Code's `MCP_TOOL_TIMEOUT` in the server's environment sets the
/// limit, and a client the server does not know gets 60 s.
async fn the_hosts_tool_timeout_sets_the_limit() {
    tokio::time::timeout(LIMIT, async {
        let claude = connect(
            TestClient::new().named("claude-code"),
            Lifecycle::Initialize,
            |config| {
                config.agent_detected = true;
                config
                    .env
                    .insert("MCP_TOOL_TIMEOUT".into(), "600000".into());
            },
        )
        .await;
        claude.call("devlanes", &["start"]).await;
        let other = connect(
            TestClient::new().named("acme-client"),
            Lifecycle::Initialize,
            |config| {
                config.agent_detected = true;
            },
        )
        .await;
        other.call("devlanes", &["start"]).await;
        assert_eq!(claude.lines(), ["--output agent start --timeout 590s"]);
        assert_eq!(other.lines(), ["--output agent start --timeout 60s"]);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// The directory: the call's cwd first, then `--path`; `--lane` adds the flag.
async fn calls_run_where_asked() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.lane = Some("acme@2".into());
        })
        .await;
        let sub = harness.dir.path().join("sub");
        std::fs::create_dir(&sub).expect("sub");
        harness
            .call_with("devlanes_read", json!({"args": ["status"], "cwd": "sub"}))
            .await;
        harness.call("devlanes_read", &["status"]).await;
        let runs = harness.runs();
        assert_eq!(runs[0]["args"], json!(["--output", "agent", "status"]));
        assert_eq!(
            std::fs::canonicalize(runs[0]["cwd"].as_str().unwrap_or_default()).ok(),
            std::fs::canonicalize(&sub).ok()
        );
        assert_eq!(
            runs[1]["args"],
            json!(["--output", "agent", "status", "--lane", "acme@2"])
        );
        let other = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.path = Some(sub.clone());
        })
        .await;
        other.call("devlanes_read", &["status"]).await;
        assert_eq!(
            std::fs::canonicalize(other.runs()[0]["cwd"].as_str().unwrap_or_default()).ok(),
            std::fs::canonicalize(&sub).ok()
        );
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// `needs_env`: the call runs again with the server's env and says so.
async fn a_lane_without_env_gets_the_servers() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_NEEDS_ENV".into(), "1".into());
        })
        .await;
        let result = harness.call("devlanes", &["start"]).await;
        assert!(!failed(&result), "{}", text(&result));
        assert!(
            text(&result).contains("got the environment of the devlanes mcp server"),
            "{}",
            text(&result)
        );
        let from_lane: Vec<Value> = harness
            .runs()
            .iter()
            .map(|run| run["from_lane"].clone())
            .collect();
        assert_eq!(from_lane, [json!("1"), Value::Null]);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// Terminal-only commands, follows without `--timeout`, write commands on
/// `devlanes_read` and parse errors are refused without running anything;
/// `--help` is answered from the parser.
async fn refusals_run_nothing() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |_| {}).await;
        for (tool, args, says) in [
            ("devlanes", &["ui"][..], "TUI"),
            ("devlanes", &["attach", "api"], "logs"),
            ("devlanes", &["env", "shell"], "env show"),
            ("devlanes", &["start", "--attach"], "--attach"),
            ("devlanes", &["setup"], "terminal"),
            ("devlanes", &["update"], "terminal"),
            ("devlanes", &["uninstall"], "terminal"),
            ("devlanes", &["completions", "bash"], "terminal"),
            ("devlanes", &["mcp"], "MCP server"),
            ("devlanes_read", &["logs", "api", "-f"], "--timeout"),
            ("devlanes_read", &["status", "--watch"], "--timeout"),
            ("devlanes_read", &["debug", "events", "-f"], "--timeout"),
            ("devlanes_read", &["infra", "logs", "db", "-f"], "--timeout"),
            ("devlanes_read", &["daemon", "logs", "-f"], "--tail"),
            ("devlanes_read", &["stop"], "devlanes tool"),
            ("devlanes_read", &["lane", "clean", "2"], "devlanes tool"),
            ("devlanes", &["no-such-command"], "no-such-command"),
        ] {
            let result = harness.call(tool, args).await;
            assert!(failed(&result), "{args:?} was not refused");
            assert!(text(&result).contains(says), "{args:?}: {}", text(&result));
        }
        let help = harness.call("devlanes_read", &["logs", "--help"]).await;
        assert!(!failed(&help));
        assert!(text(&help).contains("--cursor"), "{}", text(&help));
        assert!(harness.runs().is_empty(), "ran {:?}", harness.lines());
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A start with a progress token reports the lane's phases from the events child.
async fn starts_report_progress() {
    tokio::time::timeout(LIMIT, async {
        let client = TestClient::new();
        let progress = client.progress.clone();
        let harness = connect(client, Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_SLEEP_MS".into(), "1500".into());
        })
        .await;
        let mut params = CallToolRequestParams::new("devlanes")
            .with_arguments(serde_json::from_value(json!({"args": ["start"]})).expect("args"));
        params.meta = Some(RequestMetaObject::with_progress_token(ProgressToken(
            NumberOrString::String("start-1".into()),
        )));
        let result = harness.client.call_tool(params).await.expect("call");
        assert!(!failed(&result), "{}", text(&result));
        // The client may handle the notifications after it has the result.
        let seen = || progress.lock().map(|seen| seen.clone()).unwrap_or_default();
        while seen().len() < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let seen = seen();
        assert_eq!(
            seen,
            [
                "acme@1 infra started",
                "acme@1 infra done",
                "acme@1 services started"
            ]
        );
        assert!(
            harness.all_runs().iter().any(|run| run["args"]
                == json!([
                    "--output",
                    "json",
                    "debug",
                    "events",
                    "-f",
                    "--timeout",
                    "175s"
                ])),
            "{:?}",
            harness.all_runs()
        );
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// `notifications/cancelled` kills the running command.
async fn cancelling_kills_the_command() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let ticks = dir.path().join("ticks");
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_SLEEP_MS".into(), "20000".into());
            config
                .env
                .insert("FAKE_TICKS".into(), ticks.display().to_string());
        })
        .await;
        let params = CallToolRequestParams::new("devlanes").with_arguments(
            serde_json::from_value(json!({"args": ["run", "build"]})).expect("args"),
        );
        let handle = harness
            .client
            .send_cancellable_request(
                ClientRequest::CallToolRequest(Request::new(params)),
                PeerRequestOptions::no_options(),
            )
            .await
            .expect("sent");
        let count = || {
            std::fs::read_to_string(&ticks)
                .map(|text| text.lines().count())
                .unwrap_or(0)
        };
        while count() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        handle.cancel(Some("test".into())).await.expect("cancel");
        // A killed child stops ticking: compare two counts well apart.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let after = count();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(count(), after, "the command kept running");
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// The app-only tools run JSON commands and return their documents. With a
/// box linked, a snapshot of every lane adds `remote ls`; one of a lane does not.
async fn app_tools_return_json() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().with_ui(),
            Lifecycle::Initialize,
            |config| {
                config.env.insert("FAKE_HOSTS".into(), "1".into());
            },
        )
        .await;
        let snapshot = harness
            .call_with("app_snapshot", json!({"lane": "acme@1", "metrics": true}))
            .await;
        let structured = snapshot.structured_content.expect("structured content");
        assert_eq!(structured["status"]["items"][0]["reference"], "acme@1");
        assert_eq!(structured["top"], json!({"lanes": []}));
        assert_eq!(structured["hosts"], Value::Null);
        let board = harness.call_with("app_snapshot", json!({})).await;
        let structured = board.structured_content.expect("structured content");
        assert_eq!(structured["hosts"]["items"][0]["host"], "hetzner");
        assert_eq!(structured["top"], Value::Null);
        let logs = harness
            .call_with(
                "app_logs",
                json!({"units": ["acme@1:api"], "tail": 50, "grep": "error"}),
            )
            .await;
        let structured = logs.structured_content.expect("structured content");
        assert_eq!(structured["logs"]["items"][0]["text"], "hello");
        let action = harness
            .call_with(
                "app_action",
                json!({"action": "stop", "target": "acme@1:api"}),
            )
            .await;
        let structured = action.structured_content.expect("structured content");
        assert_eq!(structured["exit_code"], 0);
        assert_eq!(
            structured["data"],
            json!({"ran": ["--output", "agent", "stop", "acme@1:api"]})
        );
        assert_eq!(
            harness.lines(),
            [
                "--output json status acme@1",
                "--output json debug top --lane acme@1",
                "--output json status",
                "--output json remote ls",
                "--output json logs acme@1:api --tail 50 --grep error",
                "--output agent stop acme@1:api",
            ]
        );
    })
    .await
    .expect("in time");
}
