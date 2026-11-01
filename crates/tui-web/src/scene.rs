//! What the demo's fake daemon reports: one small project, `myapp`, with four
//! lanes of the same three services, shared Postgres and Redis and a mail
//! catcher per lane, everything healthy and at this release. Each lane's
//! output names its own database, Redis index and ports, as `lane.name` and
//! `lane.index` give them, and new lines come in every second.

use std::collections::BTreeMap;

use devlanes_proto::{Actor, ActorKind, ContainerSummary, LaneSummary, LogLine, UnitSummary};
use devlanes_tui::{
    DaemonMetrics, LaneMetrics, Metrics, SystemMetrics, UnitMetrics,
    api::{ProfileList, TaskInfo},
};
use serde_json::{Value, json};

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

/// The lane an agent started, whose units say so and whose api is under load.
const AGENT_LANE: u16 = 3;

/// The container each lane runs for itself: a mail catcher.
const LANE_CONTAINER: &str = "mailpit";

/// The worktree the TUI opens on.
pub const PATH: &str = "/work/myapp";

/// The fake daemon's clock at the start: every history ends here.
pub const NOW_MS: u64 = 1_792_000_000_000;

/// Seconds after midnight of the last line on every unit's first screen;
/// live lines carry this plus the fake's step.
const CLOCK: u64 = 10 * 3600 + 42 * 60;

/// One sample a second for 15 minutes, the daemon's raw ring.
const STEP_MS: u64 = 1_000;
const POINTS: u64 = 900;

/// `myapp@<index>`.
fn reference(index: u16) -> String {
    format!("{PROJECT}@{index}")
}

/// The lane index of `myapp@<index>`.
fn index_of(lane: &str) -> u16 {
    lane.rsplit_once('@')
        .and_then(|(_, index)| index.parse().ok())
        .unwrap_or(0)
}

/// `lane.name`: `main` for lane 0, `lane<N>` for the others.
fn lane_name(index: u16) -> String {
    match index {
        0 => "main".into(),
        _ => format!("lane{index}"),
    }
}

/// The lane's database, as `db_name: myapp_${{ lane.name }}` gives it.
fn database(index: u16) -> String {
    format!("{PROJECT}_{}", lane_name(index))
}

