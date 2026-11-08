//! What makes the fake daemon move on its own: the attached service or
//! container prints new lines every second, tasks run and print their
//! output, and starts, stops and restarts change unit states.

use std::collections::{BTreeMap, VecDeque};

use devlanes_client::restart::Progress;
use devlanes_proto::{Event, LaneSummary, Method, Selection, UnitSummary, events};
use devlanes_tui::app::{App, Effect, Msg};
use serde_json::json;

use crate::scene;

/// A task run, going or done.
struct Run {
    lines: Vec<String>,
    printed: usize,
    exit: i32,
    finished: bool,
}

/// What changed since the scene's lanes: units started, stopped or run,
/// and the daemon stopped or restarted.
#[derive(Default)]
pub struct State {
    /// Units as they stand now, by lane and name, over the scene's.
    units: BTreeMap<(String, String), UnitSummary>,
    runs: BTreeMap<(String, String), Run>,
    /// Units that come up on the next second.
    starting: Vec<(String, String)>,
    /// What the daemon reports in the coming seconds while it stops,
    /// restarts or starts, one batch a second.
    daemon_steps: VecDeque<Vec<Msg>>,
    /// The daemon was stopped and not started again.
    down: bool,
    /// The daemon process, a new one after each restart or start.
    process: scene::Daemon,
}

/// A `lane.changed` event carrying `lane`, which replaces the app's copy.
fn lane_changed(lane: &LaneSummary) -> Msg {
    Msg::Event(Event {
        seq: 0,
        event: events::LANE_CHANGED.into(),
        data: json!({ "lane": lane }),
    })
}

/// `lane`'s state from its services: starting while one starts, running
/// while one runs, `idle` while none ever ran, else `stopped`.
fn lane_state(lane: &LaneSummary) -> String {
    let services = || lane.units.iter().filter(|unit| unit.kind == "service");
    if services().any(|unit| unit.state == "starting") {
        "starting".into()
    } else if services().any(|unit| matches!(unit.state.as_str(), "ready" | "running")) {
        "running".into()
    } else if services().all(|unit| unit.state == "idle") {
        "idle".into()
    } else {
        "stopped".into()
    }
}

/// The units of `lane` a control with `selection` acts on: the named ones
/// (`api` or `myapp@0:api`), else every service.
fn chosen(lane: &LaneSummary, selection: &Selection) -> Vec<UnitSummary> {
    let named = |unit: &UnitSummary| {
        selection.units.iter().any(|selected| {
            let name = selected
                .rsplit_once(':')
                .map_or(selected.as_str(), |(_, name)| name);
            name == unit.name
        })
    };
    lane.units
        .iter()
        .filter(|unit| {
            if selection.units.is_empty() {
                unit.kind == "service"
            } else {
                named(unit)
            }
        })
        .cloned()
        .collect()
}

impl State {
    /// The scene's lanes with every change applied.
    pub fn lanes(&self) -> Vec<LaneSummary> {
        let mut lanes = scene::lanes();
        for lane in &mut lanes {
            let mut touched = false;
            for ((reference, name), unit) in &self.units {
                if *reference != lane.reference {
                    continue;
                }
                touched = true;
                match lane.units.iter_mut().find(|u| u.name == *name) {
                    Some(existing) => *existing = unit.clone(),
                    None => lane.units.push(unit.clone()),
                }
            }
            if touched {
                lane.state = lane_state(lane);
            }
        }
        lanes
    }

    /// The lane `reference` with every change applied.
    fn lane(&self, reference: &str) -> Option<LaneSummary> {
        self.lanes()
            .into_iter()
            .find(|lane| lane.reference == reference)
    }

    /// Records `unit` as it stands now.
    fn set(&mut self, unit: UnitSummary) {
        self.units
            .insert((unit.lane.clone(), unit.name.clone()), unit);
    }

