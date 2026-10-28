//! Every daemon result shape in every text mode, from fixed fake responses.
//! Agent-mode output doubles as the examples agents learn from, so its wording is
//! part of the contract.

use clap::Parser;
use devlanes_cli::*;
use devlanes_proto::{Event, Method, RpcError};
use serde_json::{Value, json};

/// Runtime for `mode` with color off, so snapshots stay readable.
fn runtime(mode: OutputMode) -> Runtime {
    let cli = Cli::parse_from(["devlanes"]);
    Runtime::resolve(
        &cli,
        Signals {
            env_output: Some(mode.as_str().into()),
            stdout_tty: mode == OutputMode::Human,
            stdin_tty: mode == OutputMode::Human,
            no_color: true,
            ..Signals::default()
        },
    )
    .expect("runtime")
}

/// A unit summary in the shape a real daemon sends (captured from `start --json`
/// of the `two-services` fixture): the loopback URL first, then the proxied one
/// with the HTTPS listener's port; units without hosts have only the first.
pub(crate) fn unit(name: &str, state: &str) -> Value {
    let port = 20100 + name.len() as u16;
    let mut urls = vec![json!(format!("http://127.0.0.1:{port}"))];
    if name != "worker" {
        urls.push(json!(format!("https://{name}.acme-2.localhost:8443")));
    }
    json!({
        "name": name, "lane": "acme@2", "state": state,
        "pid": if state == "idle" { Value::Null } else { json!(4100) },
        "exit_code": if state == "failed" { json!(1) } else { Value::Null },
        "ports": {"default": port},
        "urls": urls,
        "actor": {"kind": "human"},
        "reason": if state == "failed" {
            json!({"code": "exited", "data": {"code": 1, "signal": null}, "exit_code": 1})
        } else {
            Value::Null
        },
        "kind": "service",
        "autostart": true, "hidden": false, "tags": [], "color": "blue",
        "started_at_ms": if state == "idle" { Value::Null } else { json!(1_700_000_000_000_u64) },
        "cpu_percent": if state == "ready" { json!(4.5) } else { Value::Null },
        "memory_bytes": if state == "ready" { json!(150_000_000) } else { Value::Null },
    })
}

/// A start result: two ready services with URLs, two without, one failure, one skipped task.
pub(crate) fn start_result() -> Value {
    json!({
        "lane": "acme@2", "branch": "feat/billing",
        "started": ["acme@2:api", "acme@2:web", "acme@2:admin", "acme@2:worker", "acme@2:mailer"],
        "already_running": [], "whole_lane": true, "ready": false, "timed_out": false,
        "failures": [{
            "lane": "acme@2", "unit": "admin", "exit_code": 1,
            "log_tail": ["Error: Cannot find module './env'", "    at main.ts:3:1"], "reason": null,
        }],
        "skipped_tasks": ["seed"],
        "units": [unit("api", "ready"), unit("web", "ready"), unit("admin", "failed"),
                  unit("worker", "ready"), unit("mailer", "ready")],
    })
}

/// A lane summary.
pub(crate) fn lane(index: Option<u32>, branch: &str, state: &str, units: Vec<Value>) -> Value {
    json!({
        "reference": index.map_or("acme@spike".into(), |i| format!("acme@{i}")),
        "project": "acme", "index": index, "branch": branch,
        "path": format!("/work/acme-lanes/{}", branch.replace('/', "-")),
        "state": state, "config_changed": false, "units": units, "profile": null,
        "moves_to": null,
        "domain": index.map(|i| if i == 0 { "acme.localhost".to_owned() } else { format!("acme-{i}.localhost") }),
        "port_block": index.map(|i| [20000 + i * 100, 20099 + i * 100]),
        "reason": null, "needs_env": false, "containers": [],
    })
}

/// A log page.
fn lines(unit: &str, texts: &[&str]) -> Vec<Value> {
    texts
        .iter()
        .enumerate()
        .map(|(seq, text)| json!({"unit": unit, "seq": seq + 1, "timestamp_ms": 1000 + seq, "text": text, "level": null}))
        .collect()
}

