//! Drift: how a lane's running units differ from what its project file would start
//! now, per unit, and `reload`, which applies only that. Everything here lives in
//! memory: a fingerprint holds environment values, so it never reaches state.db.
//!
//! One job per lane computes it ([`DriftJob`]). A pass reads the lane's facts
//! (config, layout, profiles, client env, overrides, allocation, infra namespace,
//! units, orphans, waiting units, last whole start) and, through the plan's inputs,
//! lane 0's layout and other projects' running lanes. Those facts change only
//! through setters that tick the engine's [`FactsClock`], and a run's exit ticks it
//! too. A pass records the count it started from. A reader that needs current drift
//! (`status`, `lane ls`, `reload`, the `start --profile` warning) asks for a pass
//! when the lane is behind the count it saw, and waits until a pass that started
//! from at least that count applied.
//!
//! A pass plans only the running units whose plan inputs changed since their spawn
//! or their last answer ([`InputsKey`]), so stops, exits and spawns plan nothing.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use devlanes_core::env::EnvLayer;
use devlanes_proto::{Context, LaneWarning, RpcError, Selection, StartResult, UnitDrift as Wire};
use devlanes_resolve::{InputsKey, ResolvedService};
use futures_util::FutureExt as _;
use serde_json::{Value, json};

use crate::{
    Session,
    engine::{Inner, LanePair, carries_env, params},
    lifecycle::StartOptions,
    model::{Kind, LaneRt, ProjectRt, lock},
    registry::internal,
    stream::LaneEvents,
};

/// What a service run was spawned with, compared field by field with what the
/// current config would spawn.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    pty: bool,
    env: BTreeMap<String, String>,
    files: Vec<(PathBuf, Vec<u8>)>,
    ports: BTreeMap<String, u16>,
    ready: devlanes_resolve::Ready,
    stop: String,
    /// The key of the plan the run was spawned from.
    inputs: InputsKey,
}

impl Fingerprint {
    /// The fingerprint of a planned service replica, from a plan with `inputs`.
    pub(crate) fn of(service: &ResolvedService, inputs: InputsKey) -> Self {
        let mut ports = service.ports.clone();
        ports.insert("default".into(), service.port);
        Self {
            program: service.launch.program.clone(),
            args: service.launch.args.clone(),
            cwd: service.launch.cwd.clone(),
            pty: service.pty,
            // Variables whose top layer is the client's shell or `--env` come from the
            // request that started the unit, not from the file, and a later command
            // from another shell replaces them for the lane.
            env: service
                .launch
                .env
                .values
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        service.launch.env.sources.get(*key),
                        Some(EnvLayer::Client | EnvLayer::Override)
                    )
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            files: service
                .launch
                .files
                .iter()
                .map(|file| (file.path.clone(), file.contents.clone()))
                .collect(),
            ports,
            ready: service.ready.clone(),
            stop: format!("{:?}", service.stop),
            inputs,
        }
    }

    /// What differs from `now`, by name: `run` for the command, `env KEY` per
    /// variable. The plan keys are not compared.
    pub(crate) fn changes(&self, now: &Self) -> Vec<String> {
        let mut fields = Vec::new();
        if self.program != now.program || self.args != now.args {
            fields.push("run".to_owned());
        }
        if self.cwd != now.cwd {
            fields.push("cwd".to_owned());
        }
        if self.ports != now.ports {
            fields.push("ports".to_owned());
        }
        if self.pty != now.pty {
            fields.push("pty".to_owned());
        }
        if self.ready != now.ready {
            fields.push("ready".to_owned());
        }
        if self.stop != now.stop {
            fields.push("stop".to_owned());
        }
        if self.files != now.files {
            fields.push("files".to_owned());
        }
        let keys: BTreeSet<&String> = self.env.keys().chain(now.env.keys()).collect();
        fields.extend(
            keys.into_iter()
                // The lane's hostnames: one service gaining or losing hosts would mark
                // every other one changed. It is the lane's, not this unit's.
                .filter(|key| key.as_str() != "DEVLANES_ORIGINS")
                .filter(|key| self.env.get(*key) != now.env.get(*key))
                .map(|key| format!("env {key}")),
        );
        fields
    }
}

