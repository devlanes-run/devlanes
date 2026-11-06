//! What clients see before they call anything: the handshake, tools,
//! instructions and the cards resource.

use rmcp::model::{ProtocolVersion, ReadResourceRequestParams, ResourceContents};
use serde_json::{Value, json};

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

/// `tools/list` as JSON, for snapshots.
async fn tools(harness: &crate::Harness) -> Value {
    let tools = harness.client.list_tools(None).await.expect("tools/list");
    serde_json::to_value(tools.tools).expect("json")
}

/// The names in a `tools/list`.
fn names(tools: &Value) -> Vec<String> {
    tools
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The tools of every curated command, in `tools/list` order.
const CURATED: [&str; 20] = [
    "status",
    "start",
    "exec",
    "logs",
    "stop",
    "restart",
    "reload",
    "wait",
    "with",
    "run",
    "debug_why",
    "debug_errors",
    "debug_top",
    "url_print",
    "url_ls",
    "env_show",
    "lane_ls",
    "lane_clean",
    "daemon_pair",
    "doctor",
];

/// The curated tools that change nothing.
const READ_ONLY: [&str; 11] = [
    "status",
    "logs",
    "wait",
    "debug_why",
    "debug_errors",
    "debug_top",
    "url_print",
    "url_ls",
    "env_show",
    "lane_ls",
    "doctor",
];

#[tokio::test]
/// `initialize` at 2025-11-25 gives the instructions and the model-facing tools;
/// starting the server and listing runs no devlanes process, so no daemon.
async fn initialize_lists_the_tools() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |_| {}).await;
        let info = harness.client.peer_info().expect("server info");
        assert_eq!(info.protocol_version, ProtocolVersion::V_2025_11_25);
        assert_eq!(
            info.server_info.as_ref().map(|server| server.name.as_str()),
            Some("devlanes")
        );
        insta::assert_snapshot!(
            "instructions",
            info.instructions.clone().unwrap_or_default()
        );
        insta::assert_json_snapshot!("tools_without_ui", tools(&harness).await);
        let resources = harness
            .client
            .list_resources(None)
            .await
            .expect("resources/list");
        insta::assert_json_snapshot!("resources_without_ui", resources.resources);
        assert!(harness.runs().is_empty(), "ran {:?}", harness.lines());
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A client with the MCP Apps extension also gets the app-only tools, the
/// cards resource and `_meta.ui.resourceUri` on the model-facing tools.
async fn mcp_apps_clients_get_the_cards() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new().with_ui(), Lifecycle::Initialize, |_| {}).await;
        let tools = tools(&harness).await;
        insta::assert_json_snapshot!("tools_with_ui", tools);
        let resources = harness
            .client
            .list_resources(None)
            .await
            .expect("resources/list");
        insta::assert_json_snapshot!("resources_with_ui", resources.resources);
        let read = harness
            .client
            .read_resource(ReadResourceRequestParams::new("ui://devlanes/lane.html"))
            .await
            .expect("resources/read");
        let [
            ResourceContents::TextResourceContents {
                mime_type, text, ..
            },
        ] = &read.contents[..]
        else {
            panic!("one text resource: {:?}", read.contents);
        };
        assert_eq!(mime_type.as_deref(), Some("text/html;profile=mcp-app"));
        assert!(text.contains("<html"), "{text}");
        // The host frames the card, and Claude's fonts may load.
        let contents = serde_json::to_value(&read.contents[0]).expect("json");
        assert_eq!(
            contents["_meta"]["ui"],
            json!({"prefersBorder": true, "csp": {"resourceDomains": ["https://assets.claude.ai"]}})
        );
        let missing = harness
            .client
            .read_resource(ReadResourceRequestParams::new("ui://devlanes/other.html"))
            .await;
        assert!(missing.is_err(), "{missing:?}");
        assert!(harness.runs().is_empty(), "ran {:?}", harness.lines());
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// `server/discover` and 2026-07-28 requests work like the handshake, with the
/// client's name and capabilities taken from each request.
async fn discover_then_call() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new().with_ui(), Lifecycle::Discover, |_| {}).await;
        let mut expected: Vec<&str> = CURATED.to_vec();
        expected.extend(["devlanes", "app_snapshot", "app_logs", "app_action"]);
        assert_eq!(names(&tools(&harness).await), expected);
        let result = harness.call_with("start", json!({})).await;
        assert!(!failed(&result), "{}", text(&result));
        // Claude Code's name gives its timeout and agent.
        assert_eq!(
            harness.runs()[0]["args"],
            json!(["--output", "agent", "start", "--timeout", "170s"])
        );
        assert_eq!(harness.runs()[0]["ai_agent"], "claude-code");
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// `--read-only` offers only the tools that change nothing, and a `devlanes`
/// tool that refuses the rest; calling a tool that changes something is an
/// error.
async fn read_only_offers_only_reading_tools() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().with_ui(),
            Lifecycle::Initialize,
            |config| {
                config.read_only = true;
            },
        )
        .await;
        let mut expected: Vec<&str> = READ_ONLY.to_vec();
        expected.extend(["devlanes", "app_snapshot", "app_logs"]);
        assert_eq!(names(&tools(&harness).await), expected);
        // Nothing they say points at tools that are missing.
        insta::assert_json_snapshot!("tools_read_only", tools(&harness).await);
        let info = harness.client.peer_info().expect("server info");
        insta::assert_snapshot!(
            "instructions_read_only",
            info.instructions.clone().unwrap_or_default()
        );
        for args in [&["stop"][..], &["lane", "clean", "2"]] {
            let refused = harness.call("devlanes", args).await;
            assert!(failed(&refused));
            assert!(
                text(&refused).contains("ask the user to run it"),
                "{}",
                text(&refused)
            );
        }
        for (tool, arguments) in [
            ("stop", json!({})),
            ("exec", json!({"name": "web", "command": ["pnpm", "dev"]})),
            ("app_action", json!({"action": "stop"})),
        ] {
            let refused = harness
                .client
                .call_tool(
                    rmcp::model::CallToolRequestParams::new(tool)
                        .with_arguments(serde_json::from_value(arguments).expect("args")),
                )
                .await;
            assert!(refused.is_err(), "{tool}: {refused:?}");
        }
        assert!(harness.runs().is_empty(), "ran {:?}", harness.lines());
    })
    .await
    .expect("in time");
}