/// Fixed daemon answers, one per command shape.
fn fixtures() -> Vec<(&'static str, Method, Value)> {
    let queued_reason = json!({"code": "queued", "exit_code": 0, "data": {
        "position": 6, "queued": 40, "in_flight": 4, "window": 4, "hold": "window_full", "cpu_percent": 93.2,
    }});
    let mut queued_unit = unit("search", "queued");
    queued_unit["reason"] = queued_reason.clone();
    let mut status_lane = lane(
        Some(2),
        "feat/billing",
        "failed",
        vec![
            unit("api", "ready"),
            unit("admin", "failed"),
            unit("worker", "idle"),
            queued_unit.clone(),
        ],
    );
    status_lane["units"][1]["reason"] =
        json!({"code": "exited", "data": {"code": 1, "signal": null}, "exit_code": 1});
    let mut start_queued = start_result();
    start_queued["timed_out"] = json!(true);
    start_queued["units"]
        .as_array_mut()
        .expect("units")
        .push(queued_unit);
    let mut moved = lane(Some(3), "fix/login", "idle", vec![unit("api", "idle")]);
    moved["moves_to"] = json!(0);
    moved["config_changed"] = json!(true);
    let mut env_lane = lane(Some(4), "feat/search", "idle", vec![]);
    env_lane["needs_env"] = json!(true);
    let mut unassigned = lane(None, "spike/xyz", "unassigned", vec![]);
    unassigned["reason"] =
        json!({"code": "unassigned", "data": {"reason": "pool_exhausted"}, "exit_code": 1});
    let mut missing = lane(Some(5), "old/branch", "idle", vec![]);
    missing["reason"] =
        json!({"code": "worktree_missing", "data": {"path": "/work/gone"}, "exit_code": 1});
    let page = |items: Vec<Value>| json!({"items": items, "truncated": false, "next_cursor": null});
    // Idle lanes print their age; whole minutes plus a margin keep snapshots stable.
    let now = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("milliseconds fit u64");
    vec![
        ("start", Method::Start, start_result()),
        ("start_queued", Method::Start, start_queued),
        // `start bg` while bg is a previous daemon's orphan: refused, nothing ran.
        (
            "start_refused_orphan",
            Method::Start,
            json!({
                "lane": "acme@0", "branch": "main", "started": [], "already_running": [],
                "whole_lane": false, "ready": false, "timed_out": false, "skipped_tasks": [],
                "failures": [{"lane": "acme@0", "unit": "bg", "exit_code": null, "log_tail": [],
                    "reason": {"code": "unit_orphaned", "exit_code": 1, "data": {
                        "unit": "bg", "lane": "acme@0", "pid": 4242, "started_at_ms": 1, "orphaned_since_ms": 2,
                        "command": "devlanes stop bg --lane acme@0"}}}],
                "units": [],
            }),
        ),
        ("start_warnings", Method::Start, {
            let mut ready = start_result();
            ready["ready"] = json!(true);
            ready["failures"] = json!([]);
            ready["skipped_tasks"] = json!([]);
            ready["units"][2] = unit("admin", "ready");
            ready["warnings"] = json!([
                {"code": "host_outside_ca", "data": {"lane": "acme@2", "unit": "api", "host": "api.example.com",
                 "permitted": ["localhost", "test"], "suggestion": "api.acme.test"}},
                {"code": "memory_baseline", "data": {"peak_memory_bytes": 8_589_934_592_u64,
                 "available_memory_bytes": 4_294_967_296_u64, "units": []}},
                {"code": "lanes_to_resume", "data": {"lanes": ["acme@1", "shop@0"]}},
            ]);
            ready
        }),
        (
            "restart_ready",
            Method::Restart,
            json!({
                "lane": "acme@2", "branch": "feat/billing", "started": ["acme@2:api"],
                "already_running": [], "whole_lane": false, "ready": true, "timed_out": false,
                "failures": [], "skipped_tasks": [], "units": [unit("api", "ready")],
            }),
        ),
        // `reload` after the file dropped worker and changed api: worker stopped,
        // api restarted.
        (
            "reload_units_removed",
            Method::Reload,
            json!({
                "lane": "acme@2", "branch": "feat/billing", "started": ["acme@2:api"],
                "already_running": [], "whole_lane": false, "ready": true, "timed_out": false,
                "failures": [], "skipped_tasks": [], "units": [unit("api", "ready")],
                "warnings": [{"code": "units_removed", "data": {"lane": "acme@2", "units": ["worker"],
                    "message": "stopped worker, which the project file no longer has"}}],
            }),
        ),
        // Nothing to apply: one ok line, no warning and no `nothing to start`.
        (
            "reload_nothing",
            Method::Reload,
            json!({
                "lane": "acme@2", "started": [], "already_running": [], "whole_lane": false,
                "ready": true, "timed_out": false, "failures": [], "skipped_tasks": [], "units": [],
                "warnings": [{"code": "nothing_to_reload", "data": {"lane": "acme@2",
                    "message": "acme@2 already runs what its project file says"}}],
            }),
        ),
        // `reload web` when web has no change.
        (
            "reload_nothing_for_units",
            Method::Reload,
            json!({
                "lane": "acme@2", "started": [], "already_running": [], "whole_lane": false,
                "ready": true, "timed_out": false, "failures": [], "skipped_tasks": [], "units": [],
                "warnings": [{"code": "nothing_to_reload", "data": {"lane": "acme@2", "units": ["web"],
                    "message": "acme@2 already runs what its project file says"}}],
            }),
        ),
        // Infra changed, so the reload restarted the whole lane.
        (
            "reload_restarted_lane",
            Method::Reload,
            json!({
                "lane": "acme@2", "branch": "feat/billing", "started": ["acme@2:api", "acme@2:web"],
                "already_running": [], "whole_lane": true, "ready": true, "timed_out": false,
                "failures": [], "skipped_tasks": [],
                "units": [unit("api", "ready"), unit("web", "ready")],
                "warnings": [{"code": "reload_restarted_lane", "data": {"lane": "acme@2",
                    "message": "infra or ports changed, so the whole lane restarted"}}],
            }),
        ),
        (
            "resume",
            Method::Resume,
            page(vec![
                json!({
                    "lane": "acme@0", "branch": "main", "started": ["acme@0:api"], "already_running": [],
                    "whole_lane": true, "ready": true, "timed_out": false, "failures": [],
                    "skipped_tasks": [], "units": [unit("api", "ready")],
                }),
                json!({"lane": "acme@3", "error": {"code": "docker_unavailable", "data": {"message": "no Docker engine is reachable"}, "exit_code": 1}}),
            ]),
        ),
        (
            "stop",
            Method::Stop,
            page(vec![unit("api", "stopped"), unit("web", "exited")]),
        ),
        (
            "status",
            Method::Status,
            page(vec![
                lane(
                    Some(0),
                    "main",
                    "running",
                    vec![unit("api", "ready"), unit("web", "ready")],
                ),
                status_lane.clone(),
                moved,
                env_lane,
                missing,
                unassigned,
            ]),
        ),
        ("status_one_lane", Method::Status, page(vec![status_lane])),
        // The file changed api, dropped worker and added mailer since the lane started.
        ("status_drift", Method::Status, {
            let mut api = unit("api", "ready");
            api["drift"] = json!({"kind": "changed", "fields": ["run", "env API_URL"]});
            let mut worker = unit("worker", "ready");
            worker["drift"] = json!({"kind": "removed"});
            let mut mailer = unit("mailer", "idle");
            mailer["drift"] = json!({"kind": "added"});
            let mut drifted = lane(
                Some(2),
                "feat/billing",
                "running",
                vec![api, unit("web", "ready"), worker, mailer],
            );
            drifted["config_changed"] = json!(true);
            page(vec![drifted])
        }),
        // Only the infra changed (reload restarts the lane), and a file that no longer
        // loads: `config changed` without a command, since a reload would refuse.
        ("status_drift_lane_only", Method::Status, {
            let mut infra = lane(
                Some(2),
                "feat/billing",
                "running",
                vec![unit("api", "ready")],
            );
            infra["config_changed"] = json!(true);
            infra["infra_changed"] = json!(true);
            let mut broken = lane(Some(3), "fix/login", "running", vec![unit("api", "ready")]);
            broken["config_changed"] = json!(true);
            page(vec![infra, broken])
        }),
        // An `infra.external` entry: no process, its own state word.
        ("status_external", Method::Status, {
            let mut pgadmin = unit("pgadmin", "external");
            pgadmin["kind"] = json!("external");
            pgadmin["role"] = json!("external");
            pgadmin["pid"] = Value::Null;
            pgadmin["started_at_ms"] = Value::Null;
            pgadmin["ports"] = json!({"default": 5050});
            pgadmin["urls"] = json!([
                "http://127.0.0.1:5050",
                "https://pgadmin.acme-2.localhost:8443"
            ]);
            page(vec![lane(
                Some(2),
                "feat/billing",
                "running",
                vec![unit("api", "ready"), pgadmin],
            )])
        }),
        // An idle lane keeps `state`; `power` adds the level and since when.
        ("status_idle", Method::Status, {
            let idle = |level: &str, minutes: u64| {
                let mut api = unit("api", "ready");
                api["power"] = json!(level);
                let mut worker = unit("worker", "ready");
                worker["power"] = json!(level);
                worker["ready_check"] = json!("started");
                (
                    vec![api, worker],
                    json!({"level": level, "since_ms": now - minutes * 60_000 - 5_000,
                           "last_activity_ms": now - (minutes + 5) * 60_000, "last_activity": "proxy"}),
                )
            };
            let (units, power) = idle("frozen", 12);
            let mut frozen = lane(Some(0), "main", "running", units);
            frozen["power"] = power;
            let (units, power) = idle("throttled", 3);
            let mut throttled = lane(Some(2), "feat/billing", "running", units);
            throttled["power"] = power;
            page(vec![
                frozen,
                throttled,
                lane(Some(3), "fix/login", "running", vec![unit("api", "ready")]),
            ])
        }),
        ("status_one_frozen_lane", Method::Status, {
            let mut api = unit("api", "ready");
            api["power"] = json!("frozen");
            let mut frozen = lane(Some(0), "main", "running", vec![api]);
            frozen["power"] = json!({"level": "frozen", "since_ms": now - 30 * 60_000 - 5_000});
            page(vec![frozen])
        }),
        (
            "debug_freeze",
            Method::DebugFreeze,
            json!({"lane": "acme@2", "level": "frozen"}),
        ),
        (
            "debug_thaw",
            Method::DebugThaw,
            json!({"lane": "acme@2", "thawed": true}),
        ),
        (
            "debug_thaw_not_idle",
            Method::DebugThaw,
            json!({"lane": "acme@2", "thawed": false}),
        ),
        (
            "events_idle",
            Method::Subscribe,
            page(vec![
                json!({"seq": 1, "event": "lane.idle", "data": {"lane": "acme@2", "level": "throttled"}}),
                json!({"seq": 2, "event": "lane.idle", "data": {"lane": "acme@2", "level": "frozen"}}),
                json!({"seq": 3, "event": "lane.thawed", "data": {"lane": "acme@2", "from": "frozen", "activity": "proxy"}}),
                json!({"seq": 4, "event": "lane.idle_failed", "data": {"lane": "acme@0", "level": "frozen",
                    "code": "freeze_failed", "message": "api: operation not permitted"}}),
                json!({"seq": 5, "event": "unit.changed", "data": {"unit": {"name": "api", "lane": "acme@2", "state": "ready", "power": "frozen"}}}),
                json!({"seq": 6, "event": "lane.thawed", "data": {"lane": "acme@0", "from": "throttled", "activity": "stop"}}),
                json!({"seq": 7, "event": "lane.idle_failed", "data": {"lane": "acme@2", "level": "normal",
                    "code": "thaw_failed", "message": "api: no such process"}}),
            ]),
        ),
        ("status_lane_warnings", Method::Status, {
            let mut warned = lane(
                Some(2),
                "feat/billing",
                "running",
                vec![unit("api", "ready")],
            );
            warned["warnings"] = json!([
                {"code": "ports_pending", "data": {"lane": "acme@2", "message": "the edited config needs a different port block"}},
                {"code": "host_conflict", "data": {"host": "api.acme.localhost", "lane": "acme@0", "other": "acme@2",
                 "message": "api.acme.localhost is routed by acme@0 and acme@2; acme@0 keeps it because it has running units"}},
            ]);
            page(vec![
                lane(Some(0), "main", "running", vec![unit("api", "ready")]),
                warned,
            ])
        }),
        (
            "lane_new",
            Method::LaneNew,
            json!({"lane": lane(Some(6), "feat/checkout", "idle", vec![])}),
        ),
        (
            "lane_clean",
            Method::LaneClean,
            json!({
                "lane": "acme@2", "stopped": ["api", "web"],
                "tasks": [{"task": "drop-db", "exit_code": 0}],
                "teardown": [{"entry": "vhost", "exit_code": 3, "ignored": false}],
                "infra": {"compose": "acme-2"},
            }),
        ),
        (
            "lane_gc",
            Method::LaneGc,
            json!({"cleaned": ["acme@5"], "offsets_removed": 3}),
        ),
        (
            "lane_gc_kept",
            Method::LaneGc,
            json!({"cleaned": [], "kept_running": ["acme@3", "acme@4"], "offsets_removed": 0}),
        ),
        ("exec", Method::Exec, {
            let mut exec = unit("probe", "ready");
            exec["kind"] = json!("exec");
            exec["urls"] = json!([]);
            exec["ports"] = json!({});
            exec
        }),
        (
            "wait_ready",
            Method::Wait,
            json!({"lane": "acme@2", "unit": "api", "state": "ready", "ready": true}),
        ),
        (
            "wait_failed",
            Method::Wait,
            json!({
                "lane": "acme@2", "unit": "admin", "state": "failed", "ready": false, "failed": ["admin"],
                "reason": {"code": "exited", "data": {"code": 1}, "exit_code": 1}, "exit_code": 1,
                "log_tail": ["Error: Cannot find module './env'"],
            }),
        ),
        (
            "wait_idle",
            Method::Wait,
            json!({"lane": "acme@2", "unit": null, "state": "idle", "ready": false}),
        ),
        (
            "errors",
            Method::Errors,
            page(vec![
                json!({"lane": "acme@2", "unit": "api", "state": "ready", "exit_code": null, "crashes": 2,
                   "reason": null, "error_lines": ["warn: slow query"], "port_conflicts": []}),
                json!({"lane": "acme@2", "unit": "admin", "state": "failed", "exit_code": 1, "crashes": 1,
                   "reason": {"code": "exited", "data": {"code": 1}, "exit_code": 1},
                   "error_lines": ["Error: Cannot find module './env'"],
                   "port_conflicts": [{"port": 20102, "pid": 911}]}),
            ]),
        ),
        ("errors_none", Method::Errors, page(vec![])),
        (
            "why",
            Method::Why,
            json!({
                "lane": "acme@2", "unit": "admin", "state": "idle",
                "reasons": [
                    {"code": "not_in_profile", "data": {"profile": "e2e"}},
                    {"code": "needs_env", "data": {"lane": "acme@2"}},
                    {"code": "config_error", "data": {"message": "unknown field", "file": "devlanes.yaml", "line": 4, "column": 3}},
                    {"code": "port_in_use", "data": {"port": 20102, "pid": 911}},
                    {"code": "waiting_after_services", "data": {}},
                    {"code": "ready_timeout", "data": {}},
                    {"code": "exited", "data": {"code": 1, "signal": null}},
                    {"code": "stopped", "data": {"exit_code": 0, "signal": null, "requested": true}},
                    {"code": "autostart_off", "data": {}},
                    {"code": "unassigned", "data": {"reason": "lanes_max", "max": 4}},
                    {"code": "no_config", "data": {}},
                    {"code": "sudo_denied", "data": {"hosts": ["admin.acme-2.localhost"], "message": "sudo said no"}},
                {"code": "queued", "data": queued_reason["data"].clone()},
                ],
            }),
        ),
        (
            "env",
            Method::Env,
            json!({"lane": "acme@2", "unit": "api", "env": {"PORT": "20101", "DATABASE_URL": "postgres://localhost:20110/acme", "MULTI": "a\nb"}}),
        ),
        (
            "env_explain",
            Method::Env,
            json!({"lane": "acme@2", "unit": "api", "items": [
            {"key": "PORT", "value": "20101", "layer": "devlanes"},
            {"key": "API_TOKEN", "value": "tok_test", "layer": "global"},
            {"key": "NODE_ENV", "value": "development", "layer": "service"},
        ], "truncated": false, "next_cursor": null}),
        ),
        (
            "url",
            Method::Url,
            json!({"lane": "acme@2", "unit": "web", "url": "https://web.acme-2.localhost", "urls": ["http://127.0.0.1:20101", "https://web.acme-2.localhost"]}),
        ),
        (
            "url_clients",
            Method::Url,
            json!({"lane": "acme@2", "unit": "web", "url": "https://web.acme-2.localhost:8443",
            "urls": ["http://127.0.0.1:20101", "https://web.acme-2.localhost:8443"],
            "clients": [
                {"client": "laptop", "id": "0123abcd", "alias": "hetzner", "url": "https://web.acme-2.localhost", "reachable": true, "reason": null},
                {"client": "desktop", "id": "4567ef01", "alias": "hetzner", "url": null, "reachable": false,
                 "reason": "port 20101 is in use there by postgres (pid 812)"},
            ]}),
        ),
        (
            "hosts_clients",
            Method::Hosts,
            json!({
                "items": [
                    {"host": "api.acme-2.localhost", "project": "acme", "lane": "acme@2", "unit": "api", "port": 20101, "fallback": false, "state": "ready",
                     "clients": [{"client": "laptop", "url": "https://api.acme-2.localhost:9443", "reachable": true, "reason": null}]},
                    {"host": "web.acme-2.localhost", "project": "acme", "lane": "acme@2", "unit": "web", "port": 20100, "fallback": false, "state": "ready",
                     "clients": [{"client": "laptop", "url": null, "reachable": false, "reason": "the lane is not mirrored there"}]},
                ],
                "proxy": {"https": {"bound": true, "port": 443, "fallback_reason": null}},
                "truncated": false, "next_cursor": null,
            }),
        ),
        (
            "hosts",
            Method::Hosts,
            json!({
                "items": [
                    {"host": "api.acme-2.localhost", "project": "acme", "lane": "acme@2", "unit": "api", "port": 20101, "fallback": false, "state": "ready"},
                    {"host": "acme-2.localhost", "project": "acme", "lane": "acme@2", "unit": "web", "port": 20100, "fallback": true, "state": "idle"},
                ],
                "proxy": {"http": {"bound": true, "port": 80, "fallback_reason": null},
                          "https": {"bound": true, "port": 8443, "fallback_reason": "port 443 needs root"}},
                "truncated": false, "next_cursor": null,
            }),
        ),
        (
            "hosts_warnings",
            Method::Hosts,
            json!({
                "items": [
                    {"host": "api.acme.localhost", "project": "acme", "lane": "acme@0", "unit": "api", "port": 20001, "fallback": false, "state": "ready"},
                ],
                "proxy": {"http": {"bound": true, "port": 80, "fallback_reason": null}},
                "warnings": [
                    {"code": "host_conflict", "data": {"host": "api.acme.localhost", "lane": "acme@0", "other": "acme@2",
                     "message": "api.acme.localhost is routed by acme@0 and acme@2; acme@0 keeps it because it comes first by project and lane index"}},
                    {"code": "host_rejected", "data": {"lane": "acme@2", "unit": "web", "host": "web_app.localhost",
                     "message": "`web_app.localhost` contains `_`, which DNS names cannot; the route is left out"}},
                ],
                "truncated": false, "next_cursor": null,
            }),
        ),
        (
            "logs",
            Method::Logs,
            page(lines("api", &["listening on :20101", "GET / 200"])),
        ),
        (
            "logs_merged",
            Method::Logs,
            page({
                let mut merged = lines("api", &["GET / 200"]);
                merged.extend(lines("web", &["compiled in 120ms"]));
                merged
            }),
        ),
        (
            "logs_all",
            Method::Logs,
            json!({"lane": "acme@2", "units": [
                {"unit": "api", "lines": lines("api", &["listening on :20101"]), "truncated": false},
                {"unit": "web", "lines": lines("web", &["compiled", "compiled"]), "truncated": true},
            ]}),
        ),
        (
            "run_ok",
            Method::Run,
            json!({"task": "migrate", "lane": "acme@2", "unit": "migrate", "interactive": false, "exit_code": 0, "state": "exited"}),
        ),
        (
            "run_failed",
            Method::Run,
            json!({"task": "test", "lane": "acme@2", "unit": "test", "interactive": false, "exit_code": 2, "state": "failed"}),
        ),
        (
            "run_detached",
            Method::Run,
            json!({"task": "seed", "lane": "acme@2", "unit": "seed", "interactive": false, "detached": true}),
        ),
        (
            "run_interactive",
            Method::Run,
            json!({"task": "console", "lane": "acme@2", "unit": "console", "interactive": true, "detached": false}),
        ),
        (
            "task_list",
            Method::TaskList,
            json!({
                "items": [
                    {"name": "migrate", "when": "lane.start", "after": null, "profiles": null, "steps": null, "confirm": false, "interactive": false, "run_script": {"file": "package.json", "name": "db:migrate"}},
                    {"name": "seed", "when": "lane.create", "after": "services", "profiles": ["dev"], "steps": null, "confirm": true, "interactive": false, "run_script": null},
                    {"name": "reset", "when": null, "after": null, "profiles": null, "steps": ["drop", "migrate"], "confirm": false, "interactive": false, "run_script": null},
                ],
                "sources": [{"file": "package.json", "runner": "npm"}],
                "truncated": false, "next_cursor": null,
            }),
        ),
        (
            "metrics",
            Method::Metrics,
            json!({"units": [
                {"lane": "acme@2", "unit": "api", "cpu_percent": 12.5, "memory_bytes": 314_572_800, "history": []},
                {"lane": "acme@2", "unit": "web", "cpu_percent": 0.4, "memory_bytes": 52_428_800, "history": []},
                {"lane": "acme@0", "unit": "api", "cpu_percent": null, "memory_bytes": null, "history": []},
                // Stopped within the range: sent for the monitor's graphs, left out of `top`.
                {"lane": "acme@1", "unit": "worker", "cpu_percent": null, "memory_bytes": null, "history": [[1, 3.0, 1024]], "ended": true},
            ]}),
        ),
        (
            "metrics_peaks",
            Method::Metrics,
            json!({"lane": "acme@2", "peaks": [
                {"unit": "api", "peak_memory_bytes": 1_073_741_824, "peak_cpu_percent": 125.5},
            ]}),
        ),
        (
            "events",
            Method::Subscribe,
            page(vec![
                json!({"seq": 1, "event": "lane.phase", "data": {"lane": "acme@2", "phase": "infra", "state": "done"}}),
                json!({"seq": 2, "event": "unit.changed", "data": {"unit": unit("api", "ready")}}),
                json!({"seq": 3, "event": "task", "data": {"lane": "acme@2", "task": "migrate", "state": "failed", "exit_code": 1}}),
                json!({"seq": 4, "event": "remote.state", "data": {"host": "box", "state": "connected"}}),
            ]),
        ),
        (
            "infra_up",
            Method::InfraUp,
            json!({"lane": "acme@2", "infra": {}, "endpoints": {
                "postgres": {"port": 20110, "ports": {"5432": 20110}},
                "redis": {"port": 20111, "ports": {"6379": 20111}},
            }}),
        ),
        (
            "infra_status",
            Method::InfraStatus,
            json!({"lane": "acme@2", "infra": {
                "shared": {"project": "acme", "services": [{"service": "postgres", "state": "running", "health": "healthy"}]},
            }}),
        ),
        (
            "doctor",
            Method::Doctor,
            json!({
                "version": "0.1.0", "pid": 4242, "home": "/home/u/.devlanes", "db_path": "/home/u/.devlanes/state.db",
                "schema_version": 5, "supported_schema_version": 5, "socket": "/home/u/.devlanes/run/daemon.sock",
                "sandbox": {"codex_sandbox": false, "app_sandbox": false},
                "infra": {"docker": {"reachable": false, "reason": "docker_unavailable"}, "compose": {"error": "docker_unavailable"}},
                "proxy": {"http": {"bound": true, "port": 80, "fallback_reason": null}, "https": {"bound": false, "reason": "no_ca"}},
                "ca": {"present": true, "path": "/home/u/.devlanes/ca/ca.pem", "trusted": "unknown"},
                "projects": 2, "subscribers": 1,
                "hosts": {"mode": "auto", "probe": "fails", "state": "failed", "names": null, "desired": 3, "skipped": 1,
                          "last_error": {"code": "helper_missing", "message": "the hosts helper is not installed"}, "tld_error": null},
            }),
        ),
        (
            "doctor_findings",
            Method::Doctor,
            json!({
                "version": "0.2.0", "pid": 4242, "home": "/home/u/.devlanes", "db_path": "/home/u/.devlanes/state.db",
                "schema_version": 5, "supported_schema_version": 5, "socket": "/home/u/.devlanes/run/daemon.sock",
                "sandbox": {"codex_sandbox": false, "app_sandbox": false},
                "infra": {"docker": {"reachable": true, "version": "27.1.1"}, "compose": {"version": "2.29.1", "supports_lane_overrides": true}},
                "proxy": {
                    "http": {"bound": true, "port": 80, "fallback_reason": null, "serving": false, "listeners": [
                        {"address": "127.0.0.1:80", "running": true, "accept_errors": 3, "last_error": "Too many open files (os error 24)"},
                        {"address": "[::1]:80", "running": false, "accept_errors": 0, "last_error": null},
                    ]},
                    "https": {"bound": true, "port": 443, "fallback_reason": null, "serving": true, "listeners": [
                        {"address": "127.0.0.1:443", "running": true, "accept_errors": 0, "last_error": null},
                    ], "shared_with": [{"pid": 5150, "command": "caddy"}]},
                },
                "ca": {"present": true, "kind": "devlanes", "path": "/home/u/.devlanes/ca/ca.pem", "trusted": "unknown",
                       "name_constrained": false,
                       "trust": [{"store": "system", "state": "untrusted"},
                                 {"store": "firefox", "state": "unknown", "reason": "certutil is not installed"}],
                       "hint": "the devlanes CA has no name constraints (it predates them), so a leaked key could sign certificates for any site",
                       "fix": "devlanes certs untrust && rm -r /home/u/.devlanes/ca && devlanes certs trust && devlanes daemon restart"},
                "route_warnings": [{"code": "host_rejected", "data": {"lane": "acme@2", "unit": "web", "host": "web_app.localhost",
                                    "message": "`web_app.localhost` contains `_`, which DNS names cannot; the route is left out"}}],
                "ca_warnings": [{"code": "host_outside_ca", "data": {
                    "lane": "acme@2", "unit": "api", "host": "api.example.com",
                    "permitted": ["localhost", "test"], "suggestion": "api.acme-2.localhost",
                }}],
                "projects": 2, "subscribers": 1,
                "hosts": {"mode": "auto", "probe": "loopback", "state": "applied", "names": 3, "desired": 3, "skipped": 0,
                          "last_error": null, "tld_error": null,
                          "helper": {"state": "mismatch", "level": "warning", "installed": "0.1.0", "expected": "0.2.0",
                                     "message": "The installed hosts helper is version 0.1.0 and devlanes is 0.2.0.",
                                     "fix": "devlanes certs trust --hosts"}},
                "user_config": {
                    "path": "/home/u/.devlanes/config.yaml", "valid": false,
                    "error": {"message": "unknown field `proxyy`", "file": "/home/u/.devlanes/config.yaml", "line": 4, "column": 1},
                    "warnings": [{"path": "tls.trust.1", "message": "mobile certificate trust is not supported yet; target ignored"}],
                },
            }),
        ),
        (
            "config_check",
            Method::ConfigCheck,
            json!({"project": "acme", "path": "/work/acme/devlanes.yaml", "warnings": [
                {"path": "templates.unused", "message": "template is never used"},
                {"code": "unknown_platform", "path": "services.api.platform",
                 "message": "no contributor provides platform `elixir`",
                 "data": {"service": "api", "platform": "elixir", "known": ["node", "python"]}},
                {"code": "host_outside_ca", "path": "services.api.hosts",
                 "message": "api.example.com is outside the names the devlanes CA can certify",
                 "host": "api.example.com", "suggestion": "api.acme.test"},
                {"code": "ports_pending", "path": "lanes", "lane": "acme@0",
                 "message": "the edited config needs a different port block"},
                {"code": "host_conflict", "path": "services.web.hosts", "host": "web.acme.localhost",
                 "lane": "shop@0", "other": "acme@0", "message": "web.acme.localhost is routed by shop@0 and acme@0"},
            ]}),
        ),
        // `lanes` holds only unset fields, so it is left out.
        (
            "config_show",
            Method::ConfigShow,
            json!({
                "project": "acme", "lane": "acme@0",
                "files": ["/work/acme/devlanes.yaml", "/work/acme/devlanes.local.yaml"],
                "config": {
                    "project": "acme",
                    "lanes": {"primary": null, "max": null},
                    "services": {"api": {
                        "extends": "node", "cwd": null,
                        "run": "npm run dev:local",
                        "env": {"NODE_ENV": "development", "TOKEN": "${{ env.API_TOKEN }}"},
                        "hosts": ["api.${{ lane.domain }}"],
                    }},
                },
            }),
        ),
        (
            "daemon_status",
            Method::DaemonStatus,
            json!({"version": "0.1.0", "pid": 4242, "uptime_ms": 7_380_000, "managed_processes": 6, "socket": "/home/u/.devlanes/run/daemon.sock"}),
        ),
        (
            "daemon_status_pipe",
            Method::DaemonStatus,
            json!({"version": "0.1.0", "pid": 4242, "uptime_ms": 0, "managed_processes": 0, "socket": r"\\.\pipe\devlanes-723741743a863791"}),
        ),
        (
            "daemon_stop",
            Method::DaemonShutdown,
            json!({"stopping": true, "processes": 6}),
        ),
        (
            "reset_state",
            Method::DaemonResetState,
            json!({"stopped": 6}),
        ),
    ]
}

