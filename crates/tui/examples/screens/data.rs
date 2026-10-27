//! A realistic fake daemon: a 45-unit monorepo lane with containers, a
//! starting lane, an idle lane, a lane whose worktree is gone, a second
//! project and a third whose config does not load; [`idle_lanes`] freezes
//! and throttles the first two.

use std::collections::BTreeMap;

use devlanes_proto::{
    Actor, ActorKind, ContainerSummary, LanePower, LaneSummary, LogLine, ORPHANED, RpcError,
    UnitSummary,
};
use devlanes_tui::{
    DaemonMetrics, LaneMetrics, Metrics, SystemMetrics, UnitMetrics,
    api::{ProfileInfo, ProfileList, TaskInfo},
};
use serde_json::{Value, json};

/// Services of the monorepo lane, by prefix family.
const SERVICES: [&str; 34] = [
    "api-gateway",
    "api-auth",
    "api-users",
    "api-billing",
    "api-orders",
    "api-search",
    "api-catalog",
    "api-inventory",
    "api-notify",
    "api-reports",
    "web-app",
    "web-admin",
    "web-docs",
    "web-storybook",
    "web-landing",
    "wrk-email",
    "wrk-billing",
    "wrk-index",
    "wrk-thumbs",
    "wrk-export",
    "wrk-cron",
    "wrk-webhooks",
    "wrk-ledger",
    "stub-stripe",
    "stub-s3",
    "stub-twilio",
    "stub-sendgrid",
    "stub-segment",
    "stub-auth0",
    "proxy-edge",
    "docs-search",
    "ml-ranker",
    "ml-embed",
    "cache-warm",
];

/// Tasks of the monorepo lane, setup steps first.
const TASKS: [&str; 11] = [
    "setup.db",
    "setup.seed",
    "setup.certs",
    "setup.env",
    "migrate",
    "seed",
    "lint",
    "typecheck",
    "test-e2e",
    "codegen",
    "clean-cache",
];

/// A unit with the fields a daemon fills for every unit. Like most monorepos
/// it has no tags, so the daemon colors it by the name's first segment.
fn unit(lane: &str, name: &str, state: &str, kind: &str) -> UnitSummary {
    let color = devlanes_core::color::unit_color(&[], name, None, &BTreeMap::new());
    // Running units started 2h 14m before the fake's clock.
    let live = matches!(state, "ready" | "running" | "starting" | "failed");
    UnitSummary {
        name: name.into(),
        lane: lane.into(),
        state: state.into(),
        kind: kind.into(),
        autostart: kind == "service",
        color: Some(color.into()),
        started_at_ms: live.then_some(NOW_MS - 8_040_000),
        ..UnitSummary::default()
    }
}

/// An `infra.external` entry, as the daemon lists it in every lane.
fn external(lane: &str, name: &str, port: u16, host: Option<&str>) -> UnitSummary {
    let mut unit = unit(lane, name, "external", "external");
    unit.role = Some("external".into());
    unit.autostart = false;
    unit.pty = false;
    unit.ports = BTreeMap::from([("default".to_owned(), port)]);
    unit.public_url = host.map(|host| format!("https://{host}"));
    unit.urls = std::iter::once(format!("http://127.0.0.1:{port}"))
        .chain(unit.public_url.clone())
        .collect();
    unit
}

/// Containers by name, host port and whether they are shared project infra.
fn containers(list: &[(&str, u16, bool)]) -> Vec<ContainerSummary> {
    list.iter()
        .map(|(name, port, shared)| ContainerSummary {
            name: (*name).into(),
            ports: BTreeMap::from([("tcp".to_owned(), *port)]),
            color: Some(devlanes_core::color::unit_color(&[], name, None, &BTreeMap::new()).into()),
            shared: *shared,
            state: Some("healthy".into()),
            cpu_percent: Some(2.0),
            memory_bytes: Some(180 << 20),
            ..ContainerSummary::default()
        })
        .collect()
}

/// `task.list` for a lane of `acme`: every configured task.
pub fn tasks(lane: &str) -> Vec<TaskInfo> {
    if !lane.starts_with("acme@") {
        return Vec::new();
    }
    TASKS
        .iter()
        .filter(|name| !name.starts_with("setup."))
        .map(|name| TaskInfo {
            name: (*name).into(),
            when: (*name == "migrate").then(|| "lane.start".into()),
            confirm: *name == "seed",
            ..TaskInfo::default()
        })
        .collect()
}

/// The ports a service publishes, from its position in the lane's block.
fn ports(index: usize) -> BTreeMap<String, u16> {
    BTreeMap::from([(
        "http".to_owned(),
        41_000 + u16::try_from(index).unwrap_or(0),
    )])
}

