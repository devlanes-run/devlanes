//! Tool results and app-only tool answers for the previews, built from
//! `devlanes_proto` types (and `devlanes_transport`'s for a box's registry) so
//! a change to the daemon's data shapes breaks this file. Shapes neither has
//! (the clean dry-run, `debug top`, the rest of a `remote ls` item, the
//! structured content the MCP server wraps around them) are JSON literals that
//! follow the daemon and the contracts between the server and the card.

use std::collections::BTreeMap;

use devlanes_proto::{
    Actor, ActorKind, ContainerSummary, LanePower, LaneSummary, LogLine, Page, RpcError,
    StartResult, UnitFailureData, UnitSummary,
};
use devlanes_transport::{
    HostHeader, HostInfo, LaneInfo, LaneKey, PortInfo, RegistrySnapshot, UnitInfo, UnitKey,
    UnitKind,
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
    /// The display mode the host opens the card in.
    pub display_mode: &'static str,
    /// Clicks the fake host makes in the card: `{click, afterMs}`, `click` the
    /// start of a button's label.
    pub steps: Vec<Value>,
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

    /// The `remote ls` output in the preview's first `app_snapshot` answer,
    /// `None` when it has none.
    pub fn remote_ls(&self) -> Option<Value> {
        self.tools
            .iter()
            .find(|rule| rule["name"] == "app_snapshot")
            .map(|rule| rule["results"][0]["structuredContent"]["hosts"].clone())
            .filter(|hosts| !hosts.is_null())
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
        read_only(
            lane_failed(),
            "lane-read-only",
            "lane card of a read-only server",
        ),
        read_only(
            board(),
            "board-read-only",
            "lanes board of a read-only server",
        ),
        read_only(
            clean(),
            "clean-read-only",
            "clean plan of a read-only server",
        ),
        daemon_stopped(),
        superseded(),
        poll_stopped(),
        clean_confirm(),
        logs_fullscreen(),
        board_fullscreen(),
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

/// The MCP server's `structuredContent` around a command's JSON output. Data that
/// names one lane (a start or clean result, a status of one lane) makes the
/// server's first card for that lane in these fixtures, so `card` is 1.
fn content(command: &str, args: &[&str], exit_code: i32, view: Option<&str>, data: Value) -> Value {
    let one_lane = data["lane"].is_string()
        || data["items"]
            .as_array()
            .is_some_and(|items| items.len() == 1 && items[0]["reference"].is_string());
    json!({
        "command": command,
        "args": args,
        "cwd": CWD,
        "exit_code": exit_code,
        "view": view,
        "data": data,
        "read_only": false,
        "card": one_lane.then_some(1),
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

/// An app-only tool's result, from a server without `--read-only`.
fn app(text: &str, mut structured: Value) -> Value {
    structured["read_only"] = json!(false);
    json!({"content": [{"type": "text", "text": text}], "structuredContent": structured})
}

/// `preview` as a server started with `--read-only` answers it: every
/// structured content says so, and `app_action` is not there.
fn read_only(mut preview: Preview, name: &'static str, title: &'static str) -> Preview {
    preview.name = name;
    preview.title = title;
    if let Some(result) = &mut preview.result {
        result["structuredContent"]["read_only"] = json!(true);
    }
    preview.tools.retain(|rule| rule["name"] != "app_action");
    for rule in &mut preview.tools {
        for result in rule["results"].as_array_mut().into_iter().flatten() {
            result["structuredContent"]["read_only"] = json!(true);
        }
    }
    preview
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
    snapshot_with(lanes, &json!({}), Value::Null)
}

/// An `app_snapshot` answer with extra top-level status fields (`hosts`) and
/// the `remote ls` output the server adds while a box is linked.
fn snapshot_with(lanes: &[&LaneSummary], extra: &Value, boxes: Value) -> Value {
    let mut status = page(lanes.iter().map(|lane| (*lane).clone()).collect());
    merge(&mut status, extra);
    // Each fixture's card is the only one for its lane.
    let newest: serde_json::Map<String, Value> = lanes
        .iter()
        .map(|lane| (lane.reference.clone(), json!(1)))
        .collect();
    app(
        &format!("{} lanes", lanes.len()),
        json!({"status": status, "top": top(lanes), "hosts": boxes, "newest_card": newest}),
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
        display_mode: "inline",
        steps: Vec::new(),
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
        display_mode: "inline",
        steps: Vec::new(),
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
        display_mode: "inline",
        steps: Vec::new(),
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
        display_mode: "inline",
        steps: Vec::new(),
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

/// A board preview of `lanes`, with `extra` status fields and the `remote ls`
/// output `boxes` in its snapshots.
fn board_of(
    name: &'static str,
    title: &'static str,
    lanes: Vec<LaneSummary>,
    extra: &Value,
    boxes: Value,
    width: Option<u32>,
) -> Preview {
    let refs: Vec<&LaneSummary> = lanes.iter().collect();
    let mut tools = vec![rule(
        "app_snapshot",
        json!({}),
        vec![snapshot_with(&refs, extra, boxes)],
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
        display_mode: "inline",
        steps: Vec::new(),
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
        Value::Null,
        None,
    )
}

/// A lane of a box's registry: `units` are `(kind, name, state)`, each on the
/// next port of the lane's block, services with the hostname `<name>.<domain>`.
fn box_lane(project: &str, index: u32, units: &[(UnitKind, &str, &str)]) -> RegistrySnapshot {
    let key = LaneKey {
        project: project.to_owned(),
        lane: index.to_string(),
    };
    let domain = domain(project, index);
    let units = units
        .iter()
        .zip(30_000 + index as u16 * 10..)
        .map(|((kind, name, state), port)| UnitInfo {
            key: UnitKey {
                lane: key.clone(),
                kind: *kind,
                name: (*name).to_owned(),
            },
            hidden: false,
            state: (*state).to_owned(),
            ports: (*kind != UnitKind::Task)
                .then(|| PortInfo {
                    name: "default".to_owned(),
                    port,
                    address: None,
                })
                .into_iter()
                .collect(),
            hosts: (*kind == UnitKind::Service)
                .then(|| HostInfo {
                    host: format!("{name}.{domain}"),
                    port,
                    fallback: false,
                    host_header: HostHeader::Keep,
                })
                .into_iter()
                .collect(),
        })
        .collect();
    RegistrySnapshot {
        lanes: vec![LaneInfo {
            key,
            index,
            domain: Some(domain),
        }],
        units,
    }
}

/// A `remote ls` item for box `host` whose link is in `state`: its registry,
/// the lanes it leaves unmirrored (`skipped_lanes` entries) and what the
/// mirror binds and routes for the rest, which is every serving port and
/// hostname.
fn box_host(host: &str, state: &str, lanes: Vec<RegistrySnapshot>, skipped: Vec<Value>) -> Value {
    let registry = RegistrySnapshot {
        lanes: lanes.iter().flat_map(|l| l.lanes.clone()).collect(),
        units: lanes.into_iter().flat_map(|l| l.units).collect(),
    };
    let skips = |lane: &str| skipped.iter().any(|s| s["lane"] == lane);
    let route_state = match state {
        "connected" => None,
        "needs_pairing" => Some("pairing_needed"),
        _ => Some("reconnecting"),
    };
    let mut ports = Vec::new();
    let mut routes = Vec::new();
    for unit in &registry.units {
        let lane = format!("{}@{}", unit.key.lane.project, unit.key.lane.lane);
        if skips(&lane) || !matches!(unit.state.as_str(), "ready" | "running" | "starting") {
            continue;
        }
        for port in &unit.ports {
            ports.push(json!({
                "port": port.port, "lane": lane, "unit": unit.key.name, "state": "bound",
                "addresses": ["127.0.0.1", "::1"], "conflicts": [], "relay": null, "last_error": null,
            }));
        }
        for name in &unit.hosts {
            routes.push(json!({
                "host": name.host, "port": name.port, "lane": lane, "unit": unit.key.name,
                "url": format!("https://{}", name.host),
                "state": route_state.unwrap_or(if unit.state == "ready" { "ready" } else { "starting" }),
            }));
        }
    }
    json!({
        "host": host,
        "state": state,
        "transport": "ssh",
        "connected_since_ms": (state == "connected").then_some(NOW - 2 * 3_600_000),
        "ports": ports,
        "routes": routes,
        "skipped_lanes": skipped,
        "skipped_hosts": [],
        "syncs": [],
        "warnings": [],
        "registry": registry,
        "box": {"version": "0.11.0", "label": null, "https_port": 443, "ports_range": "30000-30999"},
        "suffixes": ["localhost"],
        "proxy": {"https_port": 443, "box_port": null, "open": false},
        "chosen_lanes": [],
    })
}

/// A `skipped_lanes` entry for box lane `project@index`, whose `web` hostname
/// is served already, with the daemon's `reason` for its `cause`.
fn skipped(project: &str, index: u32, cause: &str, winner: Option<&str>) -> Value {
    let lane = format!("{project}@{index}");
    let reason = match (cause, winner) {
        ("local_running", _) => format!("{lane} runs on this machine"),
        (_, Some(winner)) => format!("{winner}'s {lane} is in use"),
        (_, None) => format!("this machine's {lane} is in use"),
    };
    let mut entry = json!({
        "lane": lane, "hosts": [format!("web.{}", domain(project, index))], "box": winner,
        "reason": reason, "cause": cause, "winner": winner, "winner_lane": lane,
        "can_use": cause != "local_running",
    });
    // The daemon says what keeps a running local lane up, and only then.
    if cause == "local_running" {
        entry["holds"] = json!("processes");
    }
    entry
}

/// The board of a status the agent ran before the daemon stopped: every
/// `app_snapshot` since says no daemon runs.
fn daemon_stopped() -> Preview {
    let mut preview = board();
    preview.name = "daemon-stopped";
    preview.title = "lanes board after the daemon stopped";
    let stopped = app(
        "the daemon is not running",
        json!({"status": null, "top": null, "hosts": null, "daemon": "stopped"}),
    );
    for rule in &mut preview.tools {
        if rule["name"] == "app_snapshot" {
            rule["results"] = json!([stopped.clone()]);
        }
    }
    preview
}

fn board_boxes() -> Preview {
    use UnitKind::{Container, Service, Task};
    // `status` lists each box by its link state only.
    let hosts = json!({"hosts": [
        {"host": "hetzner", "state": "connected", "server": "devlanes 0.11.0"},
        {"host": "fork-b", "state": "needs_pairing", "error": "the box asks for a new pairing", "pairing_retry_at_ms": null,
         "fix": "devlanes daemon pair on fork-b, then devlanes remote connect fork-b --code <code>"},
        {"host": "ci-box", "state": "reconnecting", "retry_in_ms": 4000, "error": "connection refused"},
    ]});
    let mut hetzner = box_host(
        "hetzner",
        "connected",
        vec![
            // This machine runs acme@0 too, so the box's stays unmirrored.
            box_lane("acme", 0, &[(Service, "web", "ready")]),
            box_lane(
                "acme",
                1,
                &[
                    (Service, "api", "ready"),
                    (Service, "web", "ready"),
                    (Task, "migrate", "exited"),
                    (Container, "postgres", "running"),
                ],
            ),
            box_lane(
                "acme",
                3,
                &[(Service, "api", "ready"), (Service, "web", "starting")],
            ),
            box_lane("shop", 0, &[(Service, "web", "ready")]),
        ],
        vec![skipped("acme", 0, "local_running", None)],
    );
    hetzner["chosen_lanes"] = json!(["shop@0"]);
    let fork_b = box_host(
        "fork-b",
        "needs_pairing",
        vec![
            box_lane("acme", 6, &[(Service, "web", "ready")]),
            // The same hostnames as hetzner's shop@0, chosen there.
            box_lane("shop", 0, &[(Service, "web", "ready")]),
        ],
        vec![skipped("shop", 0, "box_chosen", Some("hetzner"))],
    );
    let ci = json!({"host": "ci-box", "state": "reconnecting", "transport": "ssh", "retry_in_ms": 4000,
        "error": "connection refused", "last_connected_ms": NOW - 86_400_000});
    let boxes = json!({
        "items": [ci, fork_b, hetzner],
        "yielded_local_lanes": [],
        "truncated": false,
        "next_cursor": null,
    });
    board_of(
        "board-boxes",
        "lanes board with read-only box lanes",
        vec![main_lane(), auth_redirect()],
        &hosts,
        boxes,
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
        Value::Null,
        None,
    )
}

fn narrow() -> Preview {
    board_of(
        "narrow",
        "lanes board at 340 px",
        board_lanes(),
        &json!({}),
        Value::Null,
        Some(340),
    )
}

fn empty() -> Preview {
    board_of(
        "empty",
        "status without lanes",
        Vec::new(),
        &json!({}),
        Value::Null,
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
        display_mode: "inline",
        steps: Vec::new(),
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
        display_mode: "inline",
        steps: Vec::new(),
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
        title: "failure card for logs",
        width: None,
        display_modes: &["inline", "fullscreen"],
        display_mode: "inline",
        steps: Vec::new(),
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
    // A clean runs no tasks or teardown for a lane whose worktree is gone.
    let hooks = worktree == "present";
    json!({
        "lane": lane.reference,
        "project": lane.project,
        "index": index,
        "branch": lane.branch,
        "path": lane.path,
        "worktree": worktree,
        "running": running,
        "tasks": if hooks { json!([{"name": "drop-cache", "run": "redis-cli -n ${{ lane.index }} flushdb"}]) } else { json!([]) },
        "teardown": if hooks { json!([{"name": "database", "run": "dropdb acme_${{ lane.index }}", "container": "postgres"}]) } else { json!([]) },
        "hooks_skipped": if hooks { Value::Null } else { json!("worktree_missing") },
        "containers": [{"name": format!("devlanes-acme-lane{index}-redis"), "kind": "container"}],
        "compose_project": format!("acme-lane{index}"),
        "volumes": [{"name": volume.0, "bytes": volume.1}],
        "infra_error": null,
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
        json!({"exit_code": 2, "text": "error: acme@0 is the main worktree's lane", "data": null}),
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
        display_mode: "inline",
        steps: Vec::new(),
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
        width: Some(480),
        display_modes: &["inline", "fullscreen"],
        display_mode: "inline",
        steps: Vec::new(),
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

/// The ready lane card after the agent made a newer card for acme@1: the first
/// snapshot says so, and the card shrinks to one line and stops polling.
fn superseded() -> Preview {
    let mut preview = lane_ready();
    preview.name = "lane-superseded";
    preview.title = "older lane card once a newer one is below";
    preview.result_delay_ms = 0;
    for rule in &mut preview.tools {
        if rule["name"] == "app_snapshot" {
            rule["results"][0]["structuredContent"]["newest_card"]["acme@1"] = json!(2);
        }
    }
    preview
}

/// The ready lane card whose snapshots fail after the first one, until the card
/// gives up and shows when it was last fresh (about 15 seconds in).
fn poll_stopped() -> Preview {
    let mut preview = lane_ready();
    preview.name = "poll-stopped";
    preview.title = "lane card after polling stopped";
    let failed = json!({
        "content": [{"type": "text", "text": "devlanes did not answer"}],
        "isError": true,
    });
    for rule in &mut preview.tools {
        if rule["name"] == "app_snapshot" {
            let first = rule["results"][0].clone();
            rule["results"] = json!([first, failed]);
        }
    }
    preview
}

/// The clean planner after the person pressed "Clean @4": the action bar asks
/// to confirm.
fn clean_confirm() -> Preview {
    let mut preview = clean();
    preview.name = "clean-confirm";
    preview.title = "clean planner, confirm step";
    preview.steps = vec![json!({"click": "Clean @4", "afterMs": 2500})];
    preview
}

/// The failure card of `logs` opened in fullscreen: the whole log with a filter.
fn logs_fullscreen() -> Preview {
    let mut preview = failure_logs();
    preview.name = "logs-fullscreen";
    preview.title = "fullscreen log viewer";
    preview.display_mode = "fullscreen";
    preview
}

/// The lanes board opened in fullscreen: CPU and a button each per lane.
fn board_fullscreen() -> Preview {
    let mut preview = board();
    preview.name = "board-fullscreen";
    preview.title = "fullscreen lanes board";
    preview.display_mode = "fullscreen";
    preview
}

fn text() -> Preview {
    let args = ["exec", "--name", "storybook", "--", "pnpm", "storybook"];
    Preview {
        name: "text",
        title: "a command without a card, which stays empty",
        width: None,
        display_modes: &["inline", "fullscreen"],
        display_mode: "inline",
        steps: Vec::new(),
        input: input(&args),
        result: Some(tool_result(
            "ready acme@0:storybook pid=41234\n  logs: devlanes logs acme@0:storybook --tail 100\n  stop: devlanes stop acme@0:storybook",
            content("exec", &args, 0, None, Value::Null),
        )),
        result_delay_ms: 300,
        tools: Vec::new(),
    }
}