/// The three text modes of one fixture, separated by mode headers.
fn render_text(method: Method, data: &Value) -> String {
    let mut output = String::new();
    for mode in [OutputMode::Human, OutputMode::Plain, OutputMode::Agent] {
        let rendered = runtime(mode)
            .reporter()
            .result(&Report::new(method, data))
            .expect("render");
        output.push_str(&format!(
            "--- {} (exit {})\n",
            mode.as_str(),
            rendered.exit_code
        ));
        output.push_str(&rendered.stdout);
        if !rendered.stderr.is_empty() {
            output.push_str(&format!("stderr: {}", rendered.stderr));
        }
    }
    output
}

#[test]
fn every_result_shape_in_every_text_mode() {
    for (name, method, data) in fixtures() {
        let json = runtime(OutputMode::Json)
            .reporter()
            .result(&Report::new(method, &data))
            .expect("json");
        let mut printed = serde_json::from_str::<Value>(&json.stdout).expect("json output");
        // doctor adds the client's own output section (tested on its own).
        if method == Method::Doctor {
            printed.as_object_mut().map(|map| map.remove("output"));
        }
        assert_eq!(
            printed, data,
            "{name}: JSON mode prints the daemon data unchanged"
        );
        let text = render_text(method, &data);
        assert!(
            !text.contains(": {"),
            "{name}: raw JSON leaked into text output\n{text}"
        );
        insta::assert_snapshot!(format!("render_{name}"), text);
    }
}

/// Doctor without a daemon: the CLI's own facts, the stopped daemon as a warning
/// (exit 0), and exit 1 once a local check fails.
#[test]
fn doctor_without_a_daemon() {
    let mut data = json!({
        "running": false, "version": "0.3.0", "home": "/home/u/.devlanes",
        "socket": "/home/u/.devlanes/run/daemon.sock", "db_path": "/home/u/.devlanes/state.db",
        "home_check": {"path": "/home/u/.devlanes", "exists": true, "mode": "700", "problem": null},
        "sandbox": {"codex_sandbox": false, "app_sandbox": false},
        "ca": {"present": true, "kind": "devlanes", "path": "/home/u/.devlanes/ca/ca.pem",
               "name_constrained": true, "hint": null, "fix": null,
               "trust": [{"store": "system", "state": "trusted"}]},
        "user_config": {"path": "/home/u/.devlanes/config.yaml", "valid": true, "error": null, "errors": [], "warnings": []},
        "port_holders": [{"listener": "https", "port": 443, "pid": 812, "command": "nginx"}],
        "hosts": {"helper": {"state": "ok", "level": "ok", "installed": "0.3.0", "expected": "0.3.0"}},
        "release": {"version": "0.3.0", "build": "local", "daemon_version": null, "install_method": "install.sh",
                    "check": null, "latest": null},
    });
    let text = render_text(Method::Doctor, &data);
    insta::assert_snapshot!("render_doctor_without_daemon", text);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        !text.contains("Docker") && !text.contains("schema") && !text.contains("projects"),
        "no daemon facts invented: {text}"
    );
    // A local failure still fails doctor.
    data["ca"]["trust"] = json!([{"store": "system", "state": "untrusted"}]);
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 1)"), "{text}");
    assert!(
        text.contains("fail tls: CA not trusted by system"),
        "{text}"
    );
}

/// Windows facts: an elevated daemon or terminal and a terminal job that kills
/// on close are warnings (exit 0); a home other accounts can reach is a warning
/// while the daemon can tighten it and a failure with the `icacls` fix after.
#[test]
fn doctor_words_the_windows_facts() {
    let mut data = json!({
        "running": false, "version": "0.3.0", "home": r"C:\Users\u\.devlanes",
        "socket": r"\\.\pipe\devlanes-0123456789abcdef", "db_path": r"C:\Users\u\.devlanes\state.db",
        "home_check": {"path": r"C:\Users\u\.devlanes", "exists": true, "problem": null},
        "sandbox": {"codex_sandbox": false, "app_sandbox": false},
        "elevated": true,
        "terminal": {"elevated": true, "job_kills_on_close": true},
    });
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(text.contains("the daemon runs as administrator"), "{text}");
    assert!(
        text.contains("this terminal runs as administrator"),
        "{text}"
    );
    assert!(text.contains("stops with it"), "{text}");
    data["elevated"] = json!(false);
    data["terminal"] = json!({"elevated": false, "job_kills_on_close": false});
    let text = render_text(Method::Doctor, &data);
    assert!(
        !text.contains("administrator") && !text.contains("stops with it"),
        "{text}"
    );

    data["home_check"] = json!({"path": r"C:\Users\u\.devlanes", "exists": true, "problem": "acl", "acl": ["S-1-1-0"], "fix": null});
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        text.contains("the daemon makes it private when it starts"),
        "{text}"
    );
    let fix = r#"icacls "C:\Users\u\.devlanes" /inheritance:r /grant:r "*S-1-5-21-1:(OI)(CI)F" && icacls "C:\Users\u\.devlanes\*" /reset /T /C"#;
    data["home_check"]["fix"] = json!(fix);
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 1)"), "{text}");
    assert!(text.contains("other accounts can access it"), "{text}");
    assert!(text.contains(fix), "{text}");
    data["home_check"] =
        json!({"path": r"C:\Users\u\.devlanes", "exists": true, "problem": "reparse_point"});
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("it is a symlink or junction"), "{text}");

    data["home_check"] = json!({"path": r"C:\Users\u\.devlanes", "exists": true, "problem": null});
    data["shell"] = json!({"path": null});
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 1)"), "{text}");
    assert!(text.contains("Git for Windows was not found"), "{text}");
    data["shell"] = json!({"path": r"C:\Program Files\Git\usr\bin\sh.exe"});
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        text.contains(r"run strings use C:\Program Files\Git\usr\bin\sh.exe"),
        "{text}"
    );
}

/// A lane port, proxy port or mirrored port in a Windows reserved port range is
/// a warning naming the `netsh` command. A range inside `ports.range` alone is
/// an ok line, since new lanes skip it, as is no overlap at all; an unreadable
/// `netsh` is a warning.
#[test]
fn doctor_words_reserved_port_ranges() {
    let command = "netsh int ipv4 show excludedportrange protocol=tcp";
    let mut data = json!({
        "running": false, "version": "0.3.0", "home": r"C:\Users\u\.devlanes",
        "socket": r"\\.\pipe\devlanes-0123456789abcdef", "db_path": r"C:\Users\u\.devlanes\state.db",
        "sandbox": {"codex_sandbox": false, "app_sandbox": false},
        "reserved_ports": {
            "ranges": [{"start": 27339, "end": 27339}, {"start": 8440, "end": 8449}],
            "pool": {"start": 20000, "end": 32767},
            "pool_overlaps": [{"start": 27339, "end": 27339}],
            "lanes": [{"lane": "acme@1", "ports": [27339]}],
            "proxy": [{"listener": "https", "port": 8443, "fallback": true}],
            "mirrored": [27339],
            "error": null,
            "command": command,
        },
    });
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        text.contains("acme@1 has port 27339 in a Windows reserved port range"),
        "{text}"
    );
    assert!(text.contains("devlanes lane clean acme@1"), "{text}");
    assert!(
        text.contains("the https proxy fallback port 8443 is in a Windows reserved port range"),
        "{text}"
    );
    assert!(text.contains("mirrored port 27339"), "{text}");
    assert!(text.contains(command), "{text}");
    assert!(!text.contains("new lanes skip"), "{text}");

    data["reserved_ports"] = json!({
        "ranges": [{"start": 27339, "end": 27339}], "pool": {"start": 20000, "end": 32767},
        "pool_overlaps": [{"start": 27339, "end": 27339}], "lanes": [], "proxy": [],
        "mirrored": [], "error": null, "command": command,
    });
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains(
            "ok ports: new lanes skip the Windows reserved ports in ports.range 20000-32767 (27339)"
        ),
        "{text}"
    );
    assert!(!text.contains("warn ports"), "{text}");

    data["reserved_ports"] = json!({
        "ranges": [{"start": 49745, "end": 49844}], "pool": {"start": 20000, "end": 32767},
        "pool_overlaps": [], "lanes": [], "proxy": [], "mirrored": [], "error": null,
        "command": command,
    });
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains("no Windows reserved port range overlaps"),
        "{text}"
    );

    data["reserved_ports"] =
        json!({"error": "ipv4: netsh failed; ipv6: netsh failed", "command": command});
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        text.contains("cannot read the Windows reserved port ranges: ipv4: netsh failed"),
        "{text}"
    );
}

/// Inside WSL the CLI adds `wsl` and the daemon `windows_drive_worktrees`:
/// interop on passes; no systemd, a `tls.trust` without `windows` and a
/// worktree on a Windows drive warn, and doctor still exits 0.
#[test]
fn doctor_inside_wsl() {
    let data = json!({
        "running": false,
        "ca": {"present": true, "kind": "devlanes", "path": "/home/u/.devlanes/ca/ca.pem",
               "trust": [{"store": "system", "state": "trusted"}]},
        "wsl": {"distro": "Ubuntu", "version": 2, "interop": true, "systemd": false},
        "windows_drive_worktrees": [{"lane": "acme@2", "path": "/mnt/c/src/acme", "drive": "C:\\"}],
    });
    let text = render_text(Method::Doctor, &data);
    insta::assert_snapshot!("render_doctor_inside_wsl", text);
    assert!(text.contains("--- plain (exit 0)"), "{text}");

    // Interop off on WSL 1 without a distro name; `windows` listed, systemd on.
    let mut quiet = data.clone();
    quiet["wsl"] = json!({"distro": null, "version": 1, "interop": false, "systemd": true});
    quiet["ca"]["trust"] = json!([{"store": "windows", "state": "untrusted"}]);
    quiet["windows_drive_worktrees"] = json!([]);
    let text = render_text(Method::Doctor, &quiet);
    assert!(
        text.contains("warn wsl: WSL 1, Windows interop off"),
        "{text}"
    );
    assert!(
        text.contains("fail tls: CA not trusted by windows"),
        "{text}"
    );
    for absent in ["leaves out windows", "systemd", "Windows drive"] {
        assert!(!text.contains(absent), "{absent}: {text}");
    }

    // Native Linux: no wsl facts, no wsl group.
    let mut native = data;
    let map = native.as_object_mut().expect("object");
    map.remove("wsl");
    map.remove("windows_drive_worktrees");
    let text = render_text(Method::Doctor, &native);
    assert!(!text.to_ascii_lowercase().contains("wsl"), "{text}");
}