/// `acme@0`: every service of the monorepo, one failed, one orphaned.
fn monorepo() -> LaneSummary {
    let reference = "acme@0";
    let mut units: Vec<UnitSummary> = SERVICES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let mut unit = unit(reference, name, "ready", "service");
            unit.ports = ports(index);
            unit.pid = Some(51_000 + u32::try_from(index).unwrap_or(0));
            unit.cpu_percent = Some((index % 7) as f64 * 3.5);
            unit.memory_bytes = Some((80 + (index as u64 * 37) % 400) << 20);
            if name.starts_with("api-") || name.starts_with("web-") {
                let host = name.split_once('-').map_or(*name, |(_, rest)| rest);
                unit.urls = vec![format!("https://{host}.acme.localhost")];
            }
            unit
        })
        .collect();
    for unit in &mut units {
        match unit.name.as_str() {
            "wrk-export" => {
                unit.state = "failed".into();
                unit.pid = None;
                unit.exit_code = Some(1);
                unit.reason = Some(RpcError::new(
                    "unit_exited",
                    json!({"unit": "wrk-export", "exit_code": 1}),
                    1,
                ));
            }
            "stub-twilio" => {
                unit.state = ORPHANED.into();
                unit.pid = Some(48_211);
                unit.orphaned_since_ms = Some(1_761_036_000_000);
            }
            "ml-embed" => unit.state = "starting".into(),
            // Listed in the config's hidden services: shown dimmed.
            "cache-warm" => unit.hidden = true,
            "api-gateway" => unit.display_name = Some("Gateway API".into()),
            "web-storybook" => {
                unit.state = "idle".into();
                unit.autostart = false;
                unit.pid = None;
            }
            "wrk-cron" => {
                unit.actor = Actor {
                    kind: ActorKind::Agent,
                    agent_id: Some("claude".into()),
                    session_id: None,
                };
            }
            _ => {}
        }
        // Workers have no readiness check: a live process shows as running.
        if unit.name.starts_with("wrk-") {
            unit.ready_check = Some("started".into());
        }
    }
    // The daemon lists a task once it ran; `task.list` names the others.
    units.extend(
        TASKS
            .iter()
            .filter(|name| {
                name.starts_with("setup.") || matches!(**name, "migrate" | "seed" | "codegen")
            })
            .map(|name| {
                let mut task = unit(reference, name, "exited", "task");
                task.exit_code = Some(0);
                task.pty = false;
                if name.starts_with("setup.") {
                    task.role = Some("setup".into());
                }
                task
            }),
    );
    // pgbouncer is in `infra.hidden`: listed dimmed under shared.
    let mut containers = containers(&[
        ("localstack", 4566, false),
        ("postgres", 5432, true),
        ("rabbitmq", 5672, false),
        ("redis", 6379, true),
        ("pgbouncer", 6432, true),
    ]);
    if let Some(pgbouncer) = containers.iter_mut().find(|c| c.name == "pgbouncer") {
        pgbouncer.hidden = true;
    }
    LaneSummary {
        containers,
        reference: reference.into(),
        project: "acme".into(),
        index: Some(0),
        branch: Some("main".into()),
        path: "/work/acme".into(),
        state: "failed".into(),
        units,
        profiles: vec!["web".into()],
        profile: Some("web".into()),
        domain: Some("acme.localhost".into()),
        port_block: Some((41_000, 41_099)),
        ..LaneSummary::default()
    }
}

/// `acme@1`: a lane in the middle of its start.
fn starting() -> LaneSummary {
    let reference = "acme@1";
    let mut units = vec![
        unit(reference, "api-gateway", "ready", "service"),
        unit(reference, "api-orders", "starting", "service"),
        unit(reference, "api-users", "starting", "service"),
        unit(reference, "web-app", "queued", "service"),
        unit(reference, "wrk-email", "queued", "service"),
        unit(reference, "stub-stripe", "ready", "service"),
        unit(reference, "setup.db", "exited", "task"),
        unit(reference, "migrate", "running", "task"),
        external(reference, "pgadmin", 5050, Some("pgadmin.acme-1.localhost")),
    ];
    units[6].role = Some("setup".into());
    units[6].exit_code = Some(0);
    for unit in &mut units {
        if unit.state == "queued" {
            unit.reason = Some(RpcError::new(
                "queued",
                json!({"position": 1, "queued": 2, "in_flight": 4, "window": 4, "hold": "window_full"}),
                0,
            ));
        }
    }
    LaneSummary {
        reference: reference.into(),
        project: "acme".into(),
        index: Some(1),
        branch: Some("feat/checkout".into()),
        path: "/work/acme-checkout".into(),
        state: "starting".into(),
        units,
        port_block: Some((41_100, 41_199)),
        profiles: vec!["web".into()],
        containers: containers(&[("postgres", 5432, true), ("redis", 6379, true)]),
        ..LaneSummary::default()
    }
}

/// `lane.phase` events of `acme@1`'s start so far: infra and setup done,
/// tasks running.
pub fn start_phases() -> Vec<serde_json::Value> {
    let phase = |phase: &str, state: &str| json!({"lane": "acme@1", "phase": phase, "state": state, "start_id": 7});
    vec![
        phase("infra", "started"),
        phase("infra", "done"),
        phase("setup", "started"),
        phase("setup", "done"),
        phase("tasks", "started"),
    ]
}

