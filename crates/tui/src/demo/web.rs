//! The website's scene: one small project, `myapp`, with four lanes of the
//! same three services and shared infra, everything healthy. The web demo
//! (crates/tui-web) runs on it; the committed screens keep [`super::data`].

use std::collections::BTreeMap;

use devlanes_proto::{Actor, ActorKind, LaneSummary, LogLine};
use serde_json::{Value, json};

use super::data::{self, containers, history, metrics_with, unit};
use crate::{
    Metrics, UnitMetrics,
    api::{ProfileList, TaskInfo},
};

/// The project every lane belongs to.
const PROJECT: &str = "myapp";

/// The services of every lane.
const SERVICES: [&str; 3] = ["api", "web", "worker"];

/// The lanes: index, branch, worktree, and whether they run.
const LANES: [(u16, &str, &str, bool); 4] = [
    (0, "main", "/work/myapp", true),
    (1, "feat/checkout", "/work/myapp-checkout", true),
    (2, "fix/login", "/work/myapp-login", false),
    (3, "agent/perf", "/work/myapp-perf", true),
];

/// The lane an agent started, whose units say so.
const AGENT_LANE: u16 = 3;

/// The worktree the TUI opens on.
pub const PATH: &str = "/work/myapp";

/// `myapp@<index>`.
fn reference(index: u16) -> String {
    format!("{PROJECT}@{index}")
}

/// The hostname of `service` in lane `index`: `myapp.localhost` for lane 0's
/// web, `api.myapp2.localhost` for lane 2's api.
fn host(index: u16, service: &str) -> String {
    let domain = match index {
        0 => format!("{PROJECT}.localhost"),
        _ => format!("{PROJECT}{index}.localhost"),
    };
    match service {
        "web" => domain,
        _ => format!("{service}.{domain}"),
    }
}

/// One lane of the project.
fn lane(index: u16, branch: &str, path: &str, running: bool) -> LaneSummary {
    let reference = reference(index);
    let units = SERVICES
        .iter()
        .enumerate()
        .map(|(position, name)| {
            let state = match (running, *name) {
                (false, _) => "idle",
                (true, "worker") => "running",
                (true, _) => "ready",
            };
            let mut unit = unit(&reference, name, state, "service");
            let port = 41_000 + index * 100 + u16::try_from(position).unwrap_or(0);
            unit.ports = BTreeMap::from([("http".to_owned(), port)]);
            if *name == "worker" {
                // A worker has no readiness check: a live process shows as running.
                unit.ready_check = Some("started".into());
            } else {
                unit.urls = vec![format!("https://{}", host(index, name))];
            }
            if running {
                unit.pid =
                    Some(51_000 + u32::from(index) * 10 + u32::try_from(position).unwrap_or(0));
            }
            if index == AGENT_LANE {
                unit.actor = Actor {
                    kind: ActorKind::Agent,
                    agent_id: Some("claude".into()),
                    session_id: None,
                };
            }
            unit
        })
        .collect();
    LaneSummary {
        reference: reference.clone(),
        project: PROJECT.into(),
        index: Some(u32::from(index)),
        branch: Some(branch.into()),
        path: path.into(),
        state: if running { "running" } else { "idle" }.into(),
        units,
        containers: containers(&[("postgres", 5432, true), ("redis", 6379, true)]),
        domain: Some(host(index, "web")),
        port_block: Some((41_000 + index * 100, 41_099 + index * 100)),
        ..LaneSummary::default()
    }
}

/// Every lane the website's daemon reports.
pub fn lanes() -> Vec<LaneSummary> {
    LANES
        .iter()
        .map(|(index, branch, path, running)| lane(*index, branch, path, *running))
        .collect()
}

/// `metrics` `step` seconds on: the running lanes' services, the agent's
/// api under load with a spike, and the shared postgres and redis.
pub fn metrics_at(step: u64) -> Metrics {
    let mut units = Vec::new();
    for (seed, unit) in lanes()
        .iter()
        .flat_map(|lane| lane.units.iter())
        .filter(|unit| unit.state != "idle")
        .enumerate()
    {
        let agent = unit.lane == reference(AGENT_LANE);
        let (cpu, memory) = match (unit.name.as_str(), agent) {
            ("api", true) => (31.0, 288 << 20),
            ("api", false) => (4.0 + seed as f64, 118 << 20),
            ("web", _) => (3.2, 110 << 20),
            _ => (1.7, 72 << 20),
        };
        let history = history(seed as u64, cpu, memory, agent && unit.name == "api", step);
        units.push(UnitMetrics {
            lane: unit.lane.clone(),
            unit: unit.name.clone(),
            kind: unit.kind.clone(),
            cpu_percent: history.last().map(|p| p.1),
            memory_bytes: history.last().map(|p| p.2),
            peak_cpu_percent: Some(cpu * 1.5),
            peak_memory_bytes: Some(memory + memory / 4),
            history,
            ..UnitMetrics::default()
        });
    }
    let running: Vec<String> = LANES
        .iter()
        .filter(|lane| lane.3)
        .map(|lane| reference(lane.0))
        .collect();
    for (seed, (name, cpu, memory)) in [("postgres", 4.9, 602u64 << 20), ("redis", 0.8, 38 << 20)]
        .into_iter()
        .enumerate()
    {
        let history = history(100 + seed as u64, cpu, memory, false, step);
        units.push(UnitMetrics {
            lane: format!("{PROJECT} (shared)"),
            unit: name.into(),
            kind: "container".into(),
            container: Some(format!("{PROJECT}-{name}-1")),
            shared: true,
            lanes: running.clone(),
            cpu_percent: history.last().map(|p| p.1),
            memory_bytes: history.last().map(|p| p.2),
            history,
            ..UnitMetrics::default()
        });
    }
    metrics_with(units, step)
}