/// One unit's difference from the current config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnitDrift {
    /// Runs, and the config would start it differently in these fields.
    Changed(Vec<String>),
    /// Runs, and the config no longer has it.
    Removed,
    /// The lane's profiles select it, it has not run since the lane's last whole
    /// start, and that start did not leave it out on purpose.
    Added,
}

impl UnitDrift {
    /// The wire form for `UnitSummary.drift`.
    pub(crate) fn wire(&self) -> Wire {
        match self {
            Self::Changed(fields) => Wire {
                kind: "changed".into(),
                fields: fields.clone(),
            },
            Self::Removed => Wire {
                kind: "removed".into(),
                fields: Vec::new(),
            },
            Self::Added => Wire {
                kind: "added".into(),
                fields: Vec::new(),
            },
        }
    }
}

/// Why the running units could not be compared with the file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Unplanned {
    /// The file does not load.
    FileInvalid,
    /// Planning failed with this error, which `reload` answers with.
    PlanFailed(RpcError),
}

/// What a plan said about one run, kept while the run and the plan's inputs stay.
#[derive(Clone)]
struct Answer {
    run: Arc<Fingerprint>,
    key: InputsKey,
    drift: Option<UnitDrift>,
}

/// A lane's drift as a pass computed it.
#[derive(Clone, Default)]
pub(crate) struct Drift {
    /// Per unit.
    pub units: BTreeMap<String, UnitDrift>,
    /// The infra settings differ from the ones of the lane's last whole start.
    pub infra_changed: bool,
    /// Units run and could not be compared with the file.
    pub unplanned: Option<Unplanned>,
    /// The plan answers behind `units`, by unit.
    answers: BTreeMap<String, Answer>,
}

impl Drift {
    /// Whether `other` shows the same to clients.
    pub(crate) fn shows_like(&self, other: &Self) -> bool {
        self.units == other.units
            && self.infra_changed == other.infra_changed
            && self.unplanned == other.unplanned
    }
}

/// The infra settings of the lane's last whole start and the units it left out
/// on purpose: units its profiles select that its selection did not name (`start
/// api` alone), and those of its selection that got no run (an `autostart` that
/// evaluated false, a stop that withdrew them). They never count as `added`.
#[derive(Debug, Clone)]
pub(crate) struct StartedWith {
    pub infra: Value,
    pub left_out: BTreeSet<String>,
}

impl StartedWith {
    /// Taken when a whole-lane start begins, with the profiles it starts with
    /// and the services it selected.
    pub(crate) fn new(
        config: &devlanes_config::ProjectConfig,
        profiles: &[String],
        selected: &[String],
    ) -> Self {
        Self {
            left_out: devlanes_resolve::select_profiles(config, profiles, &[])
                .map(|selection| selection.services)
                .unwrap_or_default()
                .iter()
                .filter(|service| !selected.contains(service))
                .flat_map(|service| units_of(config, service))
                .collect(),
            infra: started_infra(config),
        }
    }
}

/// The infra settings a whole-lane start brings up. Routes (`external`, `hosts`),
/// `hidden` and `teardown` apply without one, so editing them is no drift.
fn started_infra(config: &devlanes_config::ProjectConfig) -> Value {
    let infra = devlanes_config::InfraConfig {
        external: Default::default(),
        hosts: Default::default(),
        hidden: None,
        teardown: Default::default(),
        ..config.infra.clone()
    };
    serde_json::to_value(&infra).unwrap_or(Value::Null)
}

/// The unit names of `service` in `config`: the service name, or `<service>.<N>` for
/// each replica when there are several, as the resolver lays them out.
pub(crate) fn units_of(config: &devlanes_config::ProjectConfig, service: &str) -> Vec<String> {
    match config.services.get(service).and_then(|s| s.replicas) {
        Some(count) if count > 1 => (1..=count).map(|n| format!("{service}.{n}")).collect(),
        _ => vec![service.to_owned()],
    }
}