/// Every lane the fake daemon reports.
pub fn lanes() -> Vec<LaneSummary> {
    let idle = LaneSummary {
        reference: "acme@2".into(),
        project: "acme".into(),
        index: Some(2),
        branch: Some("fix/cache".into()),
        path: "/work/acme-cache".into(),
        state: "idle".into(),
        config_changed: true,
        units: vec![
            unit("acme@2", "api-gateway", "idle", "service"),
            unit("acme@2", "web-app", "idle", "service"),
        ],
        ..LaneSummary::default()
    };
    let missing = LaneSummary {
        reference: "acme@3".into(),
        project: "acme".into(),
        index: Some(3),
        branch: Some("spike/old-search".into()),
        path: "/work/acme-old-search".into(),
        state: "no-config".into(),
        reason: Some(RpcError::new(
            "worktree_missing",
            json!({"path": "/work/acme-old-search", "message": "the worktree directory is gone"}),
            1,
        )),
        ..LaneSummary::default()
    };
    let shop = LaneSummary {
        reference: "shop@0".into(),
        project: "shop".into(),
        index: Some(0),
        branch: Some("main".into()),
        path: "/work/shop".into(),
        state: "running".into(),
        units: vec![
            unit("shop@0", "storefront", "ready", "service"),
            unit("shop@0", "checkout-api", "ready", "service"),
            unit("shop@0", "admin", "stopped", "service"),
        ],
        ..LaneSummary::default()
    };
    let shop_feature = LaneSummary {
        reference: "shop@1".into(),
        project: "shop".into(),
        index: Some(1),
        branch: Some("feat/gift-cards".into()),
        path: "/work/shop-gift-cards".into(),
        state: "idle".into(),
        ..LaneSummary::default()
    };
    let billing = LaneSummary {
        reference: "billing@0".into(),
        project: "billing".into(),
        index: Some(0),
        branch: Some("main".into()),
        path: "/work/billing".into(),
        state: "no-config".into(),
        reason: Some(RpcError::new(
            "config_error",
            json!({
                "message": "services.invoices.port: expected a number, found \"auto\"",
                "file": "/work/billing/devlanes.yaml",
                "line": 42,
                "column": 11,
            }),
            1,
        )),
        ..LaneSummary::default()
    };
    vec![
        monorepo(),
        starting(),
        idle,
        missing,
        shop,
        shop_feature,
        billing,
    ]
}

/// [`lanes`] after a quiet stretch: `acme@0` frozen with its own containers
/// paused, `acme@1` started and throttled. Their failed, orphaned and busy
/// units are settled first, since the daemon never idles a lane with those.
pub fn idle_lanes() -> Vec<LaneSummary> {
    let mut lanes = lanes();
    for lane in &mut lanes {
        let level = match lane.reference.as_str() {
            "acme@0" => "frozen",
            "acme@1" => "throttled",
            _ => continue,
        };
        lane.state = "running".into();
        lane.power = Some(LanePower {
            level: level.into(),
            since_ms: NOW_MS - 720_000,
            last_activity_ms: Some(NOW_MS - 1_620_000),
            last_activity: Some("proxy".into()),
        });
        for unit in &mut lane.units {
            unit.reason = None;
            if unit.kind == "task" {
                unit.state = "exited".into();
                unit.exit_code = Some(0);
                continue;
            }
            // An external port has no process to throttle or freeze.
            if unit.state == "idle" || unit.kind == "external" {
                continue;
            }
            unit.state = "ready".into();
            unit.exit_code = None;
            unit.orphaned_since_ms = None;
            unit.power = Some(level.into());
        }
        if level == "frozen" {
            for container in lane.containers.iter_mut().filter(|c| !c.shared) {
                container.state = Some("paused".into());
            }
        }
    }
    lanes
}