    /// The screen of task `unit` of `lane` so far, when it ran here.
    pub fn run_screen(&self, lane: &str, unit: &str) -> Option<String> {
        self.runs
            .get(&(lane.to_owned(), unit.to_owned()))
            .map(|run| run.lines[..run.printed].join("\r\n"))
    }

    /// Whether the daemon is stopped, so nothing answers.
    pub fn down(&self) -> bool {
        self.down
    }

    /// The daemon process the fake reports.
    pub fn process(&self) -> scene::Daemon {
        self.process
    }

    /// Plays a daemon stop, restart or start out over the next seconds, as
    /// the steps the TUI draws and then the answer.
    fn daemon(&mut self, step: u64, effect: &Effect) {
        // The new process exists from the "started" step on, a few seconds
        // in; its facts are what the TUI fetches once it is back.
        let next = scene::Daemon {
            pid: self.process.pid + 1,
            started_at_ms: scene::NOW_MS + (step + 3) * 1_000,
        };
        let status = scene::daemon_status(next, next.started_at_ms + 1_000);
        let stopping = || {
            [
                vec![Msg::DaemonStep(Progress::Stopping { processes: 12 })],
                vec![Msg::DaemonStep(Progress::Stopped)],
            ]
        };
        let starting = || {
            [
                vec![Msg::DaemonStep(Progress::Starting("state database".into()))],
                vec![Msg::DaemonStep(Progress::Started {
                    version: devlanes_version::VERSION.into(),
                    pid: Some(next.pid.into()),
                })],
            ]
        };
        let steps: Vec<Vec<Msg>> = match effect {
            Effect::StopDaemon => {
                // Stopping the daemon stops every unit it manages.
                for lane in self.lanes() {
                    for mut unit in lane.units.into_iter().filter(|u| u.kind == "service") {
                        if unit.state != "idle" {
                            unit.state = "stopped".into();
                            unit.pid = None;
                            unit.started_at_ms = None;
                            self.set(unit);
                        }
                    }
                }
                self.down = true;
                stopping()
                    .into_iter()
                    .chain([vec![Msg::DaemonStopped(Ok(json!({})))]])
                    .collect()
            }
            // A restart brings the lanes that ran back as they were. The
            // TUI's subscription to the old daemon ended with it; the last
            // batch is the one it opens on the new one.
            Effect::RestartDaemon => {
                self.process = next;
                stopping()
                    .into_iter()
                    .chain(starting())
                    .chain([
                        vec![Msg::DaemonRestarted(Ok(status))],
                        vec![Msg::Subscribed(Ok(Vec::new()))],
                    ])
                    .collect()
            }
            _ => {
                self.down = false;
                self.process = next;
                starting()
                    .into_iter()
                    .chain([
                        vec![Msg::DaemonStarted(Ok(status))],
                        vec![Msg::Subscribed(Ok(Vec::new()))],
                    ])
                    .collect()
            }
        };
        self.daemon_steps = steps.into();
    }

