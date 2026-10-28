//! Drift: how a lane's running units differ from what its project file would start
//! now, per unit, and `reload`, which applies only that. Everything here lives in
//! memory: a fingerprint holds environment values, so it never reaches state.db.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use devlanes_core::env::EnvLayer;
use devlanes_proto::{Context, LaneWarning, RpcError, Selection, StartResult, UnitDrift as Wire};
use devlanes_resolve::ResolvedService;
use serde_json::{Value, json};

use crate::{
    Session,
    engine::{Inner, carries_env, params},
    lifecycle::StartOptions,
    model::{EnvMap, Kind, LaneRt, LaneSt, ProjectRt},
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
}

impl Fingerprint {
    /// The fingerprint of a planned service replica.
    pub(crate) fn of(service: &ResolvedService) -> Self {
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
        }
    }

    /// What differs from `now`, by name: `run` for the command, `env KEY` per variable.
    fn changes(&self, now: &Self) -> Vec<String> {
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
    /// The lane's profiles select it now, it was not selected at the lane's last
    /// whole start, and it is not running.
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

/// The units the lane's profiles selected at its last whole start, replicas
/// included, and the infra settings it started with. A unit the profiles select now
/// but did not then (a new service, or a replica of a raised `replicas`) is `added`;
/// one the user stopped or left out on purpose (`start api` alone) never is.
#[derive(Debug, Clone)]
pub(crate) struct StartedWith {
    pub units: BTreeSet<String>,
    pub infra: Value,
}

impl StartedWith {
    /// Taken when a whole-lane start begins, with the profiles it starts with.
    pub(crate) fn new(config: &devlanes_config::ProjectConfig, profiles: &[String]) -> Self {
        Self {
            units: devlanes_resolve::select_profiles(config, profiles, &[])
                .map(|selection| selection.services)
                .unwrap_or_default()
                .iter()
                .flat_map(|service| units_of(config, service))
                .collect(),
            infra: serde_json::to_value(&config.infra).unwrap_or(Value::Null),
        }
    }
}

/// The unit names of `service` in `config`: the service name, or `<service>.<N>` for
/// each replica when there are several, as the resolver lays them out.
fn units_of(config: &devlanes_config::ProjectConfig, service: &str) -> Vec<String> {
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

/// The running service units of a lane with their fingerprints.
fn running_services(st: &LaneSt) -> Vec<(String, Option<Arc<Fingerprint>>)> {
    st.units
        .iter()
        .filter(|(_, rec)| rec.kind == Kind::Service && rec.host.is_alive())
        .map(|(unit, rec)| (unit.clone(), rec.fingerprint.clone()))
        .collect()
}

impl Inner {
    /// Recomputes the lane's drift against its current config. Nothing running, or
    /// no loaded config, leaves no drift: the error a broken file gives is the lane's
    /// reason instead. A lane whose plan fails keeps no per-unit drift either.
    pub(crate) async fn update_drift(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        let (config, running, orphans, started, profiles, in_flight, generation) = {
            let mut st = lane.st();
            // Taken now: a later update, start or stop bumps it, and this one's
            // result, planned from an older state, is then dropped.
            st.drift_generation += 1;
            (
                st.config.loaded().cloned(),
                running_services(&st),
                st.orphans.keys().cloned().collect::<BTreeSet<String>>(),
                st.started_with.clone(),
                st.profiles.clone(),
                !st.starts.is_empty(),
                st.drift_generation,
            )
        };
        if running.is_empty() {
            let mut st = lane.st();
            st.drift.clear();
            st.drift_error = None;
            st.infra_changed = false;
            // A start in flight spawns from the config it began with; its end decides.
            if !in_flight {
                st.config_changed = false;
            }
            return;
        }
        let Some(config) = config else {
            // A file that does not load still changed what the running units came from.
            let mut st = lane.st();
            st.drift.clear();
            st.drift_error = None;
            st.config_changed = true;
            return;
        };
        let mut drift = BTreeMap::new();
        // Only the running services the file still has are planned: each plan can
        // cost a contributor call.
        let mut only = BTreeSet::new();
        for (unit, _) in &running {
            match service_of(&config, unit) {
                Some(service) => {
                    only.insert(format!("services.{service}"));
                }
                None => {
                    drift.insert(unit.clone(), UnitDrift::Removed);
                }
            }
        }
        let planned = if only.is_empty() {
            None
        } else {
            Some(
                self.resolve_config(project, lane, config.clone(), Some(&only))
                    .await,
            )
        };
        let plan_error = match &planned {
            Some(Err(error)) => Some(error.clone()),
            _ => None,
        };
        if let Some(Ok(resolved)) = &planned {
            for (unit, fingerprint) in &running {
                if drift.contains_key(unit) {
                    continue;
                }
                // Fewer replicas leave the extra ones out of the plan.
                let Some(service) = resolved.services.get(unit) else {
                    drift.insert(unit.clone(), UnitDrift::Removed);
                    continue;
                };
                // A unit without one was started by a daemon that did not keep it.
                let Some(fingerprint) = fingerprint else {
                    continue;
                };
                let fields = fingerprint.changes(&Fingerprint::of(service));
                if !fields.is_empty() {
                    drift.insert(unit.clone(), UnitDrift::Changed(fields));
                }
            }
        }
        let selected = devlanes_resolve::select_profiles(&config, &profiles, &[])
            .map(|selection| selection.services)
            .unwrap_or_default();
        let laid_out: Vec<(String, String)> = lane
            .st()
            .view
            .as_ref()
            .map(|view| {
                view.services
                    .iter()
                    .map(|service| (service.name.clone(), service.unit.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // Without a whole start to compare with (a lane brought back next to a previous
        // daemon's processes), nothing counts as added.
        if let Some(started) = &started {
            for (service, unit) in laid_out {
                // A previous daemon's process runs the unit too, unsupervised.
                let alive =
                    running.iter().any(|(running, _)| *running == unit) || orphans.contains(&unit);
                if selected.contains(&service) && !started.units.contains(&unit) && !alive {
                    drift.insert(unit, UnitDrift::Added);
                }
            }
        }
        let infra_changed = started.is_some_and(|started| {
            serde_json::to_value(&config.infra).unwrap_or(Value::Null) != started.infra
        });
        let mut st = lane.st();
        if st.drift_generation != generation {
            return;
        }
        // A plan that fails also means the file no longer matches what runs.
        st.config_changed = !drift.is_empty() || infra_changed || plan_error.is_some();
        st.drift = drift;
        st.drift_error = plan_error;
        st.infra_changed = infra_changed;
    }

    /// After a start: the units it spawned match the file when the start used the
    /// lane's current config, so only their drift goes, without a new plan. A start
    /// that planned from a config edited meanwhile gets a full recomputation.
    pub(crate) fn settle_drift(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        used: &Arc<devlanes_config::ProjectConfig>,
        spawned: &[String],
    ) {
        // A unit a reload added counts as started: stopping it later is on purpose.
        if let Some(started) = &mut lane.st().started_with {
            started.units.extend(spawned.iter().cloned());
        }
        let current = lane.st().config.loaded().cloned();
        let same = current.is_some_and(|current| {
            Arc::ptr_eq(&current, used)
                || serde_json::to_value(current.as_ref()).ok()
                    == serde_json::to_value(used.as_ref()).ok()
        });
        if !same {
            self.spawn_drift(project, lane);
            return;
        }
        let mut st = lane.st();
        st.drift_generation += 1;
        st.drift.retain(|unit, _| !spawned.contains(unit));
        st.config_changed = !st.drift.is_empty() || st.infra_changed;
    }

    /// After a stop: drift of units that no longer run goes, except `added`, which
    /// is about units that do not run.
    pub(crate) fn prune_drift(&self, lane: &LaneRt) {
        let mut st = lane.st();
        st.drift_generation += 1;
        let alive: BTreeSet<String> = running_services(&st)
            .into_iter()
            .map(|(unit, _)| unit)
            .collect();
        st.drift
            .retain(|unit, drift| *drift == UnitDrift::Added || alive.contains(unit));
        if alive.is_empty() {
            st.drift.clear();
            st.infra_changed = false;
        }
        st.config_changed = !st.drift.is_empty() || st.infra_changed;
    }

    /// [`Self::update_drift`] in the background, then a `lane.changed`, for callers
    /// that must not wait for a plan (a start's result, a reload).
    pub(crate) fn spawn_drift(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        let (engine, project, lane) = (self.clone(), project.clone(), lane.clone());
        tokio::spawn(async move {
            engine.update_drift(&project, &lane).await;
            engine.emit_lane(&project, &lane);
        });
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
        let ((project, lane), selectors) = self.lane_for_units(&selection.units, context).await?;
        self.fresh_config(&project, &lane).await;
        let name = project.name();
        let reference = lane.st().reference(&name);
        lane.st().config.require(&reference)?;
        self.update_drift(&project, &lane).await;
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
            // Names the lane does not have are a typo, not "nothing to reload".
            let known = |selector: &str| {
                st.units.contains_key(selector)
                    || st.orphans.contains_key(selector)
                    || st.view.as_ref().is_some_and(|view| {
                        view.services
                            .iter()
                            .any(|service| service.unit == selector || service.name == selector)
                    })
            };
            if let Some(unknown) = selectors.iter().find(|selector| !known(selector)) {
                let mut candidates: Vec<String> = st.units.keys().cloned().collect();
                candidates.extend(
                    st.view
                        .iter()
                        .flat_map(|view| view.services.iter().map(|s| s.unit.clone())),
                );
                candidates.sort();
                candidates.dedup();
                return Err(RpcError::new(
                    "not_found",
                    json!({"reference": unknown, "candidates": candidates}),
                    3,
                ));
            }
            // A plan that failed has no per-unit answer; its error is the answer.
            if st.drift.is_empty()
                && let Some(error) = &st.drift_error
            {
                return Err(error.clone());
            }
            let picked = |unit: &str| {
                selectors.is_empty()
                    || selectors.iter().any(|selector| {
                        selector == unit
                            || unit
                                .rsplit_once('.')
                                .is_some_and(|(base, _)| base == selector)
                    })
            };
            let drift: BTreeMap<String, UnitDrift> = st
                .drift
                .iter()
                .filter(|(unit, _)| picked(unit))
                .map(|(unit, drift)| (unit.clone(), drift.clone()))
                .collect();
            (
                drift,
                selectors.is_empty() && (st.infra_changed || st.ports_pending),
            )
        };
        let options = |start_id| StartOptions {
            selectors: Vec::new(),
            profiles: Vec::new(),
            transient: false,
            env: carries_env(context),
            overrides: EnvMap::new(),
            actor: context.actor.clone(),
            timeout: self.timeout(selection.timeout_ms),
            session: None,
            setup_only: false,
            all_at_once: selection.all_at_once,
            start_id: Some(start_id),
            arrival,
            restoring: false,
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
        self.prune_drift(&lane);
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
            st.config.require(&reference)?;
            if let Some(reason) = &st.unassigned {
                return Err(reason.clone());
            }
            if st.record.is_none() {
                return Err(RpcError::new("unassigned", json!({"lane": reference}), 1));
            }
            if options.env.is_none() && st.env.is_none() {
                return Err(RpcError::new("needs_env", json!({"lane": reference}), 1));
            }
        }
        // Transient: the stop leaves the saved profiles and services for the start.
        self.stop_lane(project, lane, None, None, true).await?;
        self.start_lane(project, lane, options).await
    }
}
