//! What makes a scene's fake daemon move on its own: attached services and
//! containers print new lines every second, tasks run and print their
//! output, and starts, stops and restarts change unit states. Scenes
//! without [`Live`] (the committed screens') answer none of this, so they
//! draw the same on every run.

use std::collections::BTreeMap;

use devlanes_proto::{Event, LaneSummary, Method, Selection, UnitSummary, events};
use serde_json::json;

use crate::app::{App, Effect, Msg};

/// A live scene's generators.
#[derive(Clone, Copy)]
pub struct Live {
    /// Lines service `unit` of lane `lane` prints at second `step`, often none.
    pub output: fn(&str, &str, u64) -> Vec<String>,
    /// Lines infra container `container` prints at second `step`, as lane
    /// `lane` sees it.
    pub container_output: fn(&str, &str, u64) -> Vec<String>,
    /// What task `task` of lane `lane` prints, one line a second, and its
    /// exit code.
    pub task: fn(&str, &str) -> (Vec<String>, i32),
}

/// A task run, going or done.
struct Run {
    lines: Vec<String>,
    printed: usize,
    exit: i32,
    finished: bool,
}

/// What changed since the scene's lanes: units started, stopped or run.
#[derive(Default)]
pub(super) struct State {
    /// Units as they stand now, by lane and name, over the scene's.
    units: BTreeMap<(String, String), UnitSummary>,
    runs: BTreeMap<(String, String), Run>,
    /// Units that become ready on the next second.
    starting: Vec<(String, String)>,
}

/// A `lane.changed` event carrying `lane`, which replaces the app's copy.
fn lane_changed(lane: &LaneSummary) -> Msg {
    Msg::Event(Event {
        seq: 0,
        event: events::LANE_CHANGED.into(),
        data: json!({ "lane": lane }),
    })
}

/// `lane`'s state from its units: starting while one starts, running while
/// one runs, else `stopped`; an idle lane nobody touched stays `idle`.
fn lane_state(lane: &LaneSummary) -> String {
    let live = |state: &str| matches!(state, "ready" | "running");
    let services = || lane.units.iter().filter(|unit| unit.kind == "service");
    if services().any(|unit| unit.state == "starting") {
        "starting".into()
    } else if services().any(|unit| live(&unit.state)) {
        "running".into()
    } else if services().all(|unit| unit.state == "idle") {
        "idle".into()
    } else {
        "stopped".into()
    }
}

