//! Tool results and app-only tool answers for the previews, built from
//! `devlanes_proto` types so a change to the daemon's data shapes breaks this
//! file. Shapes proto does not have (the clean dry-run, `debug top`, the
//! structured content the MCP server wraps around them) are JSON literals that
//! follow the contracts between the server and the card.

use std::collections::BTreeMap;

use devlanes_proto::{
    Actor, ActorKind, ContainerSummary, LanePower, LaneSummary, LogLine, Page, RpcError,
    StartResult, UnitFailureData, UnitSummary,
};
use serde_json::{Value, json};

/// The clock every fixture is written against. The fake host moves all times by
/// the difference to the real clock, so "3m ago" stays 3 minutes.
pub const NOW: u64 = 1_794_000_000_000;

/// The working directory of every fixture's tool call.
const CWD: &str = "/work/acme";

/// One preview: a tool call, its result and the answers to the card's app-only
/// tool calls.
pub struct Preview {
    pub name: &'static str,
    pub title: &'static str,
    /// The host's container width in pixels; `None` fills the preview.
    pub width: Option<u32>,
    pub display_modes: &'static [&'static str],
    /// `ui/notifications/tool-input` arguments: `{args, cwd}`.
    pub input: Value,
    /// The `CallToolResult`; `None` keeps the call running.
    pub result: Option<Value>,
    pub result_delay_ms: u64,
    /// `{name, match, results}`: the first rule whose `match` fits the call's
    /// arguments answers it, with its `results` in turn and the last one again.
    pub tools: Vec<Value>,
}

impl Preview {
    /// The `structuredContent` of the result, for the snapshots.
    pub fn structured(&self) -> Value {
        self.result
            .as_ref()
            .map_or(Value::Null, |result| result["structuredContent"].clone())
    }
}

/// Every preview, in the order the index lists them.
pub fn all() -> Vec<Preview> {
    vec![
        lane_starting(),
        lane_ready(),
        lane_failed(),
        lane_frozen(),
        board(),
        board_boxes(),
        board_long_names(),
        narrow(),
        empty(),
        failure_start(),
        failure_why(),
        failure_logs(),
        clean(),
        pair(),
        text(),
    ]
}

/// An agent actor as the CLI reports it.
fn agent(id: &str) -> Actor {
    Actor {
        kind: ActorKind::Agent,
        agent_id: Some(id.to_owned()),
        session_id: Some(format!("{id}-session")),
    }
}

/// A person.
fn you() -> Actor {
    Actor::default()
}

/// A service of lane `index`: `web` gets the lane's domain, `api` the `api.`
/// host, others no URL.
fn service(project: &str, index: u32, name: &str, state: &str, actor: &Actor) -> UnitSummary {
    let slot = ["api", "web", "worker", "storybook", "indexer"]
        .iter()
        .position(|known| *known == name)
        .unwrap_or(5) as u16;
    let port = 20_000 + (index as u16) * 10 + 2 + slot;
    let domain = domain(project, index);
    let public_url = match name {
        "web" => Some(format!("https://{domain}")),
        "api" => Some(format!("https://api.{domain}")),
        _ => None,
    };
    let running = matches!(state, "ready" | "starting" | "stopping");
    UnitSummary {
        name: name.to_owned(),
        lane: format!("{project}@{index}"),
        state: state.to_owned(),
        pid: running.then_some(40_000 + u32::from(port)),
        exit_code: (state == "failed").then_some(1),
        ports: BTreeMap::from([("http".to_owned(), port)]),
        urls: std::iter::once(format!("http://127.0.0.1:{port}"))
            .chain(public_url.clone())
            .collect(),
        actor: actor.clone(),
        reason: (state == "failed").then(|| {
            RpcError::new(
                "ready_timeout",
                json!({"check": "http /health", "timeout_ms": 30_000}),
                1,
            )
        }),
        kind: "service".to_owned(),
        autostart: true,
        started_at_ms: (state != "idle").then_some(NOW - 600_000),
        finished_at_ms: (state == "failed").then_some(NOW - 180_000),
        cpu_percent: running.then_some(4.0),
        memory_bytes: running.then_some(180 << 20),
        ready_check: Some(if name == "worker" { "started" } else { "http" }.to_owned()),
        role: Some("service".to_owned()),
        public_url,
        ..UnitSummary::default()
    }
}