/// `host.list` with three boxes: `hetzner` connected with two `acme` lanes
/// (lane 0 skipped, since this machine serves its hostnames), `devbox`
/// reconnecting through a command with a `shop` lane, and `gpu` waiting for
/// a sign-in. Ports sit in the box's range, clear of this machine's blocks.
pub fn boxes() -> Value {
    let key = |project: &str, lane: u32| json!({"project": project, "lane": lane.to_string()});
    let unit = |project: &str,
                lane: u32,
                kind: &str,
                name: &str,
                state: &str,
                ports: Value,
                hosts: Value| {
        json!({
            "key": {"lane": key(project, lane), "kind": kind, "name": name},
            "hidden": false, "state": state, "ports": ports, "hosts": hosts,
        })
    };
    let port = |port: u16| json!([{"name": "default", "port": port}]);
    let host = |host: &str, port: u16| json!([{"host": host, "port": port, "fallback": false, "host_header": "keep"}]);
    let mut admin = unit(
        "acme",
        4,
        "service",
        "api-admin",
        "ready",
        port(24_103),
        json!([]),
    );
    admin["hidden"] = json!(true);
    let mirrored = |port: u16, unit: &str, state: &str, conflicts: Value, relay: Value| {
        let addresses = if state == "bound" {
            json!(["127.0.0.1", "::1"])
        } else {
            json!([])
        };
        json!({
            "port": port, "lane": "acme@4", "unit": unit, "kind": "service", "state": state,
            "addresses": addresses,
            "conflicts": conflicts, "relay": relay, "last_error": null,
        })
    };
    let route = |host: &str, unit: &str, state: &str| {
        json!({"host": host, "port": 0, "lane": "acme@4", "unit": unit,
               "url": format!("https://{host}:8443"), "state": state})
    };
    let mut gateway = mirrored(24_100, "api-gateway", "bound", json!([]), Value::Null);
    gateway["last_error"] = json!({
        "reason": "dial_failed", "at_ms": NOW_MS - 120_000,
        "message": "nothing on the box accepted it (connection refused)",
    });
    let mut db = mirrored(24_110, "postgres", "bound", json!([]), Value::Null);
    db["kind"] = json!("container");
    json!({"items": [
        {
            "host": "hetzner", "state": "connected", "transport": "ssh",
            "server": "devlanes 0.9.2 (linger off)", "daemon_version": "devlanes 0.9.2 (linger off)",
            "address": "hetzner", "last_connected_ms": NOW_MS - 8_040_000,
            "connected_since_ms": NOW_MS - 8_040_000, "https_port": 443, "lanes": 2,
            "ports": [
                gateway,
                mirrored(24_101, "web-app", "in_use", json!([{
                    "address": "127.0.0.1", "pid": 9131, "box": null, "holder": "node (pid 9131)",
                    "message": "port 24101 is not mirrored because node (pid 9131) uses it here",
                }]), json!(53_001)),
                mirrored(24_102, "wrk-email", "bound", json!([]), Value::Null),
                mirrored(24_105, "ext-build", "bound", json!([]), Value::Null),
                db,
            ],
            "routes": [
                route("gateway.acme-4.localhost", "api-gateway", "ready"),
                route("app.acme-4.localhost", "web-app", "ready"),
                route("docs.acme-4.localhost", "web-docs", "idle"),
            ],
            "skipped_lanes": [{"lane": "acme@0", "hosts": ["gateway.acme.localhost"], "box": null,
                               "reason": "this machine serves its hostnames"}],
            "skipped_hosts": [{"host": "ext.example.com", "lane": "acme@4", "unit": "ext-build"}],
            "syncs": [{"lane": "acme@4", "unit": "ext-build", "index": 0,
                       "from": "/home/acme/acme/ext/dist",
                       "to": "/Users/acme/.devlanes/sync/hetzner/acme/acme@4/dist",
                       "state": "watching", "written": 214, "removed": 0, "pruned": 0,
                       "compared": 214, "errors": [], "error": null, "updated_ms": NOW_MS}],
            "warnings": [
                format!("the box runs devlanes 0.9.2 and this machine {VERSION}. Install the same release on both."),
                "the box stops your processes at logout. Run loginctl enable-linger or devlanes daemon install there.",
                "this machine cannot use the box's HTTPS port 443 (permission denied). Open its hostnames on port 8443.",
            ],
            "registry": {
                "lanes": [
                    {"key": key("acme", 0), "index": 0, "domain": "acme.localhost"},
                    {"key": key("acme", 4), "index": 4, "domain": "acme-4.localhost"},
                ],
                "units": [
                    unit("acme", 0, "service", "api-gateway", "ready", port(24_000), host("gateway.acme.localhost", 24_000)),
                    unit("acme", 0, "service", "web-app", "ready", port(24_001), json!([])),
                    unit("acme", 4, "service", "api-gateway", "ready", port(24_100), host("gateway.acme-4.localhost", 24_100)),
                    unit("acme", 4, "service", "web-app", "ready", port(24_101), host("app.acme-4.localhost", 24_101)),
                    unit("acme", 4, "service", "wrk-email", "running", port(24_102), json!([])),
                    admin,
                    unit("acme", 4, "service", "web-docs", "idle", port(24_104), host("docs.acme-4.localhost", 24_104)),
                    unit("acme", 4, "service", "ext-build", "ready", port(24_105), host("ext.example.com", 24_105)),
                    unit("acme", 4, "task", "migrate", "exited", json!([]), json!([])),
                    unit("acme", 4, "container", "postgres", "running",
                         json!([{"name": "5432", "port": 24_110}]), json!([])),
                ],
            },
            "box": {"version": "0.9.2", "label": "devlanes 0.9.2 (linger off)",
                    "https_port": 443, "ports_range": "24000-29999"},
            "suffixes": ["localhost"],
            "proxy": {"https_port": 8443, "box_port": 443, "open": false},
        },
        {
            "host": "devbox", "state": "reconnecting", "transport": "command",
            "retry_in_ms": 8_000,
            "error": "docker exec exited 1: Error response from daemon: container devbox is not running",
            "daemon_version": format!("devlanes {VERSION}"), "connected_since_ms": null,
            "last_connected_ms": NOW_MS - 600_000, "https_port": 9443,
            "ports": [], "routes": [], "skipped_lanes": [], "skipped_hosts": [], "syncs": [], "warnings": [],
            "registry": {
                "lanes": [{"key": key("shop", 0), "index": 0, "domain": "shop.localhost"}],
                "units": [
                    unit("shop", 0, "service", "storefront", "ready", port(30_000), json!([])),
                    unit("shop", 0, "service", "checkout-api", "failed", port(30_001), json!([])),
                ],
            },
            "box": {"version": VERSION, "https_port": 9443, "ports_range": "30000-32767"},
            "suffixes": ["localhost"],
            "proxy": {"https_port": 8443, "box_port": 9443, "open": true},
        },
        {
            "host": "gpu", "state": "needs_auth", "transport": "ssh",
            "error": "ssh to gpu failed (255): Permission denied (publickey).",
            "fix": "devlanes remote connect gpu", "last_connected_ms": NOW_MS - 86_400_000,
            "daemon_version": format!("devlanes {VERSION}"),
            "ports": [], "routes": [], "skipped_lanes": [], "skipped_hosts": [], "syncs": [], "warnings": [],
            "registry": {"lanes": [], "units": []},
            "suffixes": ["localhost"],
        },
    ], "truncated": false, "next_cursor": null})
}