/// The service a unit belongs to in `config`: itself, or `api` for replica `api.2`.
pub(crate) fn service_of<'a>(
    config: &devlanes_config::ProjectConfig,
    unit: &'a str,
) -> Option<&'a str> {
    if config.services.contains_key(unit) {
        return Some(unit);
    }
    let (base, replica) = unit.rsplit_once('.')?;
    (replica.bytes().all(|b| b.is_ascii_digit()) && config.services.contains_key(base))
        .then_some(base)
}

/// Whether the loaded config `current` is `used`, or reads the same. Compares
/// pointers first: they differ only while a start an edit overtook runs.
pub(crate) fn same_config(
    current: Option<&Arc<devlanes_config::ProjectConfig>>,
    used: &Arc<devlanes_config::ProjectConfig>,
) -> bool {
    current.is_some_and(|current| {
        Arc::ptr_eq(current, used)
            || serde_json::to_value(current.as_ref()).ok()
                == serde_json::to_value(used.as_ref()).ok()
    })
}

/// The single-flight drift job of one lane.
pub(crate) struct DriftJob {
    /// A leaf lock: nothing is taken while it is held.
    state: std::sync::Mutex<JobState>,
    /// The clock count the last applied pass started from.
    applied: tokio::sync::watch::Sender<u64>,
}

/// Whether a pass is asked for and whether a runner is active.
#[derive(Default)]
struct JobState {
    requested: bool,
    running: bool,
}

impl Default for DriftJob {
    /// No pass applied yet.
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(JobState::default()),
            applied: tokio::sync::watch::Sender::new(0),
        }
    }
}

impl DriftJob {
    /// Asks for a pass; true when no runner is active, so the caller starts one.
    fn request(&self) -> bool {
        let mut state = lock(&self.state);
        state.requested = true;
        !std::mem::replace(&mut state.running, true)
    }

    /// For the runner: true when a pass is asked for, which it then runs. False
    /// ends the runner, under the lock a request takes, so no request is lost.
    fn take(&self) -> bool {
        let mut state = lock(&self.state);
        if std::mem::take(&mut state.requested) {
            true
        } else {
            state.running = false;
            false
        }
    }

    /// Whether the last applied pass started before the clock read `seen`.
    fn behind(&self, seen: u64) -> bool {
        *self.applied.borrow() < seen
    }
}

/// The facts of one lane a pass reads under the lane lock.
struct Taken {
    config: Option<Arc<devlanes_config::ProjectConfig>>,
    /// Running service units and their fingerprints.
    running: Vec<(String, Option<Arc<Fingerprint>>)>,
    recorded: BTreeSet<String>,
    orphans: BTreeSet<String>,
    /// `(service, unit)` of each laid out service unit.
    laid_out: Vec<(String, String)>,
    started: Option<StartedWith>,
    waiting: BTreeSet<String>,
    profiles: Vec<String>,
    answers: BTreeMap<String, Answer>,
}

impl Inner {
    /// Asks for a drift pass of `lane`, starting its runner when none runs.
    /// Requests during a pass make it run once more.
    pub(crate) fn request_drift(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        if !lane.drift_job.request() {
            return;
        }
        let (engine, project, lane) = (self.clone(), project.clone(), lane.clone());
        tokio::spawn(async move {
            while lane.drift_job.take() {
                engine.drift_pass(&project, &lane).await;
            }
        });
    }

    /// Asks for a pass of `lane` when it is behind the clock, without waiting.
    pub(crate) fn refresh_drift(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        if lane.drift_job.behind(self.clock.now()) {
            self.request_drift(project, lane);
        }
    }

    /// Waits until the drift of each of `lanes` includes every fact change made
    /// before this call: at most the pass in flight plus one, per lane, with
    /// the lanes in parallel.
    pub(crate) async fn current_drift(self: &Arc<Self>, lanes: &[LanePair]) {
        let seen = self.clock.now();
        let waits = lanes
            .iter()
            .filter(|(_, lane)| lane.drift_job.behind(seen))
            .map(|(project, lane)| {
                self.request_drift(project, lane);
                let mut applied = lane.drift_job.applied.subscribe();
                async move {
                    // The sender lives on the lane, which `lanes` holds.
                    let _ = applied.wait_for(|at| *at >= seen).await;
                }
            })
            .collect::<Vec<_>>();
        futures_util::future::join_all(waits).await;
    }