/// The domain of a lane: `acme.localhost` for lane 0, `acme1.localhost` after.
fn domain(project: &str, index: u32) -> String {
    if index == 0 {
        format!("{project}.localhost")
    } else {
        format!("{project}{index}.localhost")
    }
}

/// A setup or task step of a lane.
fn step(project: &str, index: u32, name: &str, role: &str, state: &str) -> UnitSummary {
    UnitSummary {
        name: name.to_owned(),
        lane: format!("{project}@{index}"),
        state: state.to_owned(),
        exit_code: (state == "exited").then_some(0),
        actor: you(),
        kind: "task".to_owned(),
        role: Some(role.to_owned()),
        pty: false,
        ..UnitSummary::default()
    }
}

/// A lane with its units; the domain and port block follow its index.
fn lane(
    project: &str,
    index: u32,
    branch: &str,
    state: &str,
    units: Vec<UnitSummary>,
) -> LaneSummary {
    let path = if index == 0 {
        format!("/work/{project}")
    } else {
        format!("/work/{project}-{}", branch.replace('/', "-"))
    };
    let start = 20_000 + index as u16 * 10;
    LaneSummary {
        reference: format!("{project}@{index}"),
        project: project.to_owned(),
        index: Some(index),
        branch: Some(branch.to_owned()),
        path: path.into(),
        state: state.to_owned(),
        units,
        domain: Some(domain(project, index)),
        port_block: Some((start, start + 9)),
        ..LaneSummary::default()
    }
}

/// The containers of an acme lane, healthy unless `waiting`.
fn containers(waiting: bool) -> Vec<ContainerSummary> {
    ["postgres", "redis"]
        .into_iter()
        .map(|name| ContainerSummary {
            name: name.to_owned(),
            state: Some(
                if waiting && name == "postgres" {
                    "starting"
                } else {
                    "healthy"
                }
                .to_owned(),
            ),
            ..ContainerSummary::default()
        })
        .collect()
}

/// acme@1 on feat/checkout as a status shows it at each stage of a start:
/// 0 infra, 1 and 2 setup, 3 services starting, 4 two ready, 5 running.
fn checkout(stage: u8) -> LaneSummary {
    let actor = agent("claude-code");
    let unit = |name: &str, ready_at: u8| {
        let state = match stage {
            s if s >= ready_at => "ready",
            s if s >= 3 => "starting",
            _ => "idle",
        };
        service("acme", 1, name, state, &actor)
    };
    let setup = |name: &str, done_at: u8| {
        let state = match stage {
            s if s >= done_at => "exited",
            s if s + 1 == done_at => "starting",
            _ => "idle",
        };
        step("acme", 1, name, "setup", state)
    };
    let units = vec![
        unit("api", 4),
        unit("web", 5),
        unit("worker", 4),
        setup("database", 2),
        setup("migrate", 3),
    ];
    let state = if stage >= 5 { "running" } else { "starting" };
    let mut lane = lane("acme", 1, "feat/checkout", state, units);
    lane.containers = containers(stage == 0);
    lane
}

/// A page of lanes as `status --json` prints it.
fn page(items: Vec<LaneSummary>) -> Value {
    serde_json::to_value(Page::complete(items)).expect("fixtures serialize")
}

/// The MCP server's `structuredContent` around a command's JSON output.
fn content(command: &str, args: &[&str], exit_code: i32, view: Option<&str>, data: Value) -> Value {
    json!({
        "command": command,
        "args": args,
        "cwd": CWD,
        "exit_code": exit_code,
        "view": view,
        "data": data,
    })
}