/// [`boxes`] after `hetzner` revoked this machine's pairing: its link waits
/// for a new code, its ports and routes stay, and the routes serve the
/// pairing page.
pub fn boxes_pairing() -> Value {
    let mut list = boxes();
    let hetzner = list["items"]
        .as_array_mut()
        .and_then(|items| items.iter_mut().find(|item| item["host"] == "hetzner"));
    if let Some(hetzner) = hetzner {
        hetzner["state"] = json!("needs_pairing");
        hetzner["error"] = json!(
            "the box no longer accepts this machine's pairing and needs a new code from `devlanes daemon pair` on the box"
        );
        hetzner["fix"] = json!(
            "devlanes daemon pair on hetzner, then devlanes remote connect hetzner --code <code>"
        );
        hetzner["connected_since_ms"] = Value::Null;
        for route in hetzner["routes"].as_array_mut().into_iter().flatten() {
            route["state"] = json!("pairing_needed");
        }
    }
    list
}

/// `remote.clients` with nobody paired or connected, pairing required.
pub fn no_clients() -> Value {
    json!({"items": [], "now_ms": NOW_MS, "require_pairing": true, "allow_connections": true})
}

/// [`no_clients`] on a machine with `remote.allow_connections: false`.
pub fn no_clients_off() -> Value {
    json!({"items": [], "now_ms": NOW_MS, "require_pairing": true, "allow_connections": false})
}

/// `remote.pair`'s code, valid for 10 minutes less two seconds, as the TUI
/// reads it.
pub fn pair_code() -> Value {
    json!({"code": "K7QM-3XWD", "expires_at_ms": NOW_MS + 598_000, "valid_ms": 598_000})
}

/// `remote.pair` while 5 wrong codes keep pairing paused for 4m 12s more.
pub fn pairing_paused() -> RpcError {
    RpcError::new(
        "pairing_paused",
        json!({"message": "the box paused pairing after 5 wrong codes in 10 minutes",
               "retry_after_ms": 252_000}),
        4,
    )
}

/// `remote.pair` on a machine with `remote.allow_connections: false`.
pub fn connections_off() -> RpcError {
    RpcError::new(
        "connections_off",
        json!({"message": "this machine does not accept connections because remote.allow_connections is false",
               "invalid": false}),
        4,
    )
}

/// `connect`'s answer for a box with `remote.allow_connections: false`.
pub fn connect_refused(host: &str) -> RpcError {
    RpcError::new(
        "remote_failed",
        json!({"host": host,
               "message": "the box does not accept connections. Set remote.allow_connections to true there"}),
        4,
    )
}

/// `remote.clients` on this machine as a box, connected clients first as the
/// daemon sorts them: a laptop connected with two pairings (two box names),
/// then a CI runner last seen an hour ago and a desktop away for three days.
pub fn clients() -> Value {
    json!({
        "now_ms": NOW_MS,
        "require_pairing": true,
        "allow_connections": true,
        "items": [
            {"id": "dl_cid_3f2a9c1e5b7d40a8c6e2f1b9d3a7c5e0b8f4d2a6c1e9b7d5f3a0c8e6b4d2f1a9",
             "name": "laptop", "paired": true, "pairings": 2,
             "paired_at_ms": NOW_MS - 604_800_000, "last_seen_ms": NOW_MS - 8_040_000,
             "connections": [{"address": "203.0.113.24", "since_ms": NOW_MS - 8_040_000,
                              "mirror": {"ports": 6, "bound": 5, "skipped_lanes": 1}}]},
            {"id": "dl_cid_0b9e44d1c7a35f2e8a6d0c1b9f4e7a2d3c5b8e1f0a9d6c4b2e7f3a1d5c8b0e9f",
             "name": "ci-runner", "paired": true, "pairings": 1,
             "paired_at_ms": NOW_MS - 7_200_000, "last_seen_ms": NOW_MS - 3_900_000,
             "connections": []},
            {"id": "dl_cid_7c41e0a9b2d85f3c6e1a4d7b0c9f2e5a8d3b6c1f4e7a0d9c2b5e8f1a4d7c0b3e",
             "name": "desktop", "paired": true, "pairings": 1,
             "paired_at_ms": NOW_MS - 1_209_600_000, "last_seen_ms": NOW_MS - 266_400_000,
             "connections": []},
        ],
    })
}

/// [`clients`] on a box with `remote.require_pairing: false`: a tablet is
/// connected without pairing, listed after the laptop with the connected.
pub fn clients_unpaired() -> Value {
    let mut list = clients();
    list["require_pairing"] = json!(false);
    if let Some(items) = list["items"].as_array_mut() {
        items.insert(
            1,
            json!({
                "id": "dl_cid_e5d8a1c4b7f02e6d9a3c8b1f5e4d7a0c2b9f6e3d8a5c1b4f7e0d3a6c9b2f5e8d1",
                "name": "tablet", "paired": false, "pairings": 0,
                "paired_at_ms": null, "last_seen_ms": null,
                "connections": [{"address": "198.51.100.7", "since_ms": NOW_MS - 300_000,
                                 "mirror": null}],
            }),
        );
    }
    list
}

/// The Docker log lines a container's pane shows, as the TUI writes them.
pub fn container_logs(container: &str) -> Vec<u8> {
    let lines: Vec<String> = (0..6)
        .map(|i| {
            format!(
                "2026-10-21 10:4{i}:0{i}.112 UTC [1] LOG:  {container}: checkpoint complete: wrote {} buffers",
                12 + i * 7
            )
        })
        .collect();
    let mut bytes = lines.join("\r\n").into_bytes();
    bytes.extend_from_slice(b"\r\n");
    bytes
}