    /// One pass: computes the lane's drift from its facts as of now, applies it
    /// with its plan answers, retires the records of dropped services, and emits
    /// `lane.changed` when what clients see changed. A pass that panics keeps
    /// the previous per-unit drift and records the panic, so `reload` refuses
    /// instead of acting on it; it still counts as applied.
    async fn drift_pass(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        let at = self.clock.now();
        let computed = std::panic::AssertUnwindSafe(self.compute_drift(project, lane))
            .catch_unwind()
            .await;
        let drift = match computed {
            Ok(drift) => drift,
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "the drift pass panicked".to_owned());
                let reference = lane.st().reference(&project.name());
                tracing::error!(lane = %reference, %message, "drift pass panicked");
                let mut kept = lane.st().drift().clone();
                kept.unplanned = Some(Unplanned::PlanFailed(internal("drift_failed", message)));
                kept
            }
        };
        let changed = {
            let mut st = lane.st();
            let shown = st.apply_drift(drift);
            let before = st.units().len();
            st.retire_dropped();
            shown || st.units().len() != before
        };
        lane.drift_job.applied.send_if_modified(|applied| {
            let newer = at > *applied;
            if newer {
                *applied = at;
            }
            newer
        });
        if changed {
            self.emit_lane(project, lane);
        }
    }

    /// The lane's drift against its current config. Nothing running leaves no
    /// drift; a file that does not load leaves no per-unit drift.
    async fn compute_drift(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) -> Drift {
        let taken = {
            let st = lane.st();
            Taken {
                config: st.config().loaded().cloned(),
                running: st
                    .units()
                    .iter()
                    .filter(|(_, rec)| rec.kind == Kind::Service && rec.host.is_alive())
                    .map(|(unit, rec)| (unit.clone(), rec.fingerprint.clone()))
                    .collect(),
                recorded: st.units().keys().cloned().collect(),
                orphans: st.orphans().keys().cloned().collect(),
                laid_out: st
                    .view()
                    .map(|view| {
                        view.services
                            .iter()
                            .map(|service| (service.name.clone(), service.unit.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                started: st.started_with().cloned(),
                waiting: st.waiting().keys().cloned().collect(),
                profiles: st.profiles().to_vec(),
                answers: st.drift().answers.clone(),
            }
        };
        let mut drift = Drift::default();
        if taken.running.is_empty() {
            return drift;
        }
        let Some(config) = taken.config else {
            drift.unplanned = Some(Unplanned::FileInvalid);
            return drift;
        };
        let key = match self.gather(project, lane, None, Some(config.clone())).await {
            Ok(gathered) => Some(gathered.key(self)),
            Err(error) => {
                drift.unplanned = Some(Unplanned::PlanFailed(error));
                None
            }
        };
        let mut stale: Vec<(&String, &Arc<Fingerprint>)> = Vec::new();
        for (unit, fingerprint) in &taken.running {
            if service_of(&config, unit).is_none() {
                drift.units.insert(unit.clone(), UnitDrift::Removed);
                continue;
            }
            // A run without one (spawned outside a plan) is never compared.
            let (Some(fingerprint), Some(key)) = (fingerprint, key) else {
                continue;
            };
            if fingerprint.inputs == key {
                continue;
            }
            match taken.answers.get(unit) {
                Some(answer) if Arc::ptr_eq(&answer.run, fingerprint) && answer.key == key => {
                    if let Some(unit_drift) = &answer.drift {
                        drift.units.insert(unit.clone(), unit_drift.clone());
                    }
                    drift.answers.insert(unit.clone(), answer.clone());
                }
                _ => stale.push((unit, fingerprint)),
            }
        }
        if !stale.is_empty() {
            // Only the stale units' services are planned: each plan can cost a
            // contributor call.
            let only: BTreeSet<String> = stale
                .iter()
                .filter_map(|(unit, _)| service_of(&config, unit))
                .map(|service| format!("services.{service}"))
                .collect();
            match self
                .resolve_config(project, lane, config.clone(), Some(&only))
                .await
            {
                Ok((resolved, key)) => {
                    for (unit, fingerprint) in stale {
                        let unit_drift = match resolved.services.get(unit) {
                            // Fewer replicas leave the extra ones out of the plan.
                            None => Some(UnitDrift::Removed),
                            Some(service) => {
                                let fields = fingerprint.changes(&Fingerprint::of(service, key));
                                (!fields.is_empty()).then_some(UnitDrift::Changed(fields))
                            }
                        };
                        if let Some(unit_drift) = &unit_drift {
                            drift.units.insert(unit.clone(), unit_drift.clone());
                        }
                        drift.answers.insert(
                            unit.clone(),
                            Answer {
                                run: fingerprint.clone(),
                                key,
                                drift: unit_drift,
                            },
                        );
                    }
                }
                Err(error) => drift.unplanned = Some(Unplanned::PlanFailed(error)),
            }
        }
        // Without a whole start to compare with (a lane brought back next to a previous
        // daemon's processes), nothing counts as added.
        if let Some(started) = &taken.started {
            let selected = devlanes_resolve::select_profiles(&config, &taken.profiles, &[])
                .map(|selection| selection.services)
                .unwrap_or_default();
            for (service, unit) in taken.laid_out {
                let added = selected.contains(&service)
                    && !taken.recorded.contains(&unit)
                    && !taken.orphans.contains(&unit)
                    && !taken.waiting.contains(&unit)
                    && !started.left_out.contains(&unit);
                if added {
                    drift.units.insert(unit, UnitDrift::Added);
                }
            }
            drift.infra_changed = started_infra(&config) != started.infra;
        }
        drift
    }

    /// `reload`: applies the lane's drift. Removed units stop, changed ones restart
    /// and added ones start; changed infra or ports restart the whole lane, which
    /// only a lane with nothing running can take. Named units limit it to them.
    pub(crate) async fn reload_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let arrival = self.arrive();
        let selection: Selection = params(value)?;
        let ((project, lane), selectors) = self
            .lane_for_units(
                &selection.units,
                context,
                crate::target::Policy::of(devlanes_proto::Method::Reload),
            )
            .await?;
        self.fresh_config(&project, &lane).await;
        let name = project.name();
        let reference = lane.st().reference(&name);
        lane.st().config().require(&reference)?;
        self.current_drift(&[(project.clone(), lane.clone())]).await;
        let (drift, whole) = {
            let st = lane.st();
            if !st.any_running() {
                return Err(RpcError::new(
                    "not_running",
                    json!({
                        "lane": reference,
                        "message": format!("nothing runs in {reference}; devlanes start starts it"),
                    }),
                    1,
                ));
            }
            // The units the selectors name, as `stop` picks them (names, globs,
            // `+tag`s, a service for its replicas); names the lane does not have
            // are a typo, not "nothing to reload".
            let picked: Option<BTreeSet<String>> = if selectors.is_empty() {
                None
            } else {
                Some(
                    st.match_units(&reference, &selectors)?
                        .into_iter()
                        .collect(),
                )
            };
            // Without a plan the drift is incomplete; its error is the answer.
            if let Some(Unplanned::PlanFailed(error)) = &st.drift().unplanned {
                return Err(error.clone());
            }
            let drift: BTreeMap<String, UnitDrift> = st
                .drift()
                .units
                .iter()
                .filter(|(unit, _)| picked.as_ref().is_none_or(|picked| picked.contains(*unit)))
                .map(|(unit, drift)| (unit.clone(), drift.clone()))
                .collect();
            (
                drift,
                selectors.is_empty() && (st.drift().infra_changed || st.ports_pending),
            )
        };
        let options = |start_id| StartOptions {
            selectors: Vec::new(),
            profiles: Vec::new(),
            transient: false,
            needs_profile_settings: false,
            env: carries_env(context),
            overrides: None,
            actor: context.actor.clone(),
            timeout: self.timeout(selection.timeout_ms),
            session: None,
            setup_only: false,
            all_at_once: selection.all_at_once,
            start_id: Some(start_id),
            arrival,
            restoring: false,
            accepted: selection.accepted.then(|| session.clone()),
        };
        let start_id = self.new_start_id();
        let stream = selection
            .events
            .then(|| LaneEvents::starts(vec![(project.clone(), lane.clone())], vec![start_id]));
        if whole {
            let restart = self.restart_lane(&project, &lane, options(start_id));
            let mut result = self.with_lane_events(session, stream, restart).await?;
            // A start from another client between the stop and this start makes it
            // add units only; say what happened.
            if result.whole_lane {
                result.warnings.push(LaneWarning {
                    code: "reload_restarted_lane".into(),
                    data: json!({
                        "lane": reference,
                        "message": "infra or ports changed, so the whole lane restarted",
                    }),
                });
            }
            return serde_json::to_value(result).map_err(|e| internal("encode", e));
        }
        let removed: Vec<String> = drift
            .iter()
            .filter(|(_, drift)| **drift == UnitDrift::Removed)
            .map(|(unit, _)| unit.clone())
            .collect();
        let restart: Vec<String> = drift
            .iter()
            .filter(|(_, drift)| **drift != UnitDrift::Removed)
            .map(|(unit, _)| unit.clone())
            .collect();
        if !removed.is_empty() {
            self.stop_lane(&project, &lane, Some(removed.clone()), None, false)
                .await?;
        }
        let mut result = if restart.is_empty() {
            StartResult {
                lane: reference.clone(),
                ready: true,
                ..StartResult::default()
            }
        } else {
            let work = self.restart_units(&project, &lane, &restart, options(start_id));
            self.with_lane_events(session, stream, work).await?
        };
        if !removed.is_empty() {
            result.warnings.push(LaneWarning {
                code: "units_removed".into(),
                data: json!({
                    "lane": reference,
                    "units": removed,
                    "message": format!(
                        "stopped {}, which the project file no longer has",
                        removed.join(", ")
                    ),
                }),
            });
        }
        if drift.is_empty() {
            result.warnings.push(LaneWarning {
                code: "nothing_to_reload".into(),
                data: json!({
                    "lane": reference,
                    "units": selectors,
                    "message": format!("nothing to reload: {reference} matches its project file"),
                }),
            });
        }
        self.emit_lane(&project, &lane);
        serde_json::to_value(result).map_err(|e| internal("encode", e))
    }

    /// Restarts the whole lane: stops everything, keeping what the lane saved, then
    /// starts it as a plain `start` would, so infra, setup, tasks and a waiting port
    /// block move all happen. What would make the start refuse (a file that does not
    /// load, an unassigned lane, no environment) refuses before anything stops.
    /// Another client's start in the gap between the stop and the start makes this one
    /// add units only, which its result's `whole_lane` says.
    pub(crate) async fn restart_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        options: StartOptions,
    ) -> Result<StartResult, RpcError> {
        let reference = lane.st().reference(&project.name());
        {
            let st = lane.st();
            st.config().require(&reference)?;
            if let Some(reason) = &st.unassigned {
                return Err(reason.clone());
            }
            if st.record().is_none() {
                return Err(crate::model::unassigned(
                    &reference,
                    crate::model::Unassigned::NoRecord,
                ));
            }
            if options.env.is_none() && st.env().is_none() {
                return Err(RpcError::new("needs_env", json!({"lane": reference}), 1));
            }
        }
        // Transient: the stop leaves the saved profiles and services for the start.
        let stopped = self.stop_lane(project, lane, None, None, true).await?;
        // Running an exec command again unasked would be an auto-restart; say it stopped.
        let execs: Vec<String> = stopped
            .into_iter()
            .filter(|unit| unit.kind == "exec")
            .map(|unit| unit.name)
            .collect();
        let mut result = match self.start_lane(project, lane, options).await {
            Ok(result) => result,
            // The exec units are gone either way; the error names them.
            Err(mut error) => {
                if !execs.is_empty()
                    && let Some(data) = error.data.as_object_mut()
                {
                    data.insert("exec_stopped".into(), json!(execs));
                }
                return Err(error);
            }
        };
        if !execs.is_empty() {
            result.warnings.push(LaneWarning {
                code: "exec_stopped".into(),
                data: json!({
                    "lane": reference,
                    "units": execs,
                    "message": format!(
                        "stopped exec units {}; a whole-lane restart does not run them again",
                        execs.join(", ")
                    ),
                }),
            });
        }
        Ok(result)
    }
}