/// A `CallToolResult` with the agent text and the structured content.
fn tool_result(text: &str, structured: Value) -> Value {
    let failed = structured["exit_code"].as_i64().unwrap_or(0) != 0;
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
        "isError": failed,
    })
}

/// The tool-input arguments of a `devlanes` call.
fn input(args: &[&str]) -> Value {
    json!({"args": args, "cwd": CWD})
}

/// A rule answering an app-only tool.
fn rule(name: &str, matches: Value, results: Vec<Value>) -> Value {
    json!({"name": name, "match": matches, "results": results})
}

/// An app-only tool's result.
fn app(text: &str, structured: Value) -> Value {
    json!({"content": [{"type": "text", "text": text}], "structuredContent": structured})
}

/// `debug top --json` with a CPU history for every ready unit and live lane.
fn top(lanes: &[&LaneSummary]) -> Value {
    let history = |seed: u64| -> Vec<Value> {
        (0..30u64)
            .map(|i| {
                let wave = ((i as f64) / 4.0 + seed as f64).sin() + ((i as f64) / 1.7).sin() / 3.0;
                let cpu = (6.0 + (seed % 4) as f64 * 3.0 + wave * 4.0).max(0.0);
                json!([
                    NOW - (30 - i) * 2_000,
                    (cpu * 10.0).round() / 10.0,
                    180u64 << 20
                ])
            })
            .collect()
    };
    let mut units = Vec::new();
    let mut rows = Vec::new();
    for (n, lane) in lanes.iter().enumerate() {
        for (m, unit) in lane.units.iter().enumerate() {
            if unit.state == "ready" {
                units.push(json!({
                    "lane": lane.reference, "unit": unit.name, "kind": "service",
                    "ended": false, "removed": false,
                    "cpu_percent": 4.0, "memory_bytes": 180u64 << 20,
                    "history": history((n * 10 + m) as u64),
                }));
            }
        }
        if matches!(lane.state.as_str(), "running" | "starting" | "failed") {
            rows.push(json!({
                "lane": lane.reference, "ended": false, "removed": false,
                "cpu_percent": 9.0, "memory_bytes": 540u64 << 20,
                "history": history(n as u64 + 3),
            }));
        }
    }
    json!({"units": units, "lanes": rows, "range_ms": 60_000})
}

/// An `app_snapshot` answer for these lanes.
fn snapshot(lanes: &[&LaneSummary]) -> Value {
    snapshot_with(lanes, &json!({}))
}

/// An `app_snapshot` answer with extra top-level status fields (`hosts`).
fn snapshot_with(lanes: &[&LaneSummary], extra: &Value) -> Value {
    let mut status = page(lanes.iter().map(|lane| (*lane).clone()).collect());
    merge(&mut status, extra);
    app(
        &format!("{} lanes", lanes.len()),
        json!({"status": status, "top": top(lanes)}),
    )
}

/// Adds the fields of `extra` to the object `into`.
fn merge(into: &mut Value, extra: &Value) {
    if let (Some(into), Some(extra)) = (into.as_object_mut(), extra.as_object()) {
        into.extend(extra.clone());
    }
}

/// `app_action` that worked.
fn action_ok(text: &str) -> Value {
    app(text, json!({"exit_code": 0, "text": text, "data": null}))
}

/// The rule every lane-like preview shares: actions succeed.
fn actions() -> Vec<Value> {
    vec![rule("app_action", json!({}), vec![action_ok("done")])]
}