/// The WSL start warning and the `daemon install` refusal name the fix.
#[test]
fn wsl_warning_and_error_are_worded() {
    let event = Event {
        seq: 1,
        event: "lane.warning".into(),
        data: json!({"lane": "acme@2", "code": "windows_drive", "data": {
            "lane": "acme@2", "path": "/mnt/c/src/acme", "drive": "C:\\",
        }}),
    };
    let line = runtime(OutputMode::Agent)
        .reporter()
        .event(Method::Subscribe, &event)
        .expect("event")
        .stdout;
    assert_eq!(
        line,
        "warn acme@2: /mnt/c/src/acme is on the Windows drive C:\\: file access is slow and file watching misses edits made from Windows; move the repository to the Linux filesystem, for example ~/src\n"
    );
    let error = RpcError::new("wsl_systemd_off", json!({"distro": "Ubuntu"}), 1);
    let rendered = runtime(OutputMode::Agent).reporter().error(&error, None);
    assert!(
        rendered
            .stderr
            .contains("systemd is not running in this WSL distro"),
        "{}",
        rendered.stderr
    );
    assert!(
        rendered
            .stderr
            .contains("set systemd=true under [boot] in /etc/wsl.conf"),
        "{}",
        rendered.stderr
    );
}

#[test]
/// `daemon stop` names what it stopped and the `lane.stop` tasks that failed.
fn daemon_stop_reports_units_and_failed_tasks() {
    let data = json!({
        "stopping": true, "processes": 2, "reason": "explicit",
        "stopped": [{"lane": "acme@0", "unit": "api"}, {"lane": "acme@0", "unit": "web"}],
        "tasks": [
            {"lane": "acme@0", "task": "flush", "exit_code": 0},
            {"lane": "acme@0", "task": "notify", "exit_code": 3},
            {"lane": "acme@1", "task": "slow", "error": {"code": "cancelled", "data": null}},
        ],
    });
    let text = render_text(Method::DaemonShutdown, &data);
    assert!(text.contains("stopped the daemon and 2 units"), "{text}");
    assert!(text.contains("ran 3 lane.stop tasks"), "{text}");
    assert!(text.contains("acme@0 notify: exit 3"), "{text}");
    assert!(text.contains("acme@1 slow: cancelled"), "{text}");
    assert!(!text.contains("flush"), "{text}");
    let idle = json!({"stopping": true, "processes": 0, "stopped": [], "tasks": []});
    let text = render_text(Method::DaemonShutdown, &idle);
    assert!(text.contains("stopped the daemon\n"), "{text}");
    assert!(!text.contains("lane.stop"), "{text}");
}

#[test]
/// A stop timeout that does not fit in the service manager's stop window is a
/// warning naming the unit and the fix; doctor still exits 0.
fn doctor_warns_about_stop_timeouts_past_the_service_window() {
    let service = |installed: bool, window: u64| {
        json!({
            "manager": "launchd agent", "path": "/u/Library/LaunchAgents/run.devlanes.daemon.plist",
            "installed": installed, "stop_window_s": window,
            "window_source": if window == 60 { "installed" } else { "default" },
            "recommended_window_s": 60,
        })
    };
    let timeouts = json!([
        {"project": "acme", "unit": "api", "timeout_ms": 10_000},
        {"project": "acme", "unit": "db", "timeout_ms": 90_000},
    ]);
    // `running: false` keeps the daemon's own checks (schema) out of the exit code.
    let data = json!({"running": false, "service": service(true, 60), "stop_timeouts": timeouts});
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(text.contains("the daemon gets 60s to stop"), "{text}");
    assert!(
        text.contains(
            "1 unit can take too long to stop for the 60s the launchd agent gives the daemon\n"
        ),
        "{text}"
    );
    assert!(text.contains("unit: acme:db (90s)\n"), "{text}");
    assert!(
        text.contains("lower services.<name>.stop.timeout to 55s or less"),
        "{text}"
    );
    assert!(!text.contains("acme:api"), "{text}");

    let data = json!({"service": service(false, 5), "stop_timeouts": timeouts});
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains("2 units can take too long to stop for the 5s a launchd agent gets by default (devlanes is not installed as a service)\n"),
        "{text}"
    );
    assert!(
        text.contains("units: acme:api (10s), acme:db (90s)\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "devlanes daemon install, and lower services.<name>.stop.timeout to 55s or less"
        ),
        "{text}"
    );

    let data = json!({"service": service(true, 5), "stop_timeouts": [timeouts[0].clone()]});
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains("gives the daemon 5s to stop, its default; devlanes now installs 60s"),
        "{text}"
    );
    assert!(
        text.contains(
            "1 unit can take too long to stop for the 5s the launchd agent gives the daemon\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("devlanes daemon install (gives the daemon 60s to stop)"),
        "{text}"
    );

    // More units than a detail line names are counted.
    let many: Vec<Value> = (0..7)
        .map(|n| json!({"project": "acme", "unit": format!("u{n}"), "timeout_ms": 10_000}))
        .collect();
    let data = json!({"service": service(false, 5), "stop_timeouts": many});
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains(
            "units: acme:u0 (10s), acme:u1 (10s), acme:u2 (10s), acme:u3 (10s), acme:u4 (10s), 2 more\n"
        ),
        "{text}"
    );
}

/// `doctor` text of `data` in one output mode.
fn doctor_in(mode: OutputMode, data: &Value) -> String {
    runtime(mode)
        .reporter()
        .result(&Report::new(Method::Doctor, data))
        .expect("render")
        .stdout
}

#[test]
/// A check's detail is its own line between the text and the fix, indented like
/// the fix in every mode; checks without a detail print no extra line.
fn doctor_prints_the_detail_line_before_the_fix() {
    let data = json!({
        "running": false,
        "service": {
            "manager": "launchd agent", "path": "/u/Library/LaunchAgents/run.devlanes.daemon.plist",
            "installed": true, "stop_window_s": 60, "window_source": "installed",
            "recommended_window_s": 60,
        },
        "stop_timeouts": [{"project": "acme", "unit": "db", "timeout_ms": 90_000}],
    });
    let fix = "fix: lower services.<name>.stop.timeout to 55s or less";

    let agent = doctor_in(OutputMode::Agent, &data);
    assert!(
        agent.contains(&format!(
            "warn service: 1 unit can take too long to stop for the 60s the launchd agent gives the daemon\n  unit: acme:db (90s)\n  {fix}\n"
        )),
        "{agent}"
    );

    for mode in [OutputMode::Human, OutputMode::Plain] {
        let text = doctor_in(mode, &data);
        let lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .position(|line| {
                line.ends_with(
                    "1 unit can take too long to stop for the 60s the launchd agent gives the daemon",
                )
            })
            .unwrap_or_else(|| panic!("{text}"));
        assert_eq!(lines[at + 1], "    unit: acme:db (90s)", "{text}");
        assert_eq!(lines[at + 2], format!("    {fix}"), "{text}");
    }

    for text in [
        agent,
        doctor_in(OutputMode::Human, &data),
        doctor_in(OutputMode::Plain, &data),
    ] {
        let lines: Vec<&str> = text.lines().collect();
        // A fix without a detail follows its check directly.
        let daemon = lines
            .iter()
            .position(|line| line.contains("not running"))
            .unwrap_or_else(|| panic!("{text}"));
        assert_eq!(
            lines[daemon + 1].trim(),
            "fix: devlanes daemon start",
            "{text}"
        );
        // A check with neither is followed by the next check or group.
        let ok = lines
            .iter()
            .position(|line| line.ends_with("the daemon gets 60s to stop"))
            .unwrap_or_else(|| panic!("{text}"));
        let next = lines[ok + 1].trim_start();
        assert!(
            !next.starts_with("unit:") && !next.starts_with("fix:"),
            "{text}"
        );
        assert_eq!(text.matches("unit: ").count(), 1, "{text}");
    }
}

#[test]
/// The Windows logon start has no stop window: it is an ok line while on, a
/// warning with the install fix when Task Manager turned it off or it runs
/// another binary, and nothing when not installed. Stop timeouts are not
/// compared with any window.
fn doctor_words_the_logon_start() {
    let path = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\devlanes";
    let command = r#""C:\tools\devlanes.exe" daemon run"#;
    let mut data = json!({
        "running": false,
        "service": {"manager": "logon Run entry", "path": path, "installed": true,
                    "enabled": true, "command": command, "current": true},
        "stop_timeouts": [{"project": "acme", "unit": "db", "timeout_ms": 90_000}],
    });
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        text.contains(&format!(
            "logon Run entry at {path}; the daemon starts at logon"
        )),
        "{text}"
    );
    assert!(!text.contains("acme:db"), "{text}");

    data["service"]["enabled"] = json!(false);
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("--- plain (exit 0)"), "{text}");
    assert!(
        text.contains("is turned off in Task Manager's Startup apps"),
        "{text}"
    );
    assert!(text.contains("fix: devlanes daemon install"), "{text}");

    data["service"]["enabled"] = json!(true);
    data["service"]["current"] = json!(false);
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains(&format!("runs another devlanes: {command}")),
        "{text}"
    );

    data["service"] = json!({"manager": "logon Run entry", "path": path, "installed": false,
                             "enabled": true, "command": null, "current": false});
    let text = render_text(Method::Doctor, &data);
    assert!(!text.contains("logon Run entry"), "{text}");
}

#[test]
/// On Windows a process ended by a console Ctrl-C or Ctrl-Break
/// (`STATUS_CONTROL_C_EXIT`) reads as interrupted; elsewhere every code stays
/// a number.
fn ctrl_c_exits_read_as_interrupted() {
    let code = i64::from(0xC000_013A_u32.cast_signed());
    let exited = RpcError::new(
        "exited",
        json!({"lane": "acme@1", "unit": "web", "code": code}),
        1,
    );
    let task = RpcError::new(
        "task_failed",
        json!({"lane": "acme@1", "task": "seed", "exit_code": code}),
        1,
    );
    let render = |error: &RpcError| {
        runtime(OutputMode::Plain)
            .reporter()
            .error(error, None)
            .stderr
    };
    if cfg!(windows) {
        assert_eq!(
            exit::words(code),
            Some("interrupted by Ctrl-C or Ctrl-Break")
        );
        assert_eq!(
            exit::words(code + (1 << 32)),
            Some("interrupted by Ctrl-C or Ctrl-Break")
        );
        assert!(
            render(&exited).contains("interrupted by Ctrl-C or Ctrl-Break"),
            "{}",
            render(&exited)
        );
        assert!(
            render(&task).contains("task seed was interrupted by Ctrl-C or Ctrl-Break"),
            "{}",
            render(&task)
        );
    } else {
        assert_eq!(exit::words(code), None);
        assert!(
            render(&exited).contains(&format!("exited with code {code}")),
            "{}",
            render(&exited)
        );
    }
    for other in [0, 1, 130, 255] {
        assert_eq!(exit::words(other), None);
    }
}

#[test]
/// A human terminal with colors gets a unit's colors back, each line ending in a
/// reset and nothing but SGR kept; every other output gets plain text.
fn logs_keep_colors_only_where_they_show() {
    let line = json!({"unit": "api", "seq": 7, "timestamp_ms": 1, "level": "info",
        "text": "INFO ready", "styled": "\u{1b}[32mINFO\u{1b}[0m \u{1b}[2Kready\u{1b}[0m"});
    let page = json!({"items": [line], "truncated": false, "next_cursor": null});
    let text = colored_runtime()
        .reporter()
        .result(&Report::new(Method::Logs, &page))
        .expect("render")
        .stdout;
    assert_eq!(text, "api#7 \u{1b}[32mINFO\u{1b}[0m ready\u{1b}[0m\n");
    for mode in [OutputMode::Human, OutputMode::Plain, OutputMode::Agent] {
        let text = runtime(mode)
            .reporter()
            .result(&Report::new(Method::Logs, &page))
            .expect("render")
            .stdout;
        assert!(!text.contains('\u{1b}'), "{mode:?}: {text:?}");
        assert!(text.contains("api#7 INFO ready"), "{mode:?}: {text:?}");
    }
}

/// A human runtime on a terminal, with colors.
fn colored_runtime() -> Runtime {
    let cli = Cli::parse_from(["devlanes"]);
    Runtime::resolve(
        &cli,
        Signals {
            env_output: Some("human".into()),
            stdout_tty: true,
            stdin_tty: true,
            ..Signals::default()
        },
    )
    .expect("runtime")
}

#[test]
/// Failure tails of `start` and `wait` show the unit's colors on a colored
/// terminal, and plain text everywhere else.
fn failure_tails_keep_colors_only_where_they_show() {
    let styled = "\u{1b}[31mError\u{1b}[0m: boom\u{1b}[0m";
    let start = json!({
        "lane": "acme@0", "ready": false, "started": ["api"],
        "failures": [{"lane": "acme@0", "unit": "api", "exit_code": 1,
            "log_tail": ["Error: boom"], "log_tail_styled": [styled]}],
    });
    let wait = json!({
        "lane": "acme@0", "unit": "api", "state": "failed", "ready": false, "failed": ["api"],
        "exit_code": 1, "log_tail": ["Error: boom"], "log_tail_styled": [styled],
    });
    for (method, data) in [(Method::Start, &start), (Method::Wait, &wait)] {
        let text = colored_runtime()
            .reporter()
            .result(&Report::new(method, data))
            .expect("render")
            .stdout;
        assert!(text.contains(styled), "{method:?}: {text:?}");
        for mode in [OutputMode::Human, OutputMode::Plain, OutputMode::Agent] {
            let text = runtime(mode)
                .reporter()
                .result(&Report::new(method, data))
                .expect("render")
                .stdout;
            assert!(!text.contains('\u{1b}'), "{method:?} {mode:?}: {text:?}");
            assert!(
                text.contains("Error: boom"),
                "{method:?} {mode:?}: {text:?}"
            );
        }
    }
}

/// Splits [`render_text`] output into its human, plain and agent parts.
fn modes(text: &str) -> Vec<&str> {
    text.split("--- ").filter(|part| !part.is_empty()).collect()
}

#[test]
/// People see the display name and `running` for a unit without a readiness
/// check; plain and agent output keep the key and the state word.
fn human_status_shows_display_names_and_running() {
    let mut api = unit("api", "ready");
    api["display_name"] = json!("Billing API");
    api["ready_check"] = json!("started");
    let mut web = unit("web", "ready");
    web["ready_check"] = json!("tcp");
    let data = json!({"items": [lane(Some(2), "feat/billing", "running", vec![api, web])],
                      "truncated": false, "next_cursor": null});
    let text = render_text(Method::Status, &data);
    let parts = modes(&text);
    let human = parts[0];
    assert!(
        human.contains("Billing API") && human.contains("running"),
        "{human}"
    );
    assert!(!human.contains(" api "), "{human}");
    for other in &parts[1..] {
        assert!(!other.contains("Billing API"), "{other}");
    }
}