/// `task.list`: the project's tasks.
pub fn tasks(_lane: &str) -> Vec<TaskInfo> {
    ["migrate", "seed", "test"]
        .into_iter()
        .map(|name| TaskInfo {
            name: name.into(),
            when: (name == "migrate").then(|| "lane.start".into()),
            confirm: name == "seed",
            ..TaskInfo::default()
        })
        .collect()
}

/// `profile.list`: the project defines none.
pub fn profiles(lane: &str) -> ProfileList {
    ProfileList {
        lane: lane.into(),
        items: Vec::new(),
    }
}

/// `daemon.status`, from the release the demo was built from, so no update
/// or version notice shows.
pub fn daemon_status() -> Value {
    json!({
        "version": devlanes_version::VERSION,
        "pid": 4242,
        "uptime_ms": 11_400_000,
        "managed_processes": 9,
        "socket": "/home/dev/.devlanes/run/daemon.sock",
    })
}

/// `doctor` facts with every check passing.
pub fn doctor() -> Value {
    json!({
        "version": devlanes_version::VERSION,
        "pid": 4242,
        "home": "/home/dev/.devlanes",
        "db_path": "/home/dev/.devlanes/state.db",
        "schema_version": 7,
        "supported_schema_version": 7,
        "socket": "/home/dev/.devlanes/run/daemon.sock",
        "sandbox": {"codex_sandbox": false, "app_sandbox": false},
        "infra": {
            "docker": {"reachable": true, "version": "27.1.1"},
            "compose": {"version": "2.29.1", "supports_lane_overrides": true}
        },
        "proxy": {
            "http": {"bound": true, "port": 80},
            "https": {"bound": true, "port": 443}
        },
        "ca": {
            "present": true,
            "path": "/home/dev/.devlanes/ca/root.pem",
            "trust": [{"store": "system", "state": "trusted"}]
        },
        "projects": 1,
        "subscribers": 1,
        "hosts": {"mode": "auto", "probe": "localhost_resolves", "state": "applied", "names": 12}
    })
}

/// What a service's terminal shows when the TUI attaches.
pub fn screen(unit: &str) -> Vec<u8> {
    let lines: Vec<String> = match unit {
        "web" => vec![
            "\x1b[32m  VITE v6.0.3\x1b[0m  ready in \x1b[1m388\x1b[0m ms".into(),
            String::new(),
            "  \x1b[32m➜\x1b[0m  Local:   \x1b[36mhttps://myapp.localhost/\x1b[0m".into(),
            "  \x1b[32m➜\x1b[0m  Network: use --host to expose".into(),
            "\x1b[2m10:42:07\x1b[0m \x1b[36m[vite]\x1b[0m hmr update /src/routes/checkout.tsx".into(),
            "\x1b[2m10:42:11\x1b[0m \x1b[36m[vite]\x1b[0m page reload src/main.tsx".into(),
        ],
        "worker" => (0..14)
            .map(|i| {
                format!(
                    "\x1b[2m10:4{}:{:02}\x1b[0m \x1b[32mINFO\x1b[0m job done queue=emails id=job_{:04} took={}ms",
                    i / 6,
                    (i * 7) % 60,
                    2_310 + i,
                    40 + i * 13
                )
            })
            .collect(),
        _ => {
            let mut lines = vec![
                "\x1b[2m> myapp-api@1.4.0 dev\x1b[0m".to_owned(),
                "\x1b[2m> tsx watch src/server.ts\x1b[0m".to_owned(),
                String::new(),
                "\x1b[32m✓\x1b[0m connected to postgres (myapp_lane0)".to_owned(),
                "\x1b[32m✓\x1b[0m connected to redis".to_owned(),
                "\x1b[1mlistening on :41000\x1b[0m".to_owned(),
            ];
            for i in 0..18 {
                lines.push(format!(
                    "\x1b[2m10:4{}:{:02}\x1b[0m GET /v1/orders/{} \x1b[32m200\x1b[0m {}ms",
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

/// Stored log lines of a service, oldest first.
pub fn log_page(unit: &str) -> Vec<LogLine> {
    data::log_page(unit)
}

/// The daemon's own log file: the running lanes' units getting ready.
pub fn daemon_log() -> Vec<String> {
    let running: Vec<u16> = LANES
        .iter()
        .filter(|lane| lane.3)
        .map(|lane| lane.0)
        .collect();
    (0..40)
        .map(|i| {
            let lane = running[i % running.len()];
            let service = SERVICES[i % 3];
            format!(
                "2026-10-21T10:{:02}:0{}.503Z  INFO devlanes_daemon::lifecycle: unit ready unit={}:{service} took=1.{}s",
                i,
                i % 10,
                reference(lane),
                i % 9
            )
        })
        .collect()
}