/// Log lines of acme@2:api from sequence `from`, oldest first.
fn api_logs(from: u64, count: u64) -> Vec<LogLine> {
    const TEXTS: [(&str, &str); 17] = [
        ("info", "> acme-api@0.4.0 dev"),
        ("info", "> tsx watch src/server.ts"),
        ("info", "[info] node v22.11.0, pnpm 9.12"),
        ("info", "[info] PORT=20023 HOST=127.0.0.1"),
        ("info", "[info] DEVLANES_LANE=2"),
        ("info", "[info] reading .env.local"),
        ("info", "[info] loading config"),
        (
            "info",
            "[info] connecting to postgres 127.0.0.1:20021/acme_2",
        ),
        ("warn", "[warn] AUTH_REDIRECT_URL not set, using default"),
        ("info", "[info] postgres ready (12 ms)"),
        ("info", "[info] registering routes (42)"),
        ("info", "[info] auth: building redirect table"),
        (
            "error",
            "TypeError: Cannot read properties of undefined (reading 'callbackUrl')",
        ),
        ("error", "    at buildRedirect (src/auth/redirect.ts:31:22)"),
        ("error", "    at registerAuth (src/routes/auth.ts:12:3)"),
        ("error", "    at main (src/server.ts:48:9)"),
        ("info", "[devlanes] process exited with code 1"),
    ];
    (from..from + count)
        .map(|seq| {
            let (level, text) = TEXTS[(seq - from) as usize % TEXTS.len()];
            LogLine {
                unit: "api".to_owned(),
                seq,
                timestamp_ms: NOW - 240_000 + seq * 50,
                text: text.to_owned(),
                level: Some(level.to_owned()),
                styled: None,
            }
        })
        .collect()
}

/// An `app_logs` answer.
fn logs_page(lines: Vec<LogLine>, next: Option<&str>) -> Value {
    let page = Page {
        items: lines,
        truncated: next.is_some(),
        next_cursor: next.map(str::to_owned),
    };
    json!({"logs": page})
}

/// The `app_logs` rules of acme@2:api: a grep, an earlier page, no new lines
/// while following, and the tail.
fn api_log_rules() -> Vec<Value> {
    let auth: Vec<LogLine> = api_logs(103, 17)
        .into_iter()
        .filter(|line| line.text.contains("auth"))
        .collect();
    vec![
        rule(
            "app_logs",
            json!({"grep": "*"}),
            vec![app("grep", logs_page(auth, None))],
        ),
        rule(
            "app_logs",
            json!({"cursor": "before:*"}),
            vec![app("earlier", logs_page(api_logs(86, 17), None))],
        ),
        rule(
            "app_logs",
            json!({"cursor": "*"}),
            vec![app("no new lines", logs_page(Vec::new(), None))],
        ),
        rule(
            "app_logs",
            json!({}),
            vec![app(
                "tail",
                logs_page(api_logs(103, 17), Some("before:api#103")),
            )],
        ),
    ]
}

/// A start result as JSON.
fn started(result: StartResult) -> Value {
    serde_json::to_value(result).expect("fixtures serialize")
}

fn lane_starting() -> Preview {
    let stages: Vec<Value> = (0..5).map(|stage| snapshot(&[&checkout(stage)])).collect();
    let mut tools = vec![rule("app_snapshot", json!({}), stages)];
    tools.extend(actions());
    Preview {
        name: "lane-starting",
        title: "lane card while it starts",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&["start"]),
        result: None,
        result_delay_ms: 0,
        tools,
    }
}

fn lane_ready() -> Preview {
    let lane = checkout(5);
    let data = StartResult {
        lane: lane.reference.clone(),
        branch: lane.branch.clone(),
        started: lane
            .units
            .iter()
            .map(|u| format!("acme@1:{}", u.name))
            .collect(),
        whole_lane: true,
        ready: true,
        units: lane.units.clone(),
        start_id: Some(7),
        ..StartResult::default()
    };
    let mut tools = vec![rule("app_snapshot", json!({}), vec![snapshot(&[&lane])])];
    tools.extend(actions());
    tools.extend(api_log_rules());
    Preview {
        name: "lane-ready",
        title: "lane card, ready",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&["start"]),
        result: Some(tool_result(
            "acme@1 (feat/checkout): 3/3 ready\nready acme@1:api    https://api.acme1.localhost\nready acme@1:web    https://acme1.localhost\nready acme@1:worker",
            content(
                "start",
                &["start", "--timeout", "170s"],
                0,
                Some("lane"),
                started(data),
            ),
        )),
        result_delay_ms: 900,
        tools,
    }
}