#[test]
/// An idle lane's status says nothing runs and names the start command; agents
/// already read the `idle` word.
fn status_of_idle_lanes_says_nothing_runs() {
    let one = json!({"items": [lane(Some(1), "main", "idle", vec![unit("api", "idle")])]});
    let rendered = render_text(Method::Status, &one);
    let parts = modes(&rendered);
    for text in &parts[..2] {
        assert!(
            text.contains("not running; next: devlanes start --lane acme@1"),
            "{text}"
        );
    }
    assert!(!parts[2].contains("next:"), "{}", parts[2]);
    let two = json!({"items": [
        lane(Some(1), "main", "idle", vec![]),
        lane(Some(2), "feat/billing", "idle", vec![]),
    ]});
    assert!(render_text(Method::Status, &two).contains("nothing is running"));
    let mut shell = lane(Some(1), "main", "idle", vec![]);
    shell["needs_env"] = json!(true);
    let text = render_text(Method::Status, &json!({"items": [shell]}));
    assert!(!text.contains("not running"), "{text}");
    let running = json!({"items": [lane(Some(1), "main", "running", vec![unit("api", "ready")])]});
    assert!(!render_text(Method::Status, &running).contains("not running"));
}

#[test]
/// A start that found everything up says so instead of looking like a fresh start.
fn start_of_a_running_lane_says_already_running() {
    let data = json!({
        "lane": "acme@2", "branch": "feat/billing", "started": [],
        "already_running": ["acme@2:api"], "whole_lane": false, "ready": true, "timed_out": false,
        "failures": [], "skipped_tasks": [], "units": [unit("api", "ready")],
    });
    let text = render_text(Method::Start, &data);
    let parts = modes(&text);
    assert!(
        parts[0].contains("already running; nothing was started"),
        "{text}"
    );
    assert!(
        parts[2].contains("already running (nothing started)"),
        "{text}"
    );
}

#[test]
fn doctor_says_how_to_recreate_an_unconstrained_ca() {
    let data = json!({
        "ca": {
            "present": true, "kind": "devlanes", "path": "/home/u/.devlanes/ca/ca.pem",
            "name_constrained": false,
            "hint": "the devlanes CA has no name constraints",
            "fix": "devlanes certs untrust && rm -r /home/u/.devlanes/ca && devlanes certs trust && devlanes daemon restart",
        },
    });
    let text = render_text(Method::Doctor, &data);
    assert!(
        text.contains("the devlanes CA has no name constraints"),
        "{text}"
    );
    assert!(text.contains("rm -r /home/u/.devlanes/ca"), "{text}");
    let constrained = json!({"ca": {"present": true, "path": "/x/ca.pem", "name_constrained": true, "hint": null}});
    assert!(!render_text(Method::Doctor, &constrained).contains("name constraints"));
}

#[test]
fn doctor_shows_release_facts_and_the_upgrade_command() {
    let release = |update_available: bool, daemon: &str| {
        json!({"release": {
            "version": "0.2.1", "build": "v0.2.1", "daemon_version": daemon,
            "install_method": "Homebrew", "command": "brew upgrade devlanes-run/tap/devlanes",
            "check": "ok", "check_off_reason": null, "latest": "0.3.0",
            "checked_at": null, "update_available": update_available,
        }})
    };
    let text = render_text(Method::Doctor, &release(true, "0.2.1"));
    assert!(
        text.contains("devlanes 0.2.1 (release build v0.2.1), installed with Homebrew"),
        "{text}"
    );
    assert!(text.contains("devlanes 0.3.0 is available"), "{text}");
    assert!(
        text.contains("fix: brew upgrade devlanes-run/tap/devlanes"),
        "{text}"
    );
    assert!(!text.contains("the daemon runs"), "{text}");

    let text = render_text(Method::Doctor, &release(false, "0.2.0"));
    assert!(text.contains("latest release 0.3.0"), "{text}");
    assert!(
        text.contains("the daemon runs 0.2.0, this CLI is 0.2.1"),
        "{text}"
    );
    assert!(
        text.contains("devlanes daemon restart when convenient (it stops services)"),
        "{text}"
    );

    let off = json!({"release": {"version": "0.2.1", "build": "local", "install_method": "install.sh",
        "check": "off", "check_off_reason": "config"}});
    let text = render_text(Method::Doctor, &off);
    assert!(text.contains("(local build)"), "{text}");
    assert!(
        text.contains("release checks are off (updates.check: false)"),
        "{text}"
    );
}

/// Windows doctor deletes `devlanes.exe.old-*` copies and states what it did.
#[test]
fn doctor_reports_old_executables() {
    let copies = json!({
        "removed": ["C:/p/devlanes.exe.old-1", "C:/p/devlanes.exe.old-2"],
        "in_use": ["C:/p/devlanes.exe.old-3"],
    });
    let release = json!({"release": {"version": "0.2.1", "build": "local",
        "install_method": "install.ps1", "check": "off", "check_off_reason": "config",
        "old_copies": copies}});
    let text = render_text(Method::Doctor, &release);
    assert!(
        text.contains("deleted 2 old executables left by an upgrade"),
        "{text}"
    );
    assert!(
        text.contains("old executable C:/p/devlanes.exe.old-3 still runs; doctor deletes it after devlanes daemon restart"),
        "{text}"
    );
    assert!(!text.contains("fail release"), "{text}");
}

#[test]
fn leftover_processes_get_a_one_line_note() {
    let pg = json!([{"pid": 4242, "command": "pg_ctl"}]);
    let run = json!({
        "task": "boot", "lane": "acme@2", "unit": null, "interactive": false,
        "exit_code": 0, "state": "exited",
        "leftovers": [{"unit": "db", "left_running": pg}, {"unit": "boot", "escaped": [{"pid": 51, "command": "node"}]}],
    });
    let text = render_text(Method::Run, &run);
    assert!(
        text.contains("  db: left running: pg_ctl (pid 4242)"),
        "{text}"
    );
    assert!(text.contains("  escaped: node (pid 51)"), "{text}");

    let mut start = start_result();
    start["leftovers"] = json!([{"unit": "setup.db", "left_running": pg}]);
    let text = render_text(Method::Start, &start);
    assert_eq!(
        text.matches("setup.db: left running: pg_ctl (pid 4242)")
            .count(),
        3,
        "{text}"
    );

    let mut task = unit("db-start", "exited");
    task["kind"] = json!("task");
    task["left_running"] = pg.clone();
    let mut status = lane(
        Some(2),
        "feat/billing",
        "running",
        vec![unit("api", "ready"), task],
    );
    status["containers"] = json!([
        {"name": "postgres", "ports": {"default": 20150}, "cpu_percent": 1.5, "memory_bytes": 52_000_000},
        {"name": "redis", "ports": {}, "shared": true, "cpu_percent": 0.5, "memory_bytes": 8_000_000},
        {"name": "s3mock", "ports": {"default": 19090}, "shared": true, "cpu_percent": 0.1,
         "memory_bytes": 9_000_000, "urls": ["https://s3.acme-2.localhost"]},
        {"name": "mail", "ports": {}},
    ]);
    let text = render_text(Method::Status, &json!({"items": [status]}));
    assert!(
        text.contains("db-start: left running: pg_ctl (pid 4242)"),
        "{text}"
    );
    let human = text.split("--- plain").next().unwrap_or_default();
    assert!(human.contains("postgres  container"), "{human}");
    assert!(human.contains("redis     shared container"), "{human}");
    // A container's `infra.hosts` URL fills the URL column like a service's.
    let s3 = human
        .lines()
        .find(|line| line.contains("s3mock"))
        .unwrap_or_default();
    assert!(s3.contains("https://s3.acme-2.localhost"), "{human}");
    assert!(
        !human.contains("mail"),
        "unsampled containers stay out\n{human}"
    );

    let why = json!({
        "lane": "acme@2", "unit": "db-start", "state": "stopped", "reasons": [],
        "left_running": pg,
    });
    let text = render_text(Method::Why, &why);
    assert!(text.contains("  left running: pg_ctl (pid 4242)"), "{text}");
}

#[test]
fn agent_status_lines_start_with_a_stable_lowercase_word() {
    let words = [
        "ready",
        "failed",
        "stopped",
        "starting",
        "idle",
        "running",
        "unassigned",
        "queued",
        "exited",
        "started",
        "created",
        "cleaned",
        "warn",
        "next:",
    ];
    for (name, method, data) in fixtures() {
        if !matches!(
            method,
            Method::Start
                | Method::Stop
                | Method::Wait
                | Method::Errors
                | Method::Why
                | Method::Exec
        ) {
            continue;
        }
        let rendered = runtime(OutputMode::Agent)
            .reporter()
            .result(&Report::new(method, &data))
            .expect("render");
        // Indented detail lines and "no ..." notices are not status lines.
        for line in rendered
            .stdout
            .lines()
            .filter(|line| !line.starts_with(' ') && !line.starts_with("no "))
        {
            let first = line.split(' ').next().unwrap_or_default();
            assert!(
                words.contains(&first) || line.starts_with("acme@"),
                "{name}: {line:?} does not start with a stable word"
            );
        }
    }
}

#[test]
fn human_status_colors_unit_names_by_tag_and_failures_red() {
    let cli = Cli::parse_from(["devlanes"]);
    let colored = Runtime::resolve(
        &cli,
        Signals {
            env_output: Some("human".into()),
            stdout_tty: true,
            ..Signals::default()
        },
    )
    .expect("runtime");
    let mut api = unit("api", "ready");
    api["color"] = json!("blue");
    let mut admin = unit("admin", "failed");
    admin["color"] = json!("green");
    let data = json!({"items": [lane(Some(2), "feat/billing", "failed", vec![api, admin])],
        "truncated": false, "next_cursor": null});
    let human = colored
        .reporter()
        .result(&Report::new(Method::Status, &data))
        .expect("render")
        .stdout;
    assert!(human.contains("\x1b[34mapi\x1b[0m"), "{human:?}");
    assert!(human.contains("\x1b[31madmin\x1b[0m"), "{human:?}");
    let plain = runtime(OutputMode::Plain)
        .reporter()
        .result(&Report::new(Method::Status, &data))
        .expect("render")
        .stdout;
    assert!(!plain.contains('\x1b'));
}

/// Frozen and throttled are quiet states: dim, never the warning yellow.
#[test]
fn human_status_dims_idle_units_and_lanes() {
    let cli = Cli::parse_from(["devlanes"]);
    let colored = Runtime::resolve(
        &cli,
        Signals {
            env_output: Some("human".into()),
            stdout_tty: true,
            ..Signals::default()
        },
    )
    .expect("runtime");
    let mut api = unit("api", "ready");
    api["power"] = json!("frozen");
    let mut worker = unit("worker", "ready");
    worker["power"] = json!("throttled");
    let mut idle = lane(Some(2), "feat/billing", "running", vec![api, worker]);
    idle["power"] = json!({"level": "frozen", "since_ms": 0});
    let human = colored
        .reporter()
        .result(&Report::new(Method::Status, &json!({"items": [idle]})))
        .expect("render")
        .stdout;
    assert!(human.contains("\x1b[2mfrozen\x1b[0m"), "{human:?}");
    assert!(human.contains("\x1b[2mthrottled\x1b[0m"), "{human:?}");
    assert!(
        human.contains("\x1b[32mrunning\x1b[0m \x1b[2mfrozen "),
        "{human:?}"
    );
    assert!(!human.contains("\x1b[33m"), "{human:?}");
}

#[test]
fn agent_output_stays_within_the_budget() {
    let units: Vec<Value> = (0..600)
        .map(|i| unit(&format!("svc-{i}"), "ready"))
        .collect();
    let data = json!({"items": [lane(Some(2), "feat/billing", "running", units)], "truncated": false, "next_cursor": null});
    let rendered = runtime(OutputMode::Agent)
        .reporter()
        .result(&Report::new(Method::Status, &data))
        .expect("render");
    assert!(
        rendered.stdout.len() <= DEFAULT_BUDGET,
        "{}",
        rendered.stdout.len()
    );
    assert!(rendered.stdout.contains("truncated; more: devlanes status"));
}

#[test]
/// A config bigger than the budget ends with the command that prints all of it,
/// for the same lane when the caller passes it.
fn agent_debug_config_is_cut_with_a_continuation() {
    let services: serde_json::Map<String, Value> = (0..1000)
        .map(|i| {
            (
                format!("svc-{i}"),
                json!({"run": format!("serve --port {i}")}),
            )
        })
        .collect();
    let data = json!({"project": "acme", "lane": "acme@2", "files": [], "config": {"project": "acme", "services": services}});
    let reporter = runtime(OutputMode::Agent).reporter();
    let rendered = reporter
        .result(&Report::new(Method::ConfigShow, &data))
        .expect("render");
    assert!(
        rendered.stdout.len() <= DEFAULT_BUDGET,
        "{}",
        rendered.stdout.len()
    );
    assert!(
        rendered
            .stdout
            .ends_with("truncated; more: devlanes debug config --budget 1048576\n"),
        "{}",
        rendered.stdout
    );
    let more = Hint::command(["debug", "config", "--budget", "1048576", "--lane", "acme@2"]);
    let rendered = reporter
        .result(&Report::new(Method::ConfigShow, &data).with_more(more))
        .expect("render");
    assert!(
        rendered
            .stdout
            .ends_with("truncated; more: devlanes debug config --budget 1048576 --lane acme@2\n"),
        "{}",
        rendered.stdout
    );
}

#[test]
fn start_progress_prints_finished_steps_once() {
    let mut output = String::new();
    let reporter = runtime(OutputMode::Plain).reporter();
    for (event, data) in [
        (
            "lane.phase",
            json!({"lane": "acme@2", "phase": "infra", "state": "started"}),
        ),
        (
            "lane.phase",
            json!({"lane": "acme@2", "phase": "infra", "state": "done"}),
        ),
        (
            "lane.phase",
            json!({"lane": "acme@2", "phase": "setup", "state": "skipped", "reason": {"code": "no_setup", "data": {}}}),
        ),
        (
            "task",
            json!({"lane": "acme@2", "task": "migrate", "state": "done", "exit_code": 0}),
        ),
        ("unit.changed", json!({"unit": unit("api", "ready")})),
        (
            "lane.phase",
            json!({"lane": "acme@2", "phase": "services", "state": "failed", "reason": {"code": "unit_failed", "data": {"lane": "acme@2", "unit": "admin"}}}),
        ),
    ] {
        let event = Event {
            seq: 1,
            event: event.into(),
            data,
        };
        output.push_str(&reporter.event(Method::Start, &event).expect("event").stdout);
    }
    insta::assert_snapshot!("render_start_progress", output);
    assert!(
        runtime(OutputMode::Agent)
            .reporter()
            .event(
                Method::Start,
                &Event {
                    seq: 1,
                    event: "lane.phase".into(),
                    data: json!({"lane": "acme@2", "phase": "infra", "state": "done"})
                }
            )
            .expect("event")
            .stdout
            .is_empty()
    );
}