/// What a unit's terminal shows when the TUI attaches.
pub fn screen(unit: &str) -> Vec<u8> {
    let lines: Vec<String> = match unit.split('-').next() {
        Some("web") => vec![
            "\x1b[32m  VITE v6.0.3\x1b[0m  ready in \x1b[1m412\x1b[0m ms".into(),
            String::new(),
            "  \x1b[32m➜\x1b[0m  Local:   \x1b[36mhttps://app.acme.localhost/\x1b[0m".into(),
            "  \x1b[32m➜\x1b[0m  Network: use --host to expose".into(),
            "\x1b[2m10:42:07\x1b[0m \x1b[36m[vite]\x1b[0m hmr update /src/routes/checkout.tsx".into(),
            "\x1b[2m10:42:11\x1b[0m \x1b[36m[vite]\x1b[0m page reload src/main.tsx".into(),
        ],
        Some("wrk") => (0..14)
            .map(|i| {
                format!(
                    "\x1b[2m2026-10-21T10:4{}:{:02}Z\x1b[0m \x1b[32mINFO\x1b[0m job done queue=default id=job_{:04} took={}ms",
                    i / 6,
                    (i * 7) % 60,
                    8_120 + i,
                    40 + i * 13
                )
            })
            .collect(),
        _ => {
            let mut lines = vec![
                "\x1b[2m> @acme/api-gateway@2.4.0 dev\x1b[0m".to_owned(),
                "\x1b[2m> tsx watch src/server.ts\x1b[0m".to_owned(),
                String::new(),
                "\x1b[32m✓\x1b[0m connected to postgres (acme_lane0)".to_owned(),
                "\x1b[32m✓\x1b[0m connected to redis".to_owned(),
                "\x1b[1mlistening on :41000\x1b[0m".to_owned(),
            ];
            for i in 0..18 {
                let status = if i % 9 == 4 { "\x1b[33m404\x1b[0m" } else { "\x1b[32m200\x1b[0m" };
                lines.push(format!(
                    "\x1b[2m10:4{}:{:02}\x1b[0m GET /v1/orders/{} {status} {}ms",
                    2 + i / 10,
                    (i * 11) % 60,
                    1_000 + i * 17,
                    3 + i % 5
                ));
            }
            lines
        }
    };
    lines.join("\r\n").into_bytes()
}

/// Stored log lines of a unit, oldest first, as `logs` returns them.
pub fn log_page(unit: &str) -> Vec<LogLine> {
    (0..60)
        .map(|seq| {
            let warn = seq % 17 == 11;
            let (level, color, rest) = if warn {
                (
                    "WARN",
                    33,
                    format!(
                        "slow query took {}ms: SELECT * FROM orders WHERE lane = $1",
                        900 + seq
                    ),
                )
            } else {
                (
                    "INFO",
                    32,
                    format!(
                        "request id=req_{:05} route=/v1/orders status=200",
                        4_000 + seq
                    ),
                )
            };
            LogLine {
                unit: unit.into(),
                seq,
                timestamp_ms: 1_761_040_000_000 + seq * 2_500,
                text: format!("{level} {rest}"),
                level: Some(level.to_lowercase()),
                styled: Some(format!("\x1b[{color}m{level}\x1b[0m {rest}\x1b[0m")),
            }
        })
        .collect()
}

/// The version of the fake daemon and of the TUI drawn against it, pinned so
/// a release does not change the committed screens.
pub const VERSION: &str = "1.0.0";

/// The fake's clock: every history ends here.
pub const NOW_MS: u64 = 1_792_000_000_000;
/// One sample a second for 15 minutes, the daemon's raw ring.
const STEP_MS: u64 = 1_000;
const POINTS: u64 = 900;

/// A deterministic wobble in 0..1 for sample `t` of series `seed`.
fn wobble(seed: u64, t: u64) -> f64 {
    let x = (seed * 7919 + t * 104_729) % 1000;
    let wave = ((t as f64 + seed as f64 * 15.0) / 45.0).sin() * 0.5 + 0.5;
    (wave * 0.7 + x as f64 / 1000.0 * 0.3).clamp(0.0, 1.0)
}

/// 15 minutes of `(ts, cpu, memory)` around `cpu` and `memory`, with a
/// spike a minute ago when `spike`, so even a narrow graph shows it.
fn history(seed: u64, cpu: f64, memory: u64, spike: bool) -> Vec<(u64, f64, u64)> {
    (0..POINTS)
        .map(|i| {
            let ts = NOW_MS - (POINTS - 1 - i) * STEP_MS;
            let mut c = cpu * (0.4 + wobble(seed, i) * 0.9);
            if spike && (820..860).contains(&i) {
                c += cpu * 2.5 * (1.0 - ((i as f64 - 840.0) / 20.0).abs());
            }
            let m = memory as f64
                * (0.92 + i as f64 / POINTS as f64 * 0.08 + wobble(seed + 1, i) * 0.02);
            (ts, c.max(0.0), m as u64)
        })
        .collect()
}