impl State {
    /// `lanes` with every change applied.
    pub(super) fn overlay(&self, mut lanes: Vec<LaneSummary>) -> Vec<LaneSummary> {
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

    /// The lane `reference` of `lanes` with every change applied.
    fn lane(&self, lanes: Vec<LaneSummary>, reference: &str) -> Option<LaneSummary> {
        self.overlay(lanes)
            .into_iter()
            .find(|lane| lane.reference == reference)
    }

    /// Records `unit` as it stands now.
    fn set(&mut self, unit: UnitSummary) {
        self.units
            .insert((unit.lane.clone(), unit.name.clone()), unit);
    }

    /// The screen of task `unit` of `lane` so far, when it ran here.
    pub(super) fn run_screen(&self, lane: &str, unit: &str) -> Option<String> {
        self.runs
            .get(&(lane.to_owned(), unit.to_owned()))
            .map(|run| run.lines[..run.printed].join("\r\n"))
    }

    /// Answers `effect` when this state handles it: task runs and lane
    /// controls. `None` leaves it to the fake's usual answers.
    pub(super) fn answer(
        &mut self,
        live: &Live,
        lanes: Vec<LaneSummary>,
        step: u64,
        effect: &Effect,
    ) -> Option<Vec<Msg>> {
        match effect {
            Effect::Run { lane, params, .. } => {
                let name = crate::tree::run_unit(&params.task).to_owned();
                let (lines, exit) = (live.task)(lane, &params.task);
                let mut unit = super::data::unit(lane, &name, "running", "task");
                unit.pty = false;
                unit.started_at_ms = Some(super::data::NOW_MS + step * 1_000);
                self.set(unit);
                self.runs.insert(
                    (lane.clone(), name),
                    Run {
                        lines,
                        printed: 0,
                        exit,
                        finished: false,
                    },
                );
                let changed = self.lane(lanes, lane).map(|lane| lane_changed(&lane));
                Some(
                    changed
                        .into_iter()
                        .chain([Msg::RunDone {
                            lane: lane.clone(),
                            task: params.task.clone(),
                            result: Ok(json!({})),
                        }])
                        .collect(),
                )
            }
            Effect::Control {
                action,
                method,
                lane,
                selection,
                label,
                target,
            } => {
                let current = self.lane(lanes.clone(), lane)?;
                for mut unit in chosen(&current, selection) {
                    match method {
                        Method::Stop => {
                            unit.state = "stopped".into();
                            unit.pid = None;
                            unit.started_at_ms = None;
                        }
                        _ => {
                            unit.state = "starting".into();
                            unit.started_at_ms = Some(super::data::NOW_MS + step * 1_000);
                            self.starting.push((unit.lane.clone(), unit.name.clone()));
                        }
                    }
                    self.set(unit);
                }
                let changed = self.lane(lanes, lane).map(|lane| lane_changed(&lane));
                Some(
                    changed
                        .into_iter()
                        .chain([Msg::ActionDone {
                            action: *action,
                            label: label.clone(),
                            target: target.clone(),
                            units: selection.units.clone(),
                            result: Ok(json!({})),
                        }])
                        .collect(),
                )
            }
            _ => None,
        }
    }

    /// One second on: units that were starting are up, runs print their
    /// next line and end, and the attached service or container prints
    /// what it printed this second.
    pub(super) fn tick(
        &mut self,
        live: &Live,
        lanes: Vec<LaneSummary>,
        step: u64,
        app: &App,
    ) -> Vec<Msg> {
        let mut msgs = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        for (lane, name) in std::mem::take(&mut self.starting) {
            if let Some(mut unit) = self.units.get(&(lane.clone(), name)).cloned() {
                unit.state = if unit.ready_check.as_deref() == Some("started") {
                    "running"
                } else {
                    "ready"
                }
                .into();
                self.set(unit);
                changed.push(lane);
            }
        }
        let attached = app
            .attached
            .as_ref()
            .map(|a| (a.generation, a.lane.clone(), a.unit.clone(), a.container));
        let mut printed_run = false;
        for ((lane, name), run) in &mut self.runs {
            if run.finished {
                continue;
            }
            if let Some(line) = run.lines.get(run.printed) {
                if let Some((generation, _, _, false)) =
                    attached.as_ref().filter(|a| a.1 == *lane && a.2 == *name)
                {
                    let separator = if run.printed == 0 { "" } else { "\r\n" };
                    msgs.push(Msg::AttachOutput {
                        generation: *generation,
                        bytes: format!("{separator}{line}").into_bytes(),
                    });
                    printed_run = true;
                }
                run.printed += 1;
            } else {
                run.finished = true;
                if let Some(unit) = self.units.get_mut(&(lane.clone(), name.clone())) {
                    unit.state = "exited".into();
                    unit.exit_code = Some(run.exit);
                }
                changed.push(lane.clone());
            }
        }
        if let Some((generation, lane, unit, container)) = attached
            && !printed_run
            && !self.runs.contains_key(&(lane.clone(), unit.clone()))
        {
            let up = || {
                self.overlay(lanes.clone())
                    .iter()
                    .filter(|l| l.reference == lane)
                    .flat_map(|l| l.units.iter())
                    .any(|u| u.name == unit && matches!(u.state.as_str(), "ready" | "running"))
            };
            let lines = if container {
                (live.container_output)(&lane, &unit, step)
            } else if up() {
                (live.output)(&lane, &unit, step)
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
        for reference in changed {
            if let Some(lane) = self.lane(lanes.clone(), &reference) {
                msgs.push(lane_changed(&lane));
            }
        }
        msgs
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