    /// Answers task runs, lane controls and daemon stops, restarts and
    /// starts; `None` for every other effect.
    pub fn answer(&mut self, step: u64, effect: &Effect) -> Option<Vec<Msg>> {
        if matches!(
            effect,
            Effect::StopDaemon | Effect::RestartDaemon | Effect::StartDaemon
        ) {
            self.daemon(step, effect);
            return Some(Vec::new());
        }
        let started_at = scene::NOW_MS + step * 1_000;
        let (reference, done) = match effect {
            Effect::Run { lane, params, .. } => {
                let name = devlanes_tui::tree::run_unit(&params.task).to_owned();
                let (lines, exit) = scene::task(lane, &params.task);
                let mut unit = scene::unit(lane, &name, "running", "task");
                unit.pty = false;
                unit.started_at_ms = Some(started_at);
                self.set(unit);
                let run = Run {
                    lines,
                    printed: 0,
                    exit,
                    finished: false,
                };
                self.runs.insert((lane.clone(), name), run);
                let done = Msg::RunDone {
                    lane: lane.clone(),
                    task: params.task.clone(),
                    result: Ok(json!({})),
                };
                (lane, done)
            }
            Effect::Control {
                action,
                method,
                lane,
                selection,
                label,
                target,
            } => {
                let current = self.lane(lane)?;
                for mut unit in chosen(&current, selection) {
                    if *method == Method::Stop {
                        unit.state = "stopped".into();
                        unit.pid = None;
                        unit.started_at_ms = None;
                    } else {
                        unit.state = "starting".into();
                        unit.started_at_ms = Some(started_at);
                        self.starting.push((unit.lane.clone(), unit.name.clone()));
                    }
                    self.set(unit);
                }
                let done = Msg::ActionDone {
                    action: *action,
                    label: label.clone(),
                    target: target.clone(),
                    units: selection.units.clone(),
                    result: Ok(json!({})),
                };
                (lane, done)
            }
            _ => return None,
        };
        let changed = self.lane(reference).map(|lane| lane_changed(&lane));
        Some(changed.into_iter().chain([done]).collect())
    }

    /// One second on: starting units are up, runs print their next line and
    /// end, and the attached service or container prints this second's lines.
    pub fn tick(&mut self, step: u64, app: &App) -> Vec<Msg> {
        let mut msgs = self.daemon_steps.pop_front().unwrap_or_default();
        let mut changed: Vec<String> = Vec::new();
        for (lane, name) in std::mem::take(&mut self.starting) {
            if let Some(mut unit) = self.units.get(&(lane.clone(), name)).cloned() {
                let worker = unit.ready_check.as_deref() == Some("started");
                unit.state = if worker { "running" } else { "ready" }.into();
                self.set(unit);
                changed.push(lane);
            }
        }
        let attached = app
            .attached
            .as_ref()
            .map(|a| (a.generation, a.lane.clone(), a.unit.clone(), a.container));
        for ((lane, name), run) in &mut self.runs {
            if run.finished {
                continue;
            }
            let Some(line) = run.lines.get(run.printed) else {
                run.finished = true;
                if let Some(unit) = self.units.get_mut(&(lane.clone(), name.clone())) {
                    unit.state = "exited".into();
                    unit.exit_code = Some(run.exit);
                }
                changed.push(lane.clone());
                continue;
            };
            if let Some((generation, ..)) = attached
                .as_ref()
                .filter(|(_, l, u, container)| l == lane && u == name && !container)
            {
                let separator = if run.printed == 0 { "" } else { "\r\n" };
                msgs.push(Msg::AttachOutput {
                    generation: *generation,
                    bytes: format!("{separator}{line}").into_bytes(),
                });
            }
            run.printed += 1;
        }
        if let Some((generation, lane, unit, container)) = attached
            && !self.runs.contains_key(&(lane.clone(), unit.clone()))
        {
            let lines = if container {
                scene::container_output(&lane, &unit, step)
            } else if self.is_up(&lane, &unit) {
                scene::output(&lane, &unit, step)
            } else {
                Vec::new()
            };
            if !lines.is_empty() {
                msgs.push(Msg::AttachOutput {
                    generation,
                    bytes: format!("\r\n{}", lines.join("\r\n")).into_bytes(),
                });
            }
        }
        changed.sort();
        changed.dedup();
        msgs.extend(
            changed
                .iter()
                .filter_map(|reference| self.lane(reference))
                .map(|lane| lane_changed(&lane)),
        );
        msgs
    }

    /// Whether unit `unit` of `lane` runs now.
    fn is_up(&self, lane: &str, unit: &str) -> bool {
        self.lane(lane).is_some_and(|lane| {
            lane.units
                .iter()
                .any(|u| u.name == unit && matches!(u.state.as_str(), "ready" | "running"))
        })
    }
}