/// The port of the service at `position` in the lane's block.
fn port(index: u16, position: u16) -> u16 {
    41_000 + index * 100 + position
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

/// A unit with the fields a daemon fills for every unit, colored by its
/// name as the daemon colors untagged units; live ones started 2h 14m
/// before the fake's clock.
pub fn unit(lane: &str, name: &str, state: &str, kind: &str) -> UnitSummary {
    let color = devlanes_core::color::unit_color(&[], name, None, &BTreeMap::new());
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

/// Healthy containers by name, host port and whether they are shared.
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

/// One lane of the project.
fn lane(index: u16, branch: &str, path: &str, running: bool) -> LaneSummary {
    let reference = reference(index);
    let units = SERVICES
        .iter()
        .zip(0_u16..)
        .map(|(name, position)| {
            let state = match (running, *name) {
                (false, _) => "idle",
                (true, "worker") => "running",
                (true, _) => "ready",
            };
            let mut unit = unit(&reference, name, state, "service");
            unit.ports = BTreeMap::from([("http".to_owned(), port(index, position))]);
            // A worker has no readiness check: a live process shows as running.
            if *name == "worker" {
                unit.ready_check = Some("started".into());
            } else {
                unit.urls = vec![format!("https://{}", host(index, name))];
            }
            if running {
                unit.pid = Some(51_000 + u32::from(index) * 10 + u32::from(position));
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
    // A lane that never started has no containers of its own yet.
    let own = (LANE_CONTAINER, 18_025 + index * 100, false);
    let containers = if running {
        containers(&[("postgres", 5432, true), ("redis", 6379, true), own])
    } else {
        containers(&[("postgres", 5432, true), ("redis", 6379, true)])
    };
    LaneSummary {
        reference: reference.clone(),
        project: PROJECT.into(),
        index: Some(u32::from(index)),
        branch: Some(branch.into()),
        path: path.into(),
        state: if running { "running" } else { "idle" }.into(),
        units,
        containers,
        domain: Some(host(index, "web")),
        port_block: Some((port(index, 0), port(index, 99))),
        ..LaneSummary::default()
    }
}

/// Every lane the daemon reports.
pub fn lanes() -> Vec<LaneSummary> {
    LANES
        .iter()
        .map(|(index, branch, path, running)| lane(*index, branch, path, *running))
        .collect()
}

// ---- metrics --------------------------------------------------------------

/// A deterministic wobble in 0..1 for sample `t` of series `seed`.
fn wobble(seed: u64, t: u64) -> f64 {
    let x = (seed * 7919 + t * 104_729) % 1000;
    let wave = ((t as f64 + seed as f64 * 15.0) / 45.0).sin() * 0.5 + 0.5;
    (wave * 0.7 + x as f64 / 1000.0 * 0.3).clamp(0.0, 1.0)
}

/// The timestamp of sample `i` of a history that ends `step` seconds after
/// [`NOW_MS`].
fn sample_ts(step: u64, i: u64) -> u64 {
    NOW_MS + step * STEP_MS - (POINTS - 1 - i) * STEP_MS
}

/// 15 minutes of `(ts, cpu, memory)` around `cpu` and `memory`, ending
/// `step` seconds after [`NOW_MS`], with a spike in the last minute of every
/// 15 when `spike`.
fn history(seed: u64, cpu: f64, memory: u64, spike: bool, step: u64) -> Vec<(u64, f64, u64)> {
    (0..POINTS)
        .map(|i| {
            // Counting samples from the start of time, not the window, makes
            // the curve move left as `step` grows instead of being redrawn.
            let n = i + step;
            let mut c = cpu * (0.4 + wobble(seed, n) * 0.9);
            let phase = n % POINTS;
            if spike && (820..860).contains(&phase) {
                c += cpu * 2.5 * (1.0 - ((phase as f64 - 840.0) / 20.0).abs());
            }
            let growth = phase as f64 / POINTS as f64 * 0.08;
            let m = memory as f64 * (0.92 + growth + wobble(seed + 1, n) * 0.02);
            (sample_ts(step, i), c.max(0.0), m as u64)
        })
        .collect()
}

/// `metrics` `step` seconds on: the running lanes' services and mail
/// catchers, the agent's api under load with a spike, the shared Postgres
/// and Redis, lane totals, the machine and `daemon`.
pub fn metrics_at(step: u64, daemon: Daemon) -> Metrics {
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
    let running: Vec<u16> = LANES
        .iter()
        .filter(|lane| lane.3)
        .map(|lane| lane.0)
        .collect();
    let mut infra: Vec<(String, &str, f64, u64, bool)> = vec![
        (
            format!("{PROJECT} (shared)"),
            "postgres",
            4.9,
            602 << 20,
            true,
        ),
        (format!("{PROJECT} (shared)"), "redis", 0.8, 38 << 20, true),
    ];
    for index in &running {
        infra.push((reference(*index), LANE_CONTAINER, 0.3, 21 << 20, false));
    }
    for (seed, (lane, name, cpu, memory, shared)) in infra.into_iter().enumerate() {
        let history = history(100 + seed as u64, cpu, memory, false, step);
        let owner = if shared {
            PROJECT.to_owned()
        } else {
            format!("{PROJECT}{}", index_of(&lane))
        };
        units.push(UnitMetrics {
            container: Some(format!("{owner}-{name}-1")),
            lane,
            unit: name.into(),
            kind: "container".into(),
            shared,
            lanes: if shared {
                running.iter().map(|index| reference(*index)).collect()
            } else {
                Vec::new()
            },
            cpu_percent: history.last().map(|p| p.1),
            memory_bytes: history.last().map(|p| p.2),
            history,
            ..UnitMetrics::default()
        });
    }
    Metrics {
        range_ms: Some(900_000),
        lanes: lane_totals(&units),
        units,
        system: Some(system(step)),
        daemon: Some(Box::new(DaemonMetrics {
            pid: Some(daemon.pid),
            started_at_ms: Some(daemon.started_at_ms),
            metrics: daemon_metrics(step),
        })),
    }
}

/// Each lane's totals: its units and its own containers, never shared ones.
fn lane_totals(units: &[UnitMetrics]) -> Vec<LaneMetrics> {
    let mut names: Vec<&str> = units
        .iter()
        .filter(|u| !u.shared)
        .map(|u| u.lane.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
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
                lane: lane.into(),
                cpu_percent: history.last().map(|p| p.1),
                memory_bytes: history.last().map(|p| p.2),
                peak_cpu_percent: history.iter().map(|p| p.1).reduce(f64::max),
                peak_memory_bytes: history.iter().map(|p| p.2).max(),
                history,
                ..LaneMetrics::default()
            }
        })
        .collect()
}

/// The machine: 10 cores, 32 GiB, busy but not short of memory.
fn system(step: u64) -> SystemMetrics {
    SystemMetrics {
        cpus: Some(10),
        memory_total_bytes: Some(32 << 30),
        available_memory_bytes: Some(9_800 << 20),
        cpu_percent: Some(72.0),
        memory_pressure: Some("normal".into()),
        cpu_stall_percent: None,
        core_percents: vec![88.0, 76.0, 64.0, 92.0, 41.0, 35.0, 18.0, 12.0, 7.0, 3.0],
        history: (0..POINTS)
            .map(|i| {
                let cpu = 45.0 + wobble(500, i + step) * 35.0;
                (sample_ts(step, i), cpu, 9_800 << 20)
            })
            .collect(),
        available_history: (0..POINTS)
            .map(|i| (sample_ts(step, i), 9_800 << 20))
            .collect(),
        pressure_history: (0..POINTS).map(|i| (sample_ts(step, i), 0.0)).collect(),
    }
}

/// The daemon's own row.
fn daemon_metrics(step: u64) -> UnitMetrics {
    let history = history(300, 1.2, 46 << 20, false, step);
    UnitMetrics {
        lane: "devlanes".into(),
        unit: "daemon".into(),
        kind: "daemon".into(),
        cpu_percent: history.last().map(|p| p.1),
        memory_bytes: history.last().map(|p| p.2),
        peak_cpu_percent: Some(4.0),
        peak_memory_bytes: Some(52 << 20),
        history,
        ..UnitMetrics::default()
    }
}

// ---- daemon facts ---------------------------------------------------------

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

/// The daemon process: its pid and when it started.
#[derive(Clone, Copy)]
pub struct Daemon {
    pub pid: u32,
    pub started_at_ms: u64,
}

impl Default for Daemon {
    /// The daemon the scene opens on, up for 3h 10m.
    fn default() -> Self {
        Self {
            pid: 4242,
            started_at_ms: NOW_MS - 11_400_000,
        }
    }
}

/// `daemon.status` of `daemon` at `now_ms`, from the release the demo was
/// built from, so no update or version notice shows.
pub fn daemon_status(daemon: Daemon, now_ms: u64) -> Value {
    json!({
        "version": devlanes_version::VERSION,
        "pid": daemon.pid,
        "uptime_ms": now_ms.saturating_sub(daemon.started_at_ms),
        "managed_processes": 12,
        "socket": "/home/dev/.devlanes/run/daemon.sock",
    })
}

/// `doctor` facts of `daemon` with every check passing.
pub fn doctor(daemon: Daemon) -> Value {
    json!({
        "version": devlanes_version::VERSION,
        "pid": daemon.pid,
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

/// `remote.clients`: nobody paired with this machine.
pub fn clients() -> Value {
    json!({"items": [], "now_ms": NOW_MS, "require_pairing": true, "allow_connections": true})
}

/// `remote.pair`'s code, valid for 10 minutes.
pub fn pair_code() -> Value {
    let valid = devlanes_proto::remote::PAIRING_CODE_TTL_MS;
    json!({"code": "K7QM-3XWD", "expires_at_ms": NOW_MS + valid, "valid_ms": valid})
}

/// The user config the options page reads.
pub const USER_CONFIG: &str = "\
# machine settings for devlanes
tui:
  theme: dark
proxy:
  https_port: 443
history:
  retention: 14d
";

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

// ---- output ---------------------------------------------------------------

/// A number that looks random but is the same for the same lane, unit and
/// second, so a run of the demo is reproducible (FNV-1a).
fn noise(lane: &str, unit: &str, step: u64) -> u64 {
    lane.bytes()
        .chain(unit.bytes())
        .chain(step.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// `seconds` after midnight as `10:42:07`.
fn clock(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// A dimmed timestamp, as the services print them.
fn stamp(seconds: u64) -> String {
    format!("\x1b[2m{}\x1b[0m", clock(seconds))
}

/// One request line of lane `index`'s api, from the number `n`.
fn request(index: u16, n: u64, seconds: u64) -> String {
    let (method, path, base_ms) = match index {
        1 => {
            let routes = [
                ("POST", "/v1/checkout/sessions", 18),
                ("GET", "/v1/cart", 4),
                ("POST", "/v1/payments/intent", 42),
                ("GET", "/v1/orders/latest", 6),
            ];
            routes[(n % 4) as usize]
        }
        AGENT_LANE => ("GET", "/v1/orders?limit=500", 160),
        _ => {
            let routes = [
                ("GET", "/v1/orders", 3),
                ("GET", "/v1/users/me", 2),
                ("POST", "/v1/orders", 9),
                ("GET", "/healthz", 1),
            ];
            routes[(n % 4) as usize]
        }
    };
    let status = if n % 23 == 7 {
        "\x1b[33m404\x1b[0m"
    } else {
        "\x1b[32m200\x1b[0m"
    };
    format!(
        "{} {method} {path} {status} {}ms",
        stamp(seconds),
        base_ms + n % 7 * (1 + u64::from(index == AGENT_LANE) * 15)
    )
}

/// One finished job of lane `index`'s worker.
fn job(index: u16, n: u64, seconds: u64) -> String {
    let queue = match index {
        1 => "checkout.receipts",
        AGENT_LANE => "reports",
        _ => "emails",
    };
    format!(
        "{} \x1b[32mINFO\x1b[0m job done queue={queue} id=job_{}{:04} took={}ms",
        stamp(seconds),
        index,
        n % 10_000,
        40 + n % 90
    )
}

/// What a service's terminal shows when the TUI attaches: how it started
/// in this lane, then its recent output.
pub fn screen(lane: &str, unit: &str) -> String {
    let index = index_of(lane);
    let first = CLOCK - 18 * 7;
    let lines: Vec<String> = match unit {
        "web" => {
            let route = match index {
                1 => "/src/routes/checkout.tsx",
                2 => "/src/routes/login.tsx",
                AGENT_LANE => "/src/routes/orders.tsx",
                _ => "/src/routes/index.tsx",
            };
            vec![
                "\x1b[2m> myapp-web@1.4.0 dev\x1b[0m".into(),
                format!("\x1b[2m> vite --port {}\x1b[0m", port(index, 1)),
                String::new(),
                "\x1b[32m  VITE v6.0.3\x1b[0m  ready in \x1b[1m388\x1b[0m ms".into(),
                String::new(),
                format!(
                    "  \x1b[32m➜\x1b[0m  Local:   \x1b[36mhttps://{}/\x1b[0m",
                    host(index, "web")
                ),
                "  \x1b[32m➜\x1b[0m  Network: use --host to expose".into(),
                format!(
                    "{} \x1b[36m[vite]\x1b[0m hmr update {route}",
                    stamp(CLOCK - 40)
                ),
                format!(
                    "{} \x1b[36m[vite]\x1b[0m hmr update {route}",
                    stamp(CLOCK - 3)
                ),
            ]
        }
        "worker" => std::iter::once("\x1b[2m> myapp-worker@1.4.0 dev\x1b[0m".to_owned())
            .chain(std::iter::once(format!(
                "\x1b[32m✓\x1b[0m connected to redis (db {index})"
            )))
            .chain((0..12_u64).map(|i| job(index, noise(lane, unit, i), first + i * 10)))
            .collect(),
        _ => {
            let mut lines = vec![
                "\x1b[2m> myapp-api@1.4.0 dev\x1b[0m".to_owned(),
                "\x1b[2m> tsx watch src/server.ts\x1b[0m".to_owned(),
                String::new(),
                format!(
                    "\x1b[32m✓\x1b[0m connected to postgres ({})",
                    database(index)
                ),
                format!("\x1b[32m✓\x1b[0m connected to redis (db {index})"),
                format!("\x1b[1mlistening on :{}\x1b[0m", port(index, 0)),
            ];
            lines.extend((0..18_u64).map(|i| request(index, noise(lane, unit, i), first + i * 7)));
            lines
        }
    };
    lines.join("\r\n")
}

/// Lines service `unit` of `lane` prints at second `step`: requests most
/// seconds (several under the agent's load test), a pool or cache line now
/// and then, jobs for the worker, rare reloads for the web server.
pub fn output(lane: &str, unit: &str, step: u64) -> Vec<String> {
    let index = index_of(lane);
    let roll = noise(lane, unit, 1_000 + step);
    let now = CLOCK + step;
    match unit {
        "api" => {
            let count = match index {
                AGENT_LANE => 2 + roll % 3,
                _ => roll % 3,
            };
            let mut lines: Vec<String> = (0..count)
                .map(|n| request(index, noise(lane, unit, step * 8 + n), now))
                .collect();
            match roll % 17 {
                3 => lines.push(format!(
                    "{} pg pool: {} of 10 connections in use ({})",
                    stamp(now),
                    2 + roll % 5,
                    database(index)
                )),
                9 => lines.push(format!(
                    "{} redis: cache {} key=orders:{}",
                    stamp(now),
                    if roll.is_multiple_of(2) {
                        "hit"
                    } else {
                        "miss"
                    },
                    roll % 900
                )),
                _ => {}
            }
            lines
        }
        "worker" if roll.is_multiple_of(3) => vec![job(index, roll, now)],
        "web" if roll.is_multiple_of(20) => vec![format!(
            "{} \x1b[36m[vite]\x1b[0m page reload src/main.tsx",
            stamp(now)
        )],
        _ => Vec::new(),
    }
}

/// An infra container's Docker log as lane `lane` sees it: the shared
/// Postgres and Redis with this lane's database and index, and the lane's
/// own mail catcher.
pub fn container_logs(lane: &str, container: &str) -> Vec<u8> {
    let index = index_of(lane);
    let lines: Vec<String> = (0..8_u64)
        .map(|i| {
            container_line(
                index,
                container,
                noise(lane, container, i),
                CLOCK - 80 + i * 10,
            )
        })
        .collect();
    // No line break after the last line: live lines start with one.
    lines.join("\r\n").into_bytes()
}

/// One log line of `container` in lane `index`, from the number `n`.
fn container_line(index: u16, container: &str, n: u64, seconds: u64) -> String {
    let at = format!("2026-10-21 {}.{:03} UTC", clock(seconds), n % 1000);
    match container {
        "postgres" => match n % 4 {
            0 => format!(
                "{at} [{}] LOG:  connection authorized: user=postgres database={} application_name=myapp-api",
                200 + n % 700,
                database(index)
            ),
            1 => format!(
                "{at} [{}] LOG:  duration: {}.{} ms  statement: SELECT * FROM orders WHERE id = $1",
                200 + n % 700,
                1 + n % 9,
                n % 10
            ),
            2 => format!(
                "{at} [{}] LOG:  disconnection: session time: 0:00:0{}.{:03} user=postgres database={}",
                200 + n % 700,
                1 + n % 8,
                n % 1000,
                database(index)
            ),
            _ => format!(
                "{at} [27] LOG:  checkpoint complete: wrote {} buffers",
                12 + n % 60
            ),
        },
        "redis" => match n % 3 {
            0 => format!(
                "1:M {} * DB {index}: {} keys ({} volatile)",
                clock(seconds),
                100 + n % 400,
                n % 40
            ),
            1 => format!(
                "1:M {} * Accepted 172.18.0.1:{}",
                clock(seconds),
                50_000 + n % 9_000
            ),
            _ => format!(
                "1:M {} * Background saving terminated with success",
                clock(seconds)
            ),
        },
        _ => match n % 3 {
            0 => format!(
                "{} [smtp] accepted message from noreply@{} to user{}@example.invalid",
                clock(seconds),
                host(index, "web"),
                n % 90
            ),
            1 => format!(
                "{} [http] GET /api/v1/messages 200 ({} messages in {})",
                clock(seconds),
                3 + n % 20,
                lane_name(index)
            ),
            _ => format!(
                "{} [smtp] listening on port {}",
                clock(seconds),
                18_025 + index * 100
            ),
        },
    }
}

/// Lines infra container `container` prints at second `step`, as `lane`
/// sees it.
pub fn container_output(lane: &str, container: &str, step: u64) -> Vec<String> {
    let roll = noise(lane, container, 2_000 + step);
    let every = match container {
        "postgres" => 2,
        "redis" => 3,
        _ => 5,
    };
    if roll.is_multiple_of(every) {
        vec![container_line(
            index_of(lane),
            container,
            roll,
            CLOCK + step,
        )]
    } else {
        Vec::new()
    }
}

/// What task `task` of `lane` prints, one line a second, and its exit code.
pub fn task(lane: &str, task: &str) -> (Vec<String>, i32) {
    let index = index_of(lane);
    let database = database(index);
    let lines = match task {
        "migrate" => vec![
            "\x1b[2m> myapp-api@1.4.0 migrate\x1b[0m".to_owned(),
            format!("applying migrations to {database}"),
            "\x1b[32m✓\x1b[0m 20261002_create_orders".to_owned(),
            "\x1b[32m✓\x1b[0m 20261009_add_order_status".to_owned(),
            "\x1b[32m✓\x1b[0m 20261018_checkout_sessions".to_owned(),
            format!("{database} is up to date (3 applied)"),
        ],
        "seed" => vec![
            "\x1b[2m> myapp-api@1.4.0 seed\x1b[0m".to_owned(),
            format!("seeding {database}"),
            "  users      50".to_owned(),
            "  products  120".to_owned(),
            "  orders    400".to_owned(),
            format!("\x1b[32m✓\x1b[0m seeded {database} in 1.8s"),
        ],
        _ => vec![
            "\x1b[2m> myapp@1.4.0 test\x1b[0m".to_owned(),
            "\x1b[2m> vitest run\x1b[0m".to_owned(),
            format!(
                " \x1b[32m✓\x1b[0m src/orders.test.ts (14 tests) {}ms",
                120 + index * 7
            ),
            " \x1b[32m✓\x1b[0m src/checkout.test.ts (9 tests) 88ms".to_owned(),
            " \x1b[32m✓\x1b[0m src/auth.test.ts (11 tests) 64ms".to_owned(),
            String::new(),
            " Test Files  \x1b[32m3 passed\x1b[0m (3)".to_owned(),
            "      Tests  \x1b[32m34 passed\x1b[0m (34)".to_owned(),
        ],
    };
    (lines, 0)
}

/// Stored log lines of a service, oldest first, for scrolling back past its
/// screen.
pub fn log_page(unit: &str) -> Vec<LogLine> {
    (0..60)
        .map(|seq| {
            let warn = seq % 17 == 11;
            let (level, color, rest) = if warn {
                (
                    "WARN",
                    33,
                    format!("slow query took {}ms: SELECT * FROM orders", 900 + seq),
                )
            } else {
                (
                    "INFO",
                    32,
                    format!("request id=req_{:05} status=200", 4_000 + seq),
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
