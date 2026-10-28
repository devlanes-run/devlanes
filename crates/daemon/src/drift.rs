//! Drift: how a lane's running units differ from what its project file would start
//! now, per unit, and `reload`, which applies only that. Everything here lives in
//! memory: a fingerprint holds environment values, so it never reaches state.db.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

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
            env: service.launch.env.values.clone(),
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

/// What the lane's profiles selected at its last whole start, and the infra settings
/// it started with. A service the profiles select now but did not then is `added`;
/// one the user left out on purpose (`start api` alone) never is.
#[derive(Debug, Clone)]
pub(crate) struct StartedWith {
    pub services: Vec<String>,
    pub infra: Value,
}

impl StartedWith {
    /// Taken when a whole-lane start begins, with the profiles it starts with.
    pub(crate) fn new(config: &devlanes_config::ProjectConfig, profiles: &[String]) -> Self {
        Self {
            services: devlanes_resolve::select_profiles(config, profiles, &[])
                .map(|selection| selection.services)
                .unwrap_or_default(),
            infra: serde_json::to_value(&config.infra).unwrap_or(Value::Null),
        }
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
        let (config, running, started, profiles, in_flight) = {
            let st = lane.st();
            (
                st.config.loaded().cloned(),
                running_services(&st),
                st.started_with.clone(),
                st.profiles.clone(),
                !st.starts.is_empty(),
            )
        };
        if running.is_empty() {
            let mut st = lane.st();
            st.drift.clear();
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
        let plan_failed = matches!(planned, Some(Err(_)));
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
        let before: &[String] = started.as_ref().map_or(&[], |s| &s.services);
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
        for (service, unit) in laid_out {
            let alive = running.iter().any(|(running, _)| *running == unit);
            if selected.contains(&service) && !before.contains(&service) && !alive {
                drift.insert(unit, UnitDrift::Added);
            }
        }
        let infra_changed = started.is_some_and(|started| {
            serde_json::to_value(&config.infra).unwrap_or(Value::Null) != started.infra
        });
        let mut st = lane.st();
        // A plan that fails also means the file no longer matches what runs.
        st.config_changed = !drift.is_empty() || infra_changed || plan_failed;
        st.drift = drift;
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
        st.drift.retain(|unit, _| !spawned.contains(unit));
        st.config_changed = !st.drift.is_empty() || st.infra_changed;
    }

    /// After a stop: drift of units that no longer run goes, except `added`, which
    /// is about units that do not run.
    pub(crate) fn prune_drift(&self, lane: &LaneRt) {
        let mut st = lane.st();
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
            result.warnings.push(LaneWarning {
                code: "reload_restarted_lane".into(),
                data: json!({
                    "lane": reference,
                    "message": "infra or ports changed, so the whole lane restarted",
                }),
            });
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
                    "message": format!("{reference} already runs what its project file says"),
                }),
            });
        }
        self.prune_drift(&lane);
        self.emit_lane(&project, &lane);
        serde_json::to_value(result).map_err(|e| internal("encode", e))
    }

    /// Restarts the whole lane: stops everything, keeping what the lane saved, then
    /// starts it as a plain `start` would, so infra, setup, tasks and a waiting port
    /// block move all happen.
    pub(crate) async fn restart_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        options: StartOptions,
    ) -> Result<StartResult, RpcError> {
        // Transient: the stop leaves the saved profiles and services for the start.
        self.stop_lane(project, lane, None, None, true).await?;
        self.start_lane(project, lane, options).await
    }
}