/// A matching helper and a valid user config show as ok; a failed /etc/hosts write
/// that already blames the helper version is not repeated by the helper check.
#[test]
fn doctor_helper_and_user_config_checks() {
    let data = json!({
        "hosts": {"mode": "auto", "probe": "loopback", "state": "applied", "names": 2, "last_error": null,
                  "helper": {"state": "ok", "level": "ok", "installed": "0.2.0", "expected": "0.2.0"}},
        "user_config": {"path": "/home/u/.devlanes/config.yaml", "valid": true, "error": null, "warnings": []},
    });
    let text = render_text(Method::Doctor, &data);
    assert!(text.contains("ok hosts: hosts helper 0.2.0"), "{text}");
    assert!(
        text.contains("ok config: user config /home/u/.devlanes/config.yaml"),
        "{text}"
    );
    let failed = json!({"hosts": {"mode": "auto", "state": "failed",
        "last_error": {"code": "helper_version_mismatch", "message": "The installed hosts helper is version 0.1.0 and devlanes is 0.2.0."},
        "helper": {"state": "mismatch", "level": "warning", "installed": "0.1.0", "expected": "0.2.0"}}});
    let text = render_text(Method::Doctor, &failed);
    assert!(
        !text.contains("helper is version 0.1.0, devlanes"),
        "{text}"
    );
    assert!(text.contains("fix: devlanes certs trust --hosts"), "{text}");
}

/// The resolver strategy's DNS server and the OS resolver file, from `dns`.
#[test]
fn doctor_checks_the_dns_server_and_the_resolver_file() {
    let dns = |file: Value, running: bool| {
        json!({"dns": {"tld": "test", "listen": "127.0.0.1:15353", "running": running,
            "error": if running { Value::Null } else { json!("address in use") },
            "resolver_file": file}})
    };
    let ok = dns(
        json!({"path": "/etc/resolver/test", "installed": true, "matches": true}),
        true,
    );
    let text = render_text(Method::Doctor, &ok);
    assert!(
        text.contains(
            "ok dns: DNS server for .test on 127.0.0.1:15353, resolver file /etc/resolver/test"
        ),
        "{text}"
    );
    let missing = dns(
        json!({"path": "/etc/resolver/test", "installed": false, "matches": false}),
        true,
    );
    let text = render_text(Method::Doctor, &missing);
    assert!(
        text.contains(
            "warn dns: .test lookups do not reach the devlanes DNS server: nothing at /etc/resolver/test"
        ),
        "{text}"
    );
    assert!(
        text.contains("fix: devlanes certs trust --resolver"),
        "{text}"
    );
    let down = dns(Value::Null, false);
    let checks = doctor_checks_of(&down);
    let check = checks.iter().find(|c| c.group == "dns").expect("dns check");
    assert_eq!(check.state, DoctorState::Fail);
    assert_eq!(
        check.text,
        "DNS server for .test is not running on 127.0.0.1:15353: address in use"
    );
    // Without the resolver strategy there is nothing to say.
    assert!(
        !doctor_checks_of(&json!({}))
            .iter()
            .any(|c| c.group == "dns")
    );
    // Other clients get the same lines, minus this command's output mode.
    assert!(!checks.iter().any(|c| c.group == "output"));
}

/// Windows reports an NRPT rule (`kind: nrpt`) where Unix has a resolver
/// file, plus Group Policy rules and other rules covering the TLD.
#[test]
fn doctor_names_the_nrpt_rule_on_windows() {
    let key = r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig\devlanes-test";
    let dns = |file: Value| {
        json!({"dns": {"tld": "test", "listen": "127.53.0.1:53", "running": true,
            "error": null, "resolver_file": file}})
    };
    let rule = |state: &str, policy: bool, foreign: Value| {
        json!({"kind": "nrpt", "path": key, "state": state,
            "installed": state != "missing", "matches": state == "installed",
            "foreign": foreign, "group_policy": policy})
    };
    let lines = |file: Value| -> Vec<String> {
        doctor_checks_of(&dns(file))
            .into_iter()
            .filter(|check| check.group == "dns")
            .map(|check| check.text)
            .collect()
    };
    assert_eq!(
        lines(rule("missing", false, json!([]))),
        [format!(
            ".test lookups do not reach the devlanes DNS server: no NRPT rule at {key}"
        )]
    );
    assert_eq!(
        lines(rule("installed", false, json!([]))),
        [format!(
            "DNS server for .test on 127.53.0.1:53, NRPT rule {key}"
        )]
    );
    assert_eq!(
        lines(rule("outdated", false, json!([]))),
        [format!(
            "{key} sends .test lookups elsewhere, not to 127.53.0.1:53"
        )]
    );
    let blocked = lines(rule("installed", true, json!(["{guid}"])));
    assert_eq!(blocked.len(), 3, "{blocked:?}");
    assert_eq!(
        blocked[1],
        "Group Policy sets NRPT rules on this machine, so Windows ignores the devlanes rule; ask an administrator for a rule sending .test to 127.53.0.1"
    );
    assert_eq!(
        blocked[2],
        "another NRPT rule also covers .test ({guid}) and may take its lookups"
    );
    assert_eq!(
        lines(json!({"kind": "nrpt", "error": "cannot read the NRPT: access denied"})),
        ["cannot check the NRPT rule for .test: cannot read the NRPT: access denied"]
    );
}

/// Windows writes no hosts file: doctor shows the `*.localhost` probe instead
/// of the helper.
#[test]
fn doctor_shows_the_windows_localhost_probe() {
    let hosts = |mode: &str, probe: Value| {
        let data = json!({"hosts": {"mode": mode, "probe": probe, "state": "native",
            "names": null, "desired": 0, "skipped": 0, "last_error": null,
            "tld_error": null, "helper": null}});
        doctor_checks_of(&data)
            .into_iter()
            .filter(|check| check.group == "hosts")
            .collect::<Vec<_>>()
    };
    let ok = hosts("auto", json!("loopback"));
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0].state, DoctorState::Ok);
    assert_eq!(
        ok[0].text,
        "*.localhost resolves to loopback; no hosts file entries needed"
    );
    let never = hosts("never", Value::Null);
    assert_eq!(
        never[0].text,
        "hosts file fallback never; devlanes writes no hosts file entries on Windows"
    );
    let failed = hosts("always", json!("fails"));
    assert_eq!(failed[0].state, DoctorState::Warn);
    assert!(
        failed[0].text.contains("failed at daemon start"),
        "{}",
        failed[0].text
    );
}

/// `lane.warning` events are one line in every mode; outside a start they name the lane.
#[test]
fn lane_warning_events_are_one_line() {
    let event = Event {
        seq: 1,
        event: "lane.warning".into(),
        data: json!({"lane": "acme@2", "code": "host_outside_ca", "data": {
            "lane": "acme@2", "unit": "api", "host": "api.example.com",
            "permitted": ["localhost"], "suggestion": "api.acme.test",
        }}),
    };
    let mut output = String::new();
    for mode in [OutputMode::Human, OutputMode::Plain, OutputMode::Agent] {
        for method in [Method::Start, Method::Subscribe] {
            output.push_str(
                &runtime(mode)
                    .reporter()
                    .event(method, &event)
                    .expect("event")
                    .stdout,
            );
        }
    }
    insta::assert_snapshot!(output, @r"
    ! api.example.com is outside the CA's names; HTTPS will fail; try api.acme.test
    ! acme@2: api.example.com is outside the CA's names; HTTPS will fail; try api.acme.test
    warn api.example.com is outside the CA's names; HTTPS will fail; try api.acme.test
    warn acme@2: api.example.com is outside the CA's names; HTTPS will fail; try api.acme.test
    warn acme@2: api.example.com is outside the CA's names; HTTPS will fail; try api.acme.test
    ");
    // Agents see a start's warnings once, in its result.
}

#[test]
fn daemon_errors_are_worded_with_hints() {
    let errors = [
        RpcError::new(
            "not_found",
            json!({"reference": "acme@9", "candidates": ["acme@0", "acme@2"]}),
            3,
        ),
        RpcError::new(
            "ambiguous",
            json!({"reference": "feat", "candidates": ["acme@feat", "shop@feat"]}),
            3,
        ),
        RpcError::new(
            "confirmation_required",
            json!({"lane": "acme@2", "flag": "--yes"}),
            2,
        ),
        RpcError::new(
            "confirmation_required",
            json!({"task": "reset", "prompt": "Drop the database?", "flag": "--yes"}),
            2,
        ),
        RpcError::new("terminal_required", json!({"task": "console"}), 2),
        RpcError::new(
            "project_name_taken",
            json!({"project": "acme", "other": "/work/other-acme", "message": "project `acme` is already used by /work/other-acme; give this one another `project:`"}),
            1,
        ),
        RpcError::new(
            "config_names_other_project",
            json!({"project": "acme-next", "message": "the project file here now names project `acme-next`; this lane keeps its ports and data until `devlanes lane gc --yes` collects it"}),
            1,
        ),
        RpcError::new(
            "config_names_other_project",
            json!({"project": "acme-next", "lane": "acme@2", "message": "the project file now names `acme-next`"}),
            1,
        ),
        RpcError::new(
            "duplicate_project_name",
            json!({"project": "acme", "file": "devlanes.prod.yaml", "other": "devlanes.yaml", "message": "both files name `acme`"}),
            1,
        ),
        // An unknown `-p`: the daemon only marks it with `hint`; its prose stays out.
        RpcError::new(
            "not_found",
            json!({"reference": "acme-prod", "candidates": [], "hint": "name the file with --file first"}),
            3,
        ),
        RpcError::new(
            "port_in_use_by_lane",
            json!({"port": 5173, "lane": "shop@1", "unit": "web"}),
            1,
        ),
        RpcError::new(
            "docker_unavailable",
            json!({"message": "no Docker engine is reachable"}),
            1,
        ),
        RpcError::new("timeout", json!({"lane": "acme@2", "unit": "api"}), 124),
        RpcError::new("not_supported_yet", json!({"feature": "remote unlock"}), 1),
        RpcError::new(
            "config_error",
            json!({"message": "unknown field `serivces`", "file": "devlanes.yaml", "line": 2, "column": 1}),
            1,
        ),
        RpcError::new("needs_env", json!({"lane": "acme@2"}), 1),
        RpcError::new(
            "protocol_mismatch",
            json!({"protocol": 7, "client_protocol": 8, "version": "0.2.0", "build_id": "a1b2c3", "managed_processes": 6}),
            4,
        ),
        RpcError::new(
            "protocol_mismatch",
            json!({"protocol": 7, "client_protocol": 8, "version": "0.2.0", "build_id": "a1b2c3", "managed_processes": 0}),
            4,
        ),
        RpcError::new("start_cancelled", json!({"lane": "acme@2"}), 1),
        RpcError::new(
            "no_config",
            json!({"path": "/work/acme/src", "searched_to": "/work/acme"}),
            3,
        ),
        RpcError::new(
            "unknown_profile",
            json!({"profile": "e2r", "candidates": ["dev", "e2e", "full"]}),
            2,
        ),
        RpcError::new(
            "unknown_profile",
            json!({"profile": "staging", "candidates": ["dev", "e2e"]}),
            2,
        ),
        RpcError::new("port_error", json!({"unit": "api", "port": "metrics"}), 1),
        RpcError::new(
            "unit_running",
            json!({"lane": "acme@2", "unit": "probe"}),
            1,
        ),
        RpcError::new("worktree_missing", json!({"path": "/work/gone"}), 3),
        RpcError::new(
            "phase_failed",
            json!({"lane": "acme@2", "phase": "setup", "error": {"code": "task_failed", "data": {"task": "migrate", "exit_code": 2}}}),
            1,
        ),
        RpcError::new(
            "start_failed",
            json!({"lane": "acme@2", "units": ["acme@2:api"]}),
            1,
        ),
        RpcError::new(
            "unsafe_home",
            json!({"path": "/home/u/.devlanes", "problem": "mode", "mode": "755"}),
            4,
        ),
        RpcError::new(
            "spawn_failed",
            json!({"program": "missing-cmd", "message": "No such file or directory (os error 2)"}),
            127,
        ),
        RpcError::new(
            "lane_running",
            json!({"lane": "acme@2", "units": ["api", "worker"]}),
            1,
        ),
        RpcError::new(
            "daemon_stopping",
            json!({"message": "the daemon is shutting down; retry once it is gone"}),
            4,
        ),
        RpcError::new(
            "daemon_start_stalled",
            json!({"step": "proxy", "log": "/home/u/.devlanes/logs/daemon.log", "waited_s": 30}),
            4,
        ),
        RpcError::new(
            "daemon_start_stalled",
            json!({"log": "/home/u/.devlanes/logs/daemon.log", "waited_s": 30}),
            4,
        ),
        RpcError::new(
            "name_taken",
            json!({"name": "api", "kind": "service", "lane": "acme@2"}),
            2,
        ),
        RpcError::new(
            "name_taken",
            json!({"name": "seed", "kind": "task", "lane": "acme@2"}),
            2,
        ),
        RpcError::new(
            "name_taken",
            json!({"name": "pgadmin", "kind": "external", "lane": "acme@2"}),
            2,
        ),
        RpcError::new(
            "external_unit",
            json!({"unit": "pgadmin", "lane": "acme@2"}),
            2,
        ),
        RpcError::new(
            "response_too_large",
            json!({"limit_bytes": 16_777_216, "message": "a message exceeded the 16 MiB frame limit"}),
            1,
        ),
        RpcError::new("client_disconnected", Value::Null, 4),
        RpcError::new("lane_not_running", json!({"lane": "acme@2"}), 1),
        RpcError::new("lane_busy", json!({"lane": "acme@2"}), 1),
        RpcError::new(
            "lane_keep_awake",
            json!({"lane": "acme@2", "units": ["worker", "queue"]}),
            1,
        ),
        RpcError::new(
            "lane_has_orphans",
            json!({"lane": "acme@2", "units": ["bg"]}),
            1,
        ),
        RpcError::new(
            "idle_failed",
            json!({"lane": "acme@2", "level": "frozen", "message": "api: operation not permitted"}),
            1,
        ),
        RpcError::new("throttle_unsupported", json!({"lane": "acme@2"}), 1),
        RpcError::new(
            "thaw_failed",
            json!({"lane": "acme@2", "level": "normal", "message": "api: no such process"}),
            1,
        ),
        RpcError::new(
            "helper_version_mismatch",
            json!({"message": "The installed hosts helper is version 0.1.0 and devlanes is 0.2.0. Run `devlanes certs trust --hosts` to reinstall it.",
                   "fix": "devlanes certs trust --hosts",
                   "helper": {"state": "mismatch", "level": "warning", "installed": "0.1.0", "expected": "0.2.0"}}),
            1,
        ),
    ];
    let reporter = runtime(OutputMode::Agent).reporter();
    let mut output = String::new();
    for error in &errors {
        let rendered = reporter.error(error, None);
        assert_eq!(rendered.exit_code, error.exit_code);
        assert!(rendered.stdout.is_empty());
        output.push_str(&rendered.stderr);
    }
    insta::assert_snapshot!("render_daemon_errors", output);
}

/// An http readiness timeout names the request and the last status the probe got
/// (only the status from daemons that send no `http_path`); a non-2xx answer without
/// a configured `status` gets the fix instead of the logs hint.
#[test]
fn http_ready_timeouts_name_the_last_answer() {
    let unit = json!({"lane": "acme@2", "unit": "web", "check": "http", "timeout_ms": 60000,
        "expected_port": 20100, "listening": [20100]});
    let with = |extra: Value| {
        let mut data = unit.clone();
        data.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        data
    };
    let errors = [
        RpcError::new(
            "ready_timeout",
            with(
                json!({"http_path": "/-/health", "http_host": "127.0.0.1:20100",
                "last_status": 404, "expected_status": null}),
            ),
            1,
        ),
        RpcError::new(
            "ready_timeout",
            with(json!({"last_status": 302, "expected_status": null})),
            1,
        ),
        RpcError::new(
            "ready_timeout",
            with(json!({"last_status": 404, "expected_status": 200})),
            1,
        ),
        RpcError::new("ready_timeout", with(json!({"expected_status": null})), 1),
        RpcError::new(
            "container_not_ready",
            json!({"container": "db", "check": "http", "timeout_ms": 30000,
                "http_path": "/", "http_host": "db.example.invalid",
                "last_status": 401, "expected_status": null}),
            1,
        ),
    ];
    let reporter = runtime(OutputMode::Agent).reporter();
    let mut output = String::new();
    for error in &errors {
        output.push_str(&reporter.error(error, None).stderr);
    }
    let why = json!({
        "lane": "acme@2", "unit": "web", "state": "failed",
        "reasons": [{"code": "ready_timeout", "data": with(json!({"last_status": 302, "expected_status": null}))}],
    });
    output.push_str(
        &runtime(OutputMode::Agent)
            .reporter()
            .result(&Report::new(Method::Why, &why))
            .expect("render")
            .stdout,
    );
    insta::assert_snapshot!(output, @r"
    error: acme@2:web wasn't ready in time; GET /-/health (Host: 127.0.0.1:20100) answered 404
    hint: use a `ready.http.path` that answers 2xx, set `ready.http.host` if the app routes by hostname, or set `ready.http.status: 404`
    error: acme@2:web wasn't ready in time; last answer 302
    hint: use a `ready.http.path` that answers 2xx, set `ready.http.host` if the app routes by hostname, or set `ready.http.status: 302`
    error: acme@2:web wasn't ready in time; last answer 404, expected 200
    hint: devlanes logs acme@2:web --tail 200
    error: acme@2:web wasn't ready in time
    hint: devlanes logs acme@2:web --tail 200
    error: container db did not pass its http check within 30s; GET / (Host: db.example.invalid) answered 401
    hint: use a `ready.http.path` that answers 2xx, set `ready.http.host` if the app routes by hostname, or set `ready.http.status: 401`
    failed acme@2:web
      did not become ready before the timeout; last answer 302 (use a `ready.http.path` that answers 2xx, set `ready.http.host` if the app routes by hostname, or set `ready.http.status: 302`)
    ");
}

/// `remote ls` shows a port's last failed open on one line and nothing for a
/// port without one; the `open_failed` warning names the port and unit.
#[test]
fn failed_opens_are_one_line() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .expect("clock");
    let hosts = json!({"items": [{
        "host": "hetzner", "state": "connected", "transport": "ssh",
        "ports": [
            {"port": 20101, "lane": "acme@0", "unit": "web", "kind": "service",
             "addresses": ["127.0.0.1"], "conflicts": [], "relay": null,
             "last_error": {"reason": "not_running", "at_ms": now,
                            "message": "the unit is not running on the box"}},
            {"port": 20102, "lane": "acme@0", "unit": "api", "kind": "service",
             "addresses": ["127.0.0.1"], "conflicts": [], "relay": null, "last_error": null},
        ],
    }], "truncated": false, "next_cursor": null});
    let text = render_text(Method::HostList, &hosts);
    assert!(
        text.contains("last_error: the unit is not running on the box (not_running, just now)"),
        "{text}"
    );
    assert_eq!(text.matches("last_error").count(), 3, "{text}");
    let event = Event {
        seq: 1,
        event: "remote.warning".into(),
        data: json!({"host": "hetzner", "code": "open_failed", "port": 20101, "lane": "acme@0",
            "unit": "web", "reason": "not_running",
            "message": "the unit is not running on the box"}),
    };
    let line = runtime(OutputMode::Agent)
        .reporter()
        .event(Method::Subscribe, &event)
        .expect("event")
        .stdout;
    assert_eq!(
        line,
        "warn hetzner: a connection to port 20101 of acme@0:web failed because the unit is not running on the box\n"
    );
}

/// `remote ls` leaves what the TUI reads (the box's registry, handshake
/// info, proxy ports and name suffixes) to `--json`; a route shows the URL
/// that opens it here, and the connection its age.
#[test]
fn box_registries_stay_out_of_remote_ls() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .expect("clock");
    let hosts = json!({"items": [{
        "host": "hetzner", "state": "connected", "transport": "ssh", "ports": [],
        "connected_since_ms": now - 5 * 60_000,
        "routes": [{"host": "web.acme-1.localhost", "port": 20101, "lane": "acme@1",
                    "unit": "web", "url": "https://web.acme-1.localhost", "state": "ready"}],
        "registry": {"lanes": [{"key": {"project": "acme", "lane": "1"}, "index": 1}],
                     "units": [{"key": {"lane": {"project": "acme", "lane": "1"},
                                        "kind": "service", "name": "registered-only"}}]},
        "box": {"version": "0.0.1", "label": "devlanes 0.0.1", "https_port": 7443,
                "ports_range": "24000-24999"},
        "proxy": {"https_port": 443, "box_port": 7443, "open": true},
        "suffixes": ["localhost", "tld-only-in-json"],
    }], "truncated": false, "next_cursor": null});
    let text = render_text(Method::HostList, &hosts);
    for hidden in [
        "registry",
        "registered-only",
        "7443",
        "24000-24999",
        "tld-only-in-json",
        "connected_since_ms",
    ] {
        assert!(!text.contains(hidden), "{hidden}: {text}");
    }
    assert!(text.contains("https://web.acme-1.localhost"), "{text}");
    assert!(text.contains("connected_since: 5m ago"), "{text}");
}