/// acme@2 on fix/auth-redirect with a failed api, started by codex.
fn auth_redirect() -> LaneSummary {
    let actor = agent("codex");
    let mut lane = lane(
        "acme",
        2,
        "fix/auth-redirect",
        "failed",
        vec![
            service("acme", 2, "api", "failed", &actor),
            service("acme", 2, "web", "ready", &actor),
            service("acme", 2, "worker", "ready", &actor),
            step("acme", 2, "database", "setup", "exited"),
        ],
    );
    lane.containers = containers(false);
    lane
}

fn lane_failed() -> Preview {
    let lane = auth_redirect();
    let mut tools = vec![rule("app_snapshot", json!({}), vec![snapshot(&[&lane])])];
    tools.extend(actions());
    tools.extend(api_log_rules());
    let args = ["status", "--lane", "fix/auth-redirect"];
    Preview {
        name: "lane-failed",
        title: "lane card with a failed service",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&args),
        result: Some(tool_result(
            "failed acme@2 fix/auth-redirect 2/3 failed=api\nfailed acme@2:api exit=1 ready check http /health did not pass in 30s",
            content("status", &args, 0, Some("lane"), page(vec![lane])),
        )),
        result_delay_ms: 300,
        tools,
    }
}

/// acme@3 on feat/search, frozen 22 minutes ago after opencode left it.
fn search() -> LaneSummary {
    let actor = agent("opencode");
    let mut units: Vec<UnitSummary> = ["api", "web", "indexer"]
        .iter()
        .map(|name| service("acme", 3, name, "ready", &actor))
        .collect();
    for unit in &mut units {
        unit.power = Some("frozen".to_owned());
    }
    let mut lane = lane("acme", 3, "feat/search", "running", units);
    lane.power = Some(LanePower {
        level: "frozen".to_owned(),
        since_ms: NOW - 22 * 60_000,
        last_activity_ms: Some(NOW - 37 * 60_000),
        last_activity: Some("proxy".to_owned()),
    });
    lane.containers = containers(false);
    lane
}

fn lane_frozen() -> Preview {
    let lane = search();
    let mut tools = vec![rule("app_snapshot", json!({}), vec![snapshot(&[&lane])])];
    tools.extend(actions());
    let args = ["status", "--lane", "feat/search"];
    Preview {
        name: "lane-frozen",
        title: "lane card, frozen",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&args),
        result: Some(tool_result(
            "running acme@3 feat/search 3/3 power=frozen",
            content("status", &args, 0, Some("lane"), page(vec![lane])),
        )),
        result_delay_ms: 300,
        tools,
    }
}

/// acme@0 on main, started by a person.
fn main_lane() -> LaneSummary {
    let actor = you();
    let units = ["api", "web", "worker", "storybook"]
        .iter()
        .map(|name| service("acme", 0, name, "ready", &actor))
        .collect();
    let mut lane = lane("acme", 0, "main", "running", units);
    lane.containers = containers(false);
    lane
}

/// acme@4 on chore/deps, idle.
fn deps() -> LaneSummary {
    let actor = you();
    let units = ["api", "web", "worker"]
        .iter()
        .map(|name| service("acme", 4, name, "idle", &actor))
        .collect();
    lane("acme", 4, "chore/deps", "idle", units)
}

/// Every lane of the board.
fn board_lanes() -> Vec<LaneSummary> {
    vec![main_lane(), checkout(3), auth_redirect(), search(), deps()]
}