/// `metrics` as a recent daemon answers it: units and containers with 15
/// minutes of history, lane totals and the machine.
pub fn metrics() -> Metrics {
    let mut units = Vec::new();
    for (seed, unit) in lanes()
        .iter()
        .flat_map(|lane| lane.units.iter())
        .filter(|u| {
            matches!(u.state.as_str(), "ready" | "running" | "starting") || u.state == ORPHANED
        })
        .enumerate()
    {
        let seed = seed as u64;
        let (cpu, memory) = match unit.name.as_str() {
            "api-search" => (96.0, 412 << 20),
            "web-storybook" => (40.0, 419 << 20),
            "api-catalog" => (38.0, 258 << 20),
            "wrk-index" => (12.0, 511 << 20),
            _ => (
                (seed % 9) as f64 * 2.5 + 0.5,
                (60 + (seed * 37) % 400) << 20,
            ),
        };
        let history = history(seed, cpu, memory, unit.name == "api-search");
        units.push(UnitMetrics {
            lane: unit.lane.clone(),
            unit: unit.name.clone(),
            kind: unit.kind.clone(),
            role: unit.role.clone(),
            cpu_percent: history.last().map(|p| p.1),
            memory_bytes: history.last().map(|p| p.2),
            peak_cpu_percent: Some(cpu * 1.5),
            peak_memory_bytes: Some(memory + (memory / 10)),
            history,
            ..UnitMetrics::default()
        });
    }
    // Containers: shared ones once under the project, the rest in their lane.
    let containers = [
        ("acme (shared)", "postgres", 6.0, 598u64 << 20, true),
        ("acme (shared)", "redis", 1.0, 38 << 20, true),
        ("acme@0", "minio", 4.0, 96 << 20, false),
    ];
    for (seed, (lane, name, cpu, memory, shared)) in containers.into_iter().enumerate() {
        let history = history(100 + seed as u64, cpu, memory, false);
        units.push(UnitMetrics {
            lane: lane.into(),
            unit: name.into(),
            kind: "container".into(),
            container: Some(format!("acme-{name}-1")),
            shared,
            lanes: if shared {
                vec!["acme@0".into(), "acme@1".into()]
            } else {
                Vec::new()
            },
            cpu_percent: history.last().map(|p| p.1),
            memory_bytes: history.last().map(|p| p.2),
            history,
            ..UnitMetrics::default()
        });
    }
    // Lane totals: their units and their own containers, never shared ones.
    let mut lane_names: Vec<String> = units
        .iter()
        .filter(|u| !u.shared)
        .map(|u| u.lane.clone())
        .collect();
    lane_names.sort();
    lane_names.dedup();
    let lanes = lane_names
        .into_iter()
        .map(|lane| {
            let members: Vec<&UnitMetrics> = units
                .iter()
                .filter(|u| u.lane == lane && !u.shared)
                .collect();
            let history: Vec<(u64, f64, u64)> = (0..POINTS as usize)
                .map(|i| {
                    let ts = members.first().map_or(0, |m| m.history[i].0);
                    let cpu = members.iter().map(|m| m.history[i].1).sum();
                    let memory = members.iter().map(|m| m.history[i].2).sum();
                    (ts, cpu, memory)
                })
                .collect();
            LaneMetrics {
                lane,
                cpu_percent: history.last().map(|p| p.1),
                memory_bytes: history.last().map(|p| p.2),
                peak_cpu_percent: history.iter().map(|p| p.1).reduce(f64::max),
                peak_memory_bytes: history.iter().map(|p| p.2).max(),
                history,
                ..LaneMetrics::default()
            }
        })
        .collect();
    let total: u64 = 32 << 30;
    let system = SystemMetrics {
        cpus: Some(10),
        memory_total_bytes: Some(total),
        available_memory_bytes: Some(9_800 << 20),
        cpu_percent: Some(72.0),
        memory_pressure: Some("normal".into()),
        cpu_stall_percent: None,
        core_percents: vec![88.0, 76.0, 64.0, 92.0, 41.0, 35.0, 18.0, 12.0, 7.0, 3.0],
        history: (0..POINTS)
            .map(|i| {
                let ts = NOW_MS - (POINTS - 1 - i) * STEP_MS;
                (ts, 45.0 + wobble(500, i) * 35.0, 9_800 << 20)
            })
            .collect(),
        available_history: (0..POINTS)
            .map(|i| (NOW_MS - (POINTS - 1 - i) * STEP_MS, 9_800 << 20))
            .collect(),
        // Fifteen seconds of `warn` that every memory graph reaches back to.
        pressure_history: (0..POINTS)
            .map(|i| {
                let level = if (POINTS - 50..POINTS - 35).contains(&i) {
                    1.0
                } else {
                    0.0
                };
                (NOW_MS - (POINTS - 1 - i) * STEP_MS, level)
            })
            .collect(),
    };
    let daemon = history(300, 1.2, 46 << 20, false);
    Metrics {
        range_ms: Some(900_000),
        units,
        lanes,
        system: Some(system),
        daemon: Some(Box::new(DaemonMetrics {
            pid: Some(4242),
            started_at_ms: Some(NOW_MS - 3 * 3_600_000),
            metrics: UnitMetrics {
                lane: "devlanes".into(),
                unit: "daemon".into(),
                kind: "daemon".into(),
                cpu_percent: daemon.last().map(|p| p.1),
                memory_bytes: daemon.last().map(|p| p.2),
                peak_cpu_percent: Some(4.0),
                peak_memory_bytes: Some(52 << 20),
                history: daemon,
                ..UnitMetrics::default()
            },
        })),
    }
}