/// A box waiting for a sign-in shows its state, the error and the command
/// that resumes it in `remote ls`, and `connect` failing that way says so.
#[test]
fn boxes_that_need_a_sign_in_name_the_command() {
    let hosts = json!({"items": [{
        "host": "hetzner", "state": "needs_auth", "transport": "ssh",
        "error": "ssh to hetzner failed (255): Permission denied (publickey).",
        "fix": "devlanes remote connect hetzner", "ports": [],
    }], "truncated": false, "next_cursor": null});
    let text = render_text(Method::HostList, &hosts);
    assert!(text.contains("needs_auth"), "{text}");
    assert!(text.contains("Permission denied (publickey)"), "{text}");
    assert!(text.contains("devlanes remote connect hetzner"), "{text}");

    let error = RpcError::new(
        "remote_needs_auth",
        json!({"host": "hetzner", "message": "Permission denied (publickey).",
               "fix": "devlanes remote connect hetzner"}),
        4,
    );
    let rendered = runtime(OutputMode::Agent).reporter().error(&error, None);
    assert!(
        rendered
            .stderr
            .contains("box hetzner needs you to sign in again: Permission denied (publickey)."),
        "{}",
        rendered.stderr
    );
    assert!(
        rendered.stderr.contains("devlanes remote connect hetzner"),
        "{}",
        rendered.stderr
    );
}

/// A box waiting for pairing shows its state, the box's reason and both
/// commands in `remote ls`; after wrong codes, the wait comes from the time
/// the box takes codes again, and is left out once that passed.
#[test]
fn boxes_that_need_pairing_name_both_commands() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .expect("clock");
    let host = |retry_at: u64| {
        json!({"items": [{
            "host": "hetzner", "state": "needs_pairing", "transport": "ssh",
            "error": "the box no longer accepts this machine's pairing and needs a new code from `devlanes daemon pair` on the box",
            "fix": "devlanes daemon pair on hetzner, then devlanes remote connect hetzner --code <code>",
            "pairing_retry_at_ms": retry_at, "ports": [],
        }], "truncated": false, "next_cursor": null})
    };
    let text = render_text(Method::HostList, &host(now + 3 * 60_000 - 5));
    assert!(text.contains("state: needs_pairing"), "{text}");
    assert!(text.contains("no longer accepts"), "{text}");
    assert!(
        text.contains(
            "fix: devlanes daemon pair on hetzner, then devlanes remote connect hetzner --code <code>"
        ),
        "{text}"
    );
    assert!(
        text.contains("pairing_paused: the box takes codes again in 3 minutes"),
        "{text}"
    );
    assert!(!text.contains("pairing_retry_at_ms"), "{text}");
    let later = render_text(Method::HostList, &host(now - 1));
    assert!(!later.contains("pairing_paused"), "{later}");
}

/// `daemon pair` in each mode: the code with what to run on the client for a
/// person, one line saying what to ask the user for an agent, the code alone
/// when piped, and the daemon's data for `--json`. A box that pairs nobody
/// says so; one paused by wrong codes refuses with the wait.
#[test]
fn daemon_pair_prints_the_code_in_every_mode() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .expect("clock");
    let data = json!({"code": "K7QM-3XWD", "expires_at_ms": now + 10 * 60_000});
    let render = |mode: OutputMode, data: &Value| {
        runtime(mode)
            .reporter()
            .result(&Report::new(Method::RemotePair, data))
            .expect("render")
            .stdout
    };
    let human = render(OutputMode::Human, &data);
    assert_eq!(
        human,
        "pairing code: K7QM-3XWD\n\
         it works once and expires in 10 minutes\n\
         on your other machine, run: devlanes remote connect <box> --code K7QM-3XWD\n"
    );
    let agent = render(OutputMode::Agent, &data);
    assert_eq!(
        agent,
        "pairing code K7QM-3XWD valid_for=10m single_use: ask the user to run \
         `devlanes remote connect <box> --code K7QM-3XWD` on their machine. <box> is this \
         machine's SSH host or remote.boxes name.\n"
    );
    assert_eq!(render(OutputMode::Plain, &data), "K7QM-3XWD\n");
    let json: Value = serde_json::from_str(&render(OutputMode::Json, &data)).expect("json output");
    assert_eq!(json["code"], "K7QM-3XWD");

    let open = json!({"code": "K7QM-3XWD", "expires_at_ms": now + 10 * 60_000,
                      "require_pairing": false});
    let human = render(OutputMode::Human, &open);
    assert!(
        human.contains("remote.require_pairing is false here, so clients connect without a code"),
        "{human}"
    );
    assert!(
        render(OutputMode::Agent, &open).contains("remote.require_pairing is false here"),
        "the agent hears it too"
    );

    let paused = runtime(OutputMode::Agent)
        .reporter()
        .error(
            &RpcError::new(
                "pairing_paused",
                json!({"message": "the box paused pairing after 5 wrong codes in 10 minutes",
                       "retry_after_ms": 4 * 60_000 - 5}),
                4,
            ),
            None,
        )
        .stderr;
    assert!(
        paused.contains("pairing is paused for 4 minutes after 5 wrong codes"),
        "{paused}"
    );
}

/// A box that takes no connections names itself in the client's error, and
/// `daemon pair` on such a box says why it gives no code and how to allow
/// connections.
#[test]
fn connections_off_reads_plainly_on_both_ends() {
    let error = |code: &str, data: serde_json::Value| {
        runtime(OutputMode::Human)
            .reporter()
            .error(&RpcError::new(code, data, 4), None)
            .stderr
    };
    let client = error(
        "remote_failed",
        json!({"host": "hetzner",
               "message": "the box does not accept connections. Set remote.allow_connections to true there"}),
    );
    assert!(
        client.contains(
            "box hetzner does not accept connections. Set remote.allow_connections to true there"
        ),
        "{client}"
    );
    assert!(!client.contains("remote ls"), "no extra hint: {client}");
    let other = error(
        "remote_failed",
        json!({"host": "hetzner", "message": "connecting to the box timed out"}),
    );
    assert!(
        other.contains("remote hetzner: connecting to the box timed out"),
        "{other}"
    );
    let pair = error(
        "connections_off",
        json!({"message": "this machine does not accept connections because remote.allow_connections is false",
               "invalid": false}),
    );
    assert!(
        pair.contains(
            "this machine does not accept connections because remote.allow_connections is false"
        ),
        "{pair}"
    );
    assert!(
        pair.contains("devlanes config set remote.allow_connections true"),
        "{pair}"
    );
    let invalid = error(
        "connections_off",
        json!({"message": "this machine does not accept connections because the remote settings in its user config are invalid",
               "invalid": true}),
    );
    assert!(invalid.contains("devlanes doctor"), "{invalid}");
}

/// `daemon clients` lists each client with a short id that `revoke` takes,
/// whether it is paired, its connection or when it was last seen, and what
/// it mirrors; `revoke` says whom it revoked.
#[test]
fn daemon_clients_and_revoke_name_each_client() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .expect("clock");
    let id = format!("dl_cid_{}", "ab12".repeat(16));
    let clients = json!({"items": [
        {"id": id, "name": "laptop", "paired": true, "pairings": 1,
         "paired_at_ms": now - 86_400_000,
         "last_seen_ms": now, "connections": [{"address": "203.0.113.4",
         "since_ms": now - 5 * 60_000, "mirror": {"ports": 4, "bound": 3, "skipped_lanes": 0}}]},
        {"id": format!("dl_cid_{}", "cd34".repeat(16)), "name": "desk", "paired": true,
         "pairings": 2, "paired_at_ms": now - 86_400_000, "last_seen_ms": now - 3 * 3_600_000,
         "connections": []},
    ]});
    let text = render_text(Method::RemoteClients, &clients);
    assert!(
        text.contains(
            "laptop  dl_cid_ab12ab12  paired      connected for 5m from 203.0.113.4  mirrors 3 of 4 ports"
        ),
        "{text}"
    );
    assert!(
        text.contains("desk    dl_cid_cd34cd34  2 pairings  last seen 3h ago"),
        "{text}"
    );
    let unseen = json!({"items": [{"id": id, "name": "laptop", "paired": true, "pairings": 1,
        "paired_at_ms": now, "last_seen_ms": null, "connections": []}]});
    assert!(
        render_text(Method::RemoteClients, &unseen).contains("not seen yet"),
        "a pairing that never connected"
    );
    assert!(
        text.contains(&format!("client laptop id={id} paired=true connections=1")),
        "{text}"
    );
    let empty = render_text(Method::RemoteClients, &json!({"items": []}));
    assert!(
        empty.contains("no paired or connected clients\nto pair one, run: devlanes daemon pair\n"),
        "{empty}"
    );

    let revoked = json!({"revoked": [{"id": id, "name": "laptop"}], "ended": 1});
    let text = render_text(Method::RemoteRevoke, &revoked);
    assert!(
        text.contains("revoked laptop (dl_cid_ab12ab12) and ended 1 connection"),
        "{text}"
    );
    assert!(
        text.contains(&format!("revoked laptop id={id} ended=1")),
        "{text}"
    );
}