/// A board preview of `lanes`, with `extra` status fields.
fn board_of(
    name: &'static str,
    title: &'static str,
    lanes: Vec<LaneSummary>,
    extra: &Value,
    width: Option<u32>,
) -> Preview {
    let refs: Vec<&LaneSummary> = lanes.iter().collect();
    let mut tools = vec![rule(
        "app_snapshot",
        json!({}),
        vec![snapshot_with(&refs, extra)],
    )];
    tools.extend(actions());
    tools.extend(api_log_rules());
    let text = lanes
        .iter()
        .map(|lane| {
            format!(
                "{} {} {}",
                lane.state,
                lane.reference,
                lane.branch.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut data = page(lanes);
    merge(&mut data, extra);
    Preview {
        name,
        title,
        width,
        display_modes: &["inline", "fullscreen"],
        input: input(&["status"]),
        result: Some(tool_result(
            &text,
            content("status", &["status"], 0, Some("board"), data),
        )),
        result_delay_ms: 300,
        tools,
    }
}

fn board() -> Preview {
    board_of(
        "board",
        "lanes board with several lanes and actors",
        board_lanes(),
        &json!({}),
        None,
    )
}

fn board_boxes() -> Preview {
    let hosts = json!({"hosts": [
        {"host": "hetzner", "state": "connected", "server": "devlanes 0.11.0"},
        {"host": "fork-b", "state": "needs_pairing", "fix": "devlanes remote connect fork-b --code CODE"},
        {"host": "ci-box", "state": "reconnecting", "retry_in_ms": 4000, "error": "connection refused"},
    ]});
    board_of(
        "board-boxes",
        "lanes board with read-only boxes",
        vec![main_lane(), auth_redirect()],
        &hosts,
        None,
    )
}

fn board_long_names() -> Preview {
    let actor = agent("a-very-long-agent-name-from-a-new-host");
    let project = "acme-platform-monorepo";
    let branch = "feature/JIRA-12345-rewrite-the-checkout-flow-to-support-multiple-currencies";
    let mut long = lane(
        project,
        12,
        branch,
        "running",
        vec![service(project, 12, "web", "ready", &actor)],
    );
    long.units[0].display_name = Some("storefront-web-server-with-a-long-name".to_owned());
    let mut named = lane(project, 13, "main", "idle", Vec::new());
    named.name = Some("experiment-with-a-second-lane-on-the-same-folder".to_owned());
    board_of(
        "long-names",
        "long names everywhere",
        vec![long, named, auth_redirect()],
        &json!({}),
        None,
    )
}

fn narrow() -> Preview {
    board_of(
        "narrow",
        "lanes board at 340 px",
        board_lanes(),
        &json!({}),
        Some(340),
    )
}

fn empty() -> Preview {
    board_of(
        "empty",
        "status without lanes",
        Vec::new(),
        &json!({}),
        None,
    )
}

fn failure_start() -> Preview {
    let lane = auth_redirect();
    let failure = |unit: &str, tail: &[&str]| UnitFailureData {
        lane: "acme@2".to_owned(),
        unit: unit.to_owned(),
        exit_code: Some(1),
        log_tail: tail.iter().map(|line| (*line).to_owned()).collect(),
        reason: Some(RpcError::new(
            "exited",
            json!({"code": 1, "signal": null}),
            1,
        )),
        log_tail_styled: Vec::new(),
    };
    let data = StartResult {
        lane: "acme@2".to_owned(),
        branch: lane.branch.clone(),
        started: vec!["acme@2:api".to_owned(), "acme@2:worker".to_owned()],
        ready: false,
        failures: vec![
            failure(
                "api",
                &[
                    "[info] auth: building redirect table",
                    "TypeError: Cannot read properties of undefined (reading 'callbackUrl')",
                    "    at buildRedirect (src/auth/redirect.ts:31:22)",
                ],
            ),
            failure("worker", &["[error] queue acme_2 does not exist"]),
        ],
        units: lane.units.clone(),
        ..StartResult::default()
    };
    let mut tools = vec![rule("app_snapshot", json!({}), vec![snapshot(&[&lane])])];
    tools.extend(actions());
    tools.extend(api_log_rules());
    Preview {
        name: "failure-start",
        title: "failure card after a failed start",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&["start"]),
        result: Some(tool_result(
            "acme@2 (fix/auth-redirect): 1/3 ready, 2 failed\nfailed acme@2:api exit=1\napi#118 TypeError: Cannot read properties of undefined (reading 'callbackUrl')",
            content(
                "start",
                &["start", "--timeout", "170s"],
                1,
                Some("failure"),
                started(data),
            ),
        )),
        result_delay_ms: 600,
        tools,
    }
}

fn failure_why() -> Preview {
    let lane = auth_redirect();
    let data = json!({
        "lane": "acme@2",
        "unit": "api",
        "state": "failed",
        "reasons": [
            {"code": "ready_timeout", "data": {"check": "http /health", "timeout_ms": 30_000}},
        ],
    });
    let mut tools = vec![rule("app_snapshot", json!({}), vec![snapshot(&[&lane])])];
    tools.extend(actions());
    tools.extend(api_log_rules());
    let args = ["debug", "why", "2:api"];
    Preview {
        name: "failure-why",
        title: "failure card for debug why",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&args),
        result: Some(tool_result(
            "failed acme@2:api exit=1 ready check http /health did not pass in 30s\nmore: devlanes logs acme@2:api --tail 200",
            content("debug why", &args, 0, Some("failure"), data),
        )),
        result_delay_ms: 300,
        tools,
    }
}

fn failure_logs() -> Preview {
    let lane = auth_redirect();
    let data = serde_json::to_value(Page {
        items: api_logs(103, 17),
        truncated: true,
        next_cursor: Some("before:api#103".to_owned()),
    })
    .expect("fixtures serialize");
    let mut tools = vec![rule("app_snapshot", json!({}), vec![snapshot(&[&lane])])];
    tools.extend(actions());
    tools.extend(api_log_rules());
    let args = ["logs", "acme@2:api", "--tail", "17"];
    Preview {
        name: "failure-logs",
        title: "log viewer for logs, inline only",
        width: None,
        display_modes: &["inline"],
        input: input(&args),
        result: Some(tool_result(
            "api#103 > acme-api@0.4.0 dev\n...\napi#119 [devlanes] process exited with code 1",
            content("logs", &args, 0, Some("failure"), data),
        )),
        result_delay_ms: 300,
        tools,
    }
}

/// The `lane clean --dry-run --json` output of a lane, as the contracts define it.
fn dry_run(lane: &LaneSummary, worktree: &str, volume: (&str, u64), data: u64) -> Value {
    let index = lane.index.unwrap_or(0);
    let running: Vec<Value> = lane
        .units
        .iter()
        .filter(|unit| matches!(unit.state.as_str(), "ready" | "starting"))
        .map(|unit| json!({"unit": format!("{}:{}", lane.reference, unit.name), "actor": unit.actor}))
        .collect();
    json!({
        "lane": lane.reference,
        "project": lane.project,
        "index": index,
        "branch": lane.branch,
        "path": lane.path,
        "worktree": worktree,
        "running": running,
        "tasks": [{"name": "drop-cache", "run": "redis-cli -n 4 flushdb"}],
        "teardown": [{"name": "database", "container": "postgres"}],
        "containers": [{"name": format!("devlanes-acme-lane{index}-redis"), "kind": "container"}],
        "compose_project": format!("acme-lane{index}"),
        "volumes": [{"name": volume.0, "bytes": volume.1}],
        "data_dir": {"path": format!("/home/dev/.devlanes/data/acme/{index}"), "bytes": data},
        "logs_bytes": 2_400_000,
        "ports": {"start": 20_000 + index * 10, "size": 10},
    })
}

fn clean() -> Preview {
    let old = lane("acme", 5, "feat/old-onboarding", "idle", Vec::new());
    let lanes = [main_lane(), auth_redirect(), deps(), old.clone()];
    let refs: Vec<&LaneSummary> = lanes.iter().collect();
    let plan = |lane: &LaneSummary, worktree: &str, volume: &str, bytes: u64| {
        app(
            "plan",
            json!({"exit_code": 0, "text": "", "data": dry_run(lane, worktree, (volume, bytes), bytes / 6)}),
        )
    };
    let refused = app(
        "refused",
        json!({"exit_code": 2, "text": "error: acme@0 is the lane of the main worktree", "data": null}),
    );
    let tools = vec![
        rule("app_snapshot", json!({}), vec![snapshot(&refs)]),
        rule(
            "app_action",
            json!({"action": "clean_plan", "target": "acme@0"}),
            vec![refused],
        ),
        rule(
            "app_action",
            json!({"action": "clean_plan", "target": "acme@2"}),
            vec![plan(
                &auth_redirect(),
                "present",
                "acme-lane2_pgdata",
                966_367_641,
            )],
        ),
        rule(
            "app_action",
            json!({"action": "clean_plan", "target": "acme@5"}),
            vec![plan(&old, "missing", "acme-lane5_pgdata", 2_576_980_377)],
        ),
        rule(
            "app_action",
            json!({"action": "clean"}),
            vec![action_ok("cleaned")],
        ),
    ];
    let args = ["lane", "clean", "acme@4", "--dry-run"];
    Preview {
        name: "clean",
        title: "clean planner",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&args),
        result: Some(tool_result(
            "acme@4 (chore/deps): a clean deletes volume acme-lane4_pgdata 1.2 GiB and the data folder 310 MiB\nask the user, then run devlanes lane clean acme@4 --yes",
            content(
                "lane clean",
                &args,
                0,
                Some("clean"),
                dry_run(
                    &deps(),
                    "present",
                    ("acme-lane4_pgdata", 1_288_490_188),
                    325_058_560,
                ),
            ),
        )),
        result_delay_ms: 300,
        tools,
    }
}