/// [`metrics`] after acme@2 stopped 40 seconds ago and an earlier acme@1,
/// cleaned 80 seconds ago, gave its reference to the acme@1 that starts now.
/// Their rows come back ended, without readings, with the history they had.
pub fn metrics_with_ended() -> Metrics {
    let mut metrics = metrics();
    let rows = [
        ("acme@2", "api-gateway", 2, 40, 18.0, 210u64 << 20, false),
        ("acme@2", "web-app", 2, 40, 9.0, 330 << 20, false),
        ("acme@1", "web-app", 7, 80, 24.0, 280 << 20, true),
        ("acme@1", "api-orders", 7, 80, 30.0, 190 << 20, true),
    ];
    let mut ended: Vec<UnitMetrics> = Vec::new();
    for (seed, (lane, name, id, gone_s, cpu, memory, removed)) in rows.into_iter().enumerate() {
        let mut history = history(200 + seed as u64, cpu, memory, false);
        history.truncate(history.len() - gone_s);
        ended.push(UnitMetrics {
            lane: lane.into(),
            unit: name.into(),
            kind: "service".into(),
            // Removed rows lose their peaks with their lane.
            peak_cpu_percent: (!removed).then_some(cpu * 1.5),
            peak_memory_bytes: (!removed).then_some(memory + memory / 10),
            history,
            series: Some(format!("unit:1:{id}:{name}")),
            ended: true,
            removed,
            ..UnitMetrics::default()
        });
    }
    for (lane, id, removed) in [("acme@2", 2, false), ("acme@1", 7, true)] {
        let members: Vec<&UnitMetrics> = ended
            .iter()
            .filter(|u| u.lane == lane && u.removed == removed)
            .collect();
        let points = members.first().map_or(0, |m| m.history.len());
        let history = (0..points)
            .map(|i| {
                let ts = members[0].history[i].0;
                let cpu = members.iter().map(|m| m.history[i].1).sum();
                let memory = members.iter().map(|m| m.history[i].2).sum();
                (ts, cpu, memory)
            })
            .collect();
        metrics.lanes.push(LaneMetrics {
            lane: lane.into(),
            history,
            series: Some(format!("lane:1:{id}")),
            ended: true,
            removed,
            ..LaneMetrics::default()
        });
    }
    metrics.units.extend(ended);
    metrics
}

/// `profile.list` for the monorepo.
pub fn profiles(lane: &str) -> ProfileList {
    let profile = |name: &str, include: &[&str], skips_infra, is_default| ProfileInfo {
        name: name.into(),
        include: include.iter().map(|s| (*s).to_owned()).collect(),
        exclude: Vec::new(),
        skips_infra,
        is_default,
    };
    ProfileList {
        lane: lane.into(),
        items: vec![
            profile("web", &["+web", "+api", "proxy-edge"], false, true),
            profile("workers", &["+workers", "stub-*"], false, false),
            profile(
                "e2e",
                &["+web", "+api", "+workers", "test-e2e"],
                false,
                false,
            ),
            profile("ml", &["ml-*", "api-search"], true, false),
        ],
    }
}

/// `daemon.status`.
pub fn daemon_status() -> Value {
    json!({
        "version": VERSION,
        "pid": 4242,
        "uptime_ms": 27_500_000,
        "managed_processes": 41,
        "socket": "/Users/acme/.devlanes/run/daemon.sock",
    })
}

/// `doctor` facts: one warning (fallback port) and one error (no CA).
pub fn doctor() -> Value {
    json!({
        "version": VERSION,
        "pid": 4242,
        "home": "/Users/acme/.devlanes",
        "db_path": "/Users/acme/.devlanes/state.db",
        "schema_version": 7,
        "supported_schema_version": 7,
        "socket": "/Users/acme/.devlanes/run/daemon.sock",
        "sandbox": {"codex_sandbox": false, "app_sandbox": false},
        "infra": {
            "docker": {"reachable": true, "version": "27.1.1"},
            "compose": {"version": "2.29.1", "supports_lane_overrides": true}
        },
        "proxy": {
            "http": {"bound": true, "port": 80},
            "https": {"bound": true, "port": 8443, "fallback_reason": "port 443 needs root"}
        },
        "ca": {"present": false, "error": "no CA has been created yet"},
        "projects": 2,
        "subscribers": 3,
        "hosts": {"mode": "auto", "probe": "localhost_resolves", "state": "applied", "names": 38}
    })
}

/// The daemon's own log file.
pub fn daemon_log() -> Vec<String> {
    let mut lines = Vec::new();
    for i in 0..40 {
        lines.push(match i % 8 {
            3 => format!("2026-10-21T10:{:02}:12.120Z  \x1b[33mWARN\x1b[0m devlanes_daemon::gate: start window full lane=acme@1 queued=2", i),
            6 => format!("2026-10-21T10:{:02}:40.002Z \x1b[31mERROR\x1b[0m devlanes_daemon::lifecycle: unit exited unit=acme@0:wrk-export code=1", i),
            _ => format!("2026-10-21T10:{:02}:0{}.503Z  INFO devlanes_daemon::lifecycle: unit ready unit=acme@0:api-{} took=1.{}s", i, i % 10, ["gateway", "auth", "users", "orders"][i % 4], i % 9),
        });
    }
    lines
}