/// A box that needs pairing says what to run on the box and gives the
/// client command as the only hint, with the wait after wrong codes from a
/// time; an ambiguous client lists the matching ids, and one that is
/// connected without pairing is named as such.
#[test]
fn pairing_errors_say_what_to_run() {
    let error = |code: &str, data: Value| {
        runtime(OutputMode::Agent)
            .reporter()
            .error(&RpcError::new(code, data, 4), None)
            .stderr
    };
    let message = "the box needs a pairing code from `devlanes daemon pair` on the box";
    let needs = error(
        "remote_needs_pairing",
        json!({"host": "hetzner", "message": message,
               "fix": "devlanes daemon pair on hetzner, then devlanes remote connect hetzner --code <code>"}),
    );
    assert!(
        needs.contains("box hetzner needs a pairing code from `devlanes daemon pair` on the box"),
        "{needs}"
    );
    assert!(
        needs.contains("devlanes remote connect hetzner --code <code>"),
        "{needs}"
    );
    assert_eq!(
        needs.matches("devlanes daemon pair").count(),
        1,
        "the message names the box command once: {needs}"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .expect("clock");
    let paused = error(
        "remote_needs_pairing",
        json!({"host": "hetzner", "message": "the box paused pairing after 5 wrong codes in 10 minutes",
               "retry_at_ms": now + 7 * 60_000 - 5}),
    );
    assert!(
        paused.contains(
            "box hetzner paused pairing after 5 wrong codes in 10 minutes. The box takes codes again in 7 minutes"
        ),
        "{paused}"
    );
    let unpaired = error(
        "client_not_found",
        json!({"client": "laptop", "unpaired": true, "candidates": []}),
    );
    assert!(
        unpaired.contains(
            "laptop has no pairing to revoke because remote.require_pairing is false here"
        ),
        "{unpaired}"
    );
    let missing = error(
        "client_not_found",
        json!({"client": "tablet", "candidates": [{"id": "dl_cid_ab", "name": "laptop"}]}),
    );
    assert!(
        missing.contains("no paired client is called tablet"),
        "{missing}"
    );
    assert!(missing.contains("devlanes daemon clients"), "{missing}");
    let ambiguous = error(
        "client_ambiguous",
        json!({"client": "laptop", "candidates": [{"id": "dl_cid_ab", "name": "laptop"},
                                                  {"id": "dl_cid_cd", "name": "laptop"}]}),
    );
    assert!(
        ambiguous.contains(
            "laptop matches several paired clients, so give one of their ids: dl_cid_ab, dl_cid_cd"
        ),
        "{ambiguous}"
    );
}

/// Warnings about another box name it; the proxy port and range overlap
/// warnings say what to do.
#[test]
fn box_collision_warnings_name_the_other_box() {
    let line = |data: serde_json::Value| {
        let event = Event {
            seq: 1,
            event: "remote.warning".into(),
            data,
        };
        runtime(OutputMode::Agent)
            .reporter()
            .event(Method::Subscribe, &event)
            .expect("event")
            .stdout
    };
    assert_eq!(
        line(
            json!({"host": "beta", "code": "lane_skipped", "lane": "acme@0",
            "hosts": ["web.acme.localhost"], "box": "alpha"})
        ),
        "warn beta: lane acme@0 is not mirrored because box alpha already serves web.acme.localhost. Run devlanes lane clean for that lane on alpha, or disconnect alpha\n"
    );
    assert_eq!(
        line(
            json!({"host": "beta", "code": "lane_skipped", "lane": "acme@0",
            "hosts": ["web.acme.localhost"], "box": null, "local_lanes": ["acme@0"]})
        ),
        "warn beta: lane acme@0 is not mirrored because lane acme@0 on this machine already serves web.acme.localhost. Stopped lanes keep their hostnames, so run devlanes lane clean acme@0 to free them\n"
    );
    assert_eq!(
        line(
            json!({"host": "beta", "code": "port_in_use", "port": 20004, "box": "alpha",
            "held_by": "acme@0:db", "message": "…"})
        ),
        "warn beta: port 20004 is not mirrored because box alpha already mirrors it for acme@0:db\n"
    );
    assert_eq!(
        line(
            json!({"host": "hetzner", "code": "proxy_port_in_use", "port": 8443,
            "own_port": 443, "holder": "in use by nginx, pid 812", "message": "…"})
        ),
        "warn hetzner: this machine cannot use the box's HTTPS port 8443 (in use by nginx, pid 812). Open its hostnames on port 443.\n"
    );
    assert_eq!(
        line(
            json!({"host": "beta", "code": "ports_range_overlap", "range": "20000-32767",
            "overlaps": ["box alpha (20000-29999)", "box gamma (30000-32767)"],
            "message": "…"})
        ),
        "warn beta: the box's ports.range 20000-32767 overlaps box alpha (20000-29999) and box gamma (30000-32767). Lanes both boxes run go to one box at a time. Other lanes can get the same ports, so give each box its own ports.range if that matters.\n"
    );
}

/// A skipped lane's warning names the lane in use and the command that
/// switches, or what to stop first; `choice_cleared` says which local lane
/// took the hostnames back.
#[test]
fn lane_choice_warnings_say_how_to_switch() {
    let line = |data: serde_json::Value| {
        let event = Event {
            seq: 1,
            event: "remote.warning".into(),
            data,
        };
        runtime(OutputMode::Agent)
            .reporter()
            .event(Method::Subscribe, &event)
            .expect("event")
            .stdout
    };
    let skipped = |cause: &str, winner: Value| {
        json!({"host": "fork-b", "code": "lane_skipped", "lane": "acme@0",
               "hosts": ["web.acme.localhost"], "box": winner, "local_lanes": [],
               "cause": cause, "winner_lane": "acme@0"})
    };
    assert_eq!(
        line(skipped("box_earlier", json!("fork-a"))),
        "warn fork-b: lane acme@0 is not mirrored because fork-a's acme@0 is in use. devlanes remote use fork-b acme@0 switches\n"
    );
    assert_eq!(
        line(skipped("local_stopped", Value::Null)),
        "warn fork-b: lane acme@0 is not mirrored because this machine's acme@0 is in use. devlanes remote use fork-b acme@0 switches\n"
    );
    assert_eq!(
        line(skipped("local_running", Value::Null)),
        "warn fork-b: lane acme@0 is not mirrored because acme@0 runs on this machine. Stop it with devlanes stop --lane acme@0, then run devlanes remote use fork-b acme@0.\n"
    );
    // `devlanes stop` leaves containers up, so the commands stop exactly the
    // lane's own ones.
    let mut held = skipped("local_running", Value::Null);
    held["holds"] = json!("containers");
    held["compose_project"] = json!("acme-lane0");
    held["containers"] = json!(["devlanes-acme-lane0-cerbos"]);
    assert_eq!(
        line(held),
        "warn fork-b: lane acme@0 is not mirrored because acme@0's containers run on this machine. Stop them with docker compose -p acme-lane0 stop and docker stop devlanes-acme-lane0-cerbos, then run devlanes remote use fork-b acme@0.\n"
    );
    // A switch someone asked for is a plain line, not a warning.
    assert_eq!(
        line(
            json!({"host": "fork-b", "code": "lane_switched", "lane": "acme@0",
                    "to": "fork-b", "from": "fork-a", "message": "…"})
        ),
        "acme@0 now comes from fork-b\n"
    );
    assert_eq!(
        line(
            json!({"host": "fork-b", "code": "lane_switched", "lane": "acme@0",
                    "to": null, "from": "fork-b", "message": "…"})
        ),
        "acme@0 now comes from this machine\n"
    );
    assert_eq!(
        line(
            json!({"host": "fork-b", "code": "choice_cleared", "lane": "acme@0",
                    "local_lane": "acme@0", "message": "…"})
        ),
        "warn fork-b: acme@0 started on this machine, so fork-b's acme@0 is no longer mirrored or chosen.\n"
    );
    assert_eq!(
        line(
            json!({"host": "fork-b", "code": "choice_cleared", "lane": "shop@0",
                    "local_lane": "acme@1", "message": "…"})
        ),
        "warn fork-b: acme@1 started on this machine, so fork-b's shop@0 is no longer mirrored or chosen.\n"
    );
}

/// `remote use` says where the lane comes from now and which lane stopped
/// serving it; agents get one line, and `--json` the daemon's reply.
#[test]
fn remote_use_says_where_the_lane_comes_from() {
    let human = |data: Value| {
        runtime(OutputMode::Human)
            .reporter()
            .result(&Report::new(Method::RemoteUse, &data))
            .expect("render")
            .stdout
    };
    let reply = |host: Value, previous: Value, mirrored: bool| json!({"lane": "clickmax@0", "host": host, "previous": previous, "mirrored": mirrored});
    assert_eq!(
        human(reply(json!("fork-b"), json!("fork-a"), true)),
        "clickmax@0 now comes from fork-b. fork-a's lane is no longer mirrored.\n"
    );
    assert_eq!(
        human(reply(json!("fork-b"), Value::Null, true)),
        "clickmax@0 now comes from fork-b. It is in use instead of this machine's lane until that one starts.\n"
    );
    assert_eq!(
        human(reply(Value::Null, json!("fork-b"), true)),
        "clickmax@0 now comes from this machine. fork-b's lane is no longer mirrored.\n"
    );
    assert_eq!(
        human(reply(json!("fork-b"), json!("fork-b"), true)),
        "clickmax@0 already comes from fork-b.\n"
    );
    assert_eq!(
        human(reply(json!("fork-b"), json!("fork-a"), false)),
        "fork-b's clickmax@0 is chosen but not mirrored yet. devlanes remote ls shows why.\n"
    );
    let mut replacing = reply(Value::Null, json!("fork-a"), true);
    replacing["replaced"] = json!([{"lane": "shop@0", "host": "fork-a"}]);
    assert_eq!(
        human(replacing),
        "clickmax@0 now comes from this machine. fork-a's lane is no longer mirrored.\nThe choice of fork-a's shop@0 is deleted, since it clashes.\n"
    );
    let mut offline = reply(json!("fork-b"), json!("fork-a"), false);
    offline["pending"] = json!(true);
    assert_eq!(
        human(offline),
        "clickmax@0 will come from fork-b once it connects.\n"
    );
    let data = reply(json!("fork-b"), json!("fork-a"), true);
    let agent = runtime(OutputMode::Agent)
        .reporter()
        .result(&Report::new(Method::RemoteUse, &data))
        .expect("render")
        .stdout;
    assert_eq!(
        agent,
        "remote use lane=clickmax@0 from=fork-b previous=fork-a mirrored=true pending=false\n"
    );
    let json = runtime(OutputMode::Json)
        .reporter()
        .result(&Report::new(Method::RemoteUse, &data))
        .expect("json")
        .stdout;
    assert_eq!(serde_json::from_str::<Value>(&json).expect("json"), data);
}

/// `remote ls` gives each skipped lane a note with the lane in use and the
/// command that switches, marks the lanes chosen for a box, and lists this
/// machine's lanes a box lane is in use instead of. `--json` keeps every
/// field.
#[test]
fn remote_ls_names_the_lane_in_use() {
    let hosts = json!({"items": [
        {"host": "fork-a", "state": "connected", "transport": "ssh", "ports": [],
         "chosen_lanes": [],
         "skipped_lanes": [
            {"lane": "clickmax@0", "hosts": ["web.clickmax.localhost"], "box": "fork-b",
             "reason": "fork-b's clickmax@0 is in use", "cause": "box_chosen",
             "winner": "fork-b", "winner_lane": "clickmax@0", "can_use": true},
            {"lane": "shop@0", "hosts": ["shop.localhost"], "box": null,
             "reason": "shop@0 runs on this machine", "cause": "local_running",
             "winner": null, "winner_lane": "shop@0", "can_use": false,
             "holds": "processes"},
            {"lane": "blog@0", "hosts": ["blog.localhost"], "box": null,
             "reason": "blog@0's containers run on this machine", "cause": "local_running",
             "winner": null, "winner_lane": "blog@0", "can_use": false,
             "holds": "containers", "compose_project": "blog-lane0", "containers": []}]},
        {"host": "fork-b", "state": "connected", "transport": "ssh", "ports": [],
         "chosen_lanes": ["clickmax@0"], "skipped_lanes": []},
    ], "yielded_local_lanes": [{"lane": "clickmax@0", "winner": "fork-b"}],
       "truncated": false, "next_cursor": null});
    let text = render_text(Method::HostList, &hosts);
    for expected in [
        "note: not mirrored because fork-b's clickmax@0 is in use. devlanes remote use fork-a clickmax@0 switches.",
        "note: not mirrored because shop@0 runs on this machine. Stop it with devlanes stop --lane shop@0, then run devlanes remote use fork-a shop@0.",
        "note: not mirrored because blog@0's containers run on this machine. Stop them with docker compose -p blog-lane0 stop, then run devlanes remote use fork-a blog@0.",
        "chosen: clickmax@0",
        "note: fork-b's lane is in use instead. devlanes remote use --local clickmax@0 switches back.",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    for hidden in [
        "cause:",
        "can_use",
        "winner_lane",
        "chosen_lanes",
        "yielded",
        "holds",
        "compose_project",
    ] {
        assert!(!text.contains(hidden), "{hidden}: {text}");
    }
    let json = runtime(OutputMode::Json)
        .reporter()
        .result(&Report::new(Method::HostList, &hosts))
        .expect("json")
        .stdout;
    assert_eq!(serde_json::from_str::<Value>(&json).expect("json"), hosts);
}

/// Each `remote use` refusal says what is wrong and what to run.
#[test]
fn remote_use_refusals_say_what_to_do() {
    let stderr = |code: &str, data: Value| {
        runtime(OutputMode::Agent)
            .reporter()
            .error(&RpcError::new(code, data, 1), None)
            .stderr
    };
    for (code, data, says, fix) in [
        (
            "box_unknown",
            json!({"host": "fork-c", "message": "…"}),
            "this machine knows no box called fork-c",
            "devlanes remote ls",
        ),
        (
            "lane_not_on_box",
            json!({"host": "fork-b", "lane": "acme@9", "message": "…"}),
            "box fork-b has no lane acme@9",
            "devlanes remote ls",
        ),
        (
            "local_lane_running",
            json!({"host": "fork-b", "lane": "clickmax@0", "local_lane": "clickmax@0", "message": "…"}),
            "clickmax@0 runs on this machine. Stop it with devlanes stop --lane clickmax@0, then run devlanes remote use fork-b clickmax@0.",
            "devlanes stop --lane clickmax@0",
        ),
        (
            "local_lane_running",
            json!({"host": "fork-b", "lane": "clickmax@0", "local_lane": "clickmax@0",
                   "holds": "containers", "compose_project": "clickmax-lane0",
                   "containers": ["devlanes-clickmax-lane0-cerbos"], "message": "…"}),
            "clickmax@0's containers run on this machine. Stop them with docker compose -p clickmax-lane0 stop and docker stop devlanes-clickmax-lane0-cerbos, then run devlanes remote use fork-b clickmax@0.",
            "docker compose -p clickmax-lane0 stop",
        ),
        (
            "local_lane_unknown",
            json!({"lane": "acme@4", "message": "…"}),
            "this machine has no lane acme@4",
            "devlanes remote use <box> acme@4",
        ),
    ] {
        let text = stderr(code, data);
        assert!(text.contains(says), "{code}: {text}");
        assert!(text.contains(fix), "{code}: {text}");
    }
}