fn pair() -> Preview {
    let data = json!({"code": "K7QM-3XWD", "expires_at_ms": NOW + 600_000, "valid_ms": 600_000});
    let renewed = json!({"code": "R2PL-8HNC", "expires_at_ms": NOW + 600_000, "valid_ms": 600_000});
    Preview {
        name: "pair",
        title: "pairing code",
        width: Some(420),
        display_modes: &["inline"],
        input: input(&["daemon", "pair"]),
        result: Some(tool_result(
            "K7QM-3XWD (valid 10 minutes)",
            content(
                "daemon pair",
                &["daemon", "pair", "--yes"],
                0,
                Some("pair"),
                data,
            ),
        )),
        result_delay_ms: 300,
        tools: vec![rule(
            "app_action",
            json!({"action": "pair"}),
            vec![app(
                "R2PL-8HNC",
                json!({"exit_code": 0, "text": "R2PL-8HNC (valid 10 minutes)", "data": renewed}),
            )],
        )],
    }
}

fn text() -> Preview {
    let args = ["exec", "--name", "storybook", "--", "pnpm", "storybook"];
    Preview {
        name: "text",
        title: "a command without a card of its own",
        width: None,
        display_modes: &["inline", "fullscreen"],
        input: input(&args),
        result: Some(tool_result(
            "started acme@0:storybook (exec) pid 41234\nread its output with devlanes logs storybook; stop it with devlanes stop storybook",
            content("exec", &args, 0, None, Value::Null),
        )),
        result_delay_ms: 300,
        tools: Vec::new(),
    }
}
