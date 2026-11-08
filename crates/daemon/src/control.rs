//! Mutating methods: start, stop, restart, resume, lanes, exec, run, infra
//! and reset. Each returns structured data only.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::Path,
    sync::Arc,
    time::Duration,
};

use devlanes_config::OpaqueExtensions;
use devlanes_core::{
    LaunchSpec, TerminalSize,
    contrib::runtime_ca_env,
    env::{EnvLayer, assemble},
    ports::PortName,
};
use devlanes_proto::{
    Context, ExecParams, Method, Page, ResumePlan, RpcError, RunParams, Selection, UnitSummary,
    events,
};
use devlanes_resolve::ResolvedLane;
use devlanes_state::DesiredState;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    Session,
    engine::{Inner, LanePair, carries_env, confirmation, params, required},
    lifecycle::{Cleaned, SpawnRequest, StartOptions, TaskOutcome},
    model::{EnvMap, Kind, LaneRt, LaneView, ProjectRt, is_mine, lane_summary, lock, unit_summary},
    registry::{PrimaryWorktree, internal, store_error},
    stream::LaneEvents,
    target::{Found, LaneOrFile, Policy},
    units::{RunGoal, UnitHost},
};

/// `resume` parameters.
#[derive(Debug, Default, Deserialize)]
struct ResumeParams {
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Stream the starts' lane events on the request.
    #[serde(default)]
    events: bool,
    /// Bypass the start gate, as `start --all-at-once` does. The lanes start
    /// concurrently either way; without it they queue in the gate together.
    #[serde(default)]
    all_at_once: bool,
}

/// `lane.new` parameters.
#[derive(Debug, Deserialize)]
struct LaneNewParams {
    /// Sent by CLIs that still had `lane new <branch>`. It is refused so such
    /// a CLI does not add an extra lane for the folder instead.
    #[serde(default)]
    branch: Option<String>,
    /// Name of that extra lane.
    #[serde(default)]
    name: Option<String>,
    /// Sent by CLIs that started the new lane through `lane.new`. It is refused
    /// before a lane is created, so such a CLI does not leave one unstarted.
    #[serde(default)]
    start: bool,
}

/// `lane.clean` parameters.
#[derive(Debug, Default, Deserialize)]
struct LaneCleanParams {
    #[serde(default)]
    lane: Option<String>,
}

/// `run` parameters: proto's [`RunParams`] plus the CLI's timeout.
#[derive(Debug, Default, Deserialize)]
struct RunArgs {
    #[serde(flatten)]
    run: RunParams,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// A `run` of one `infra.setup` or `infra.teardown` entry.
struct InfraEntry {
    /// `setup` or `teardown`.
    phase: &'static str,
    entry: String,
    detach: bool,
    timeout_ms: Option<u64>,
}

/// A lane `lane gc` collects: its worktree is gone, or the worktree no longer holds the
/// project's file (deleted, or now naming another project). Never collected on its own,
/// since a branch switch back would find the lane gone.
fn stale(st: &crate::model::LaneSt) -> bool {
    !st.present
        || matches!(
            st.config,
            crate::model::ConfigState::Missing | crate::model::ConfigState::Elsewhere(_)
        )
}

/// Splits `infra.setup.<entry>` or `infra.teardown.<entry>` into the phase and
/// the entry; `None` for a task name.
fn infra_entry(task: &str) -> Option<(&'static str, &str)> {
    if let Some(entry) = task.strip_prefix("infra.setup.") {
        return Some(("setup", entry));
    }
    task.strip_prefix("infra.teardown.")
        .map(|entry| ("teardown", entry))
}

/// Waits until this run of `unit` is listed on the lane. The client attaches
/// to it next, so an earlier run that ended does not count, or the client
/// would attach to its finished screen. A run that could not start (its
/// previous run still going) answers with its error.
async fn await_spawn(
    lane: &LaneRt,
    unit: &str,
    previous: Option<&UnitHost>,
    outcome: &mut tokio::sync::oneshot::Receiver<Option<RpcError>>,
) -> Result<(), RpcError> {
    for _ in 0..100 {
        let spawned = lane
            .st()
            .units
            .get(unit)
            .is_some_and(|rec| previous.is_none_or(|previous| !rec.host.same_run(previous)));
        if spawned {
            return Ok(());
        }
        match outcome.try_recv() {
            Ok(Some(error)) => return Err(error),
            Ok(None) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => return Ok(()),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

/// The answer to a `run` that waited for its task or entry to end. An entry
/// with `ignore_errors` still answers with its real exit code: it was run on
/// its own, to see whether it works.
fn run_result(task: &str, lane: &str, unit: Option<&str>, outcome: &TaskOutcome) -> Value {
    let code = outcome.code;
    let mut result = json!({
        "task": task,
        "lane": lane,
        "unit": unit,
        "interactive": false,
        "exit_code": code,
        "state": outcome.state.word(),
    });
    if let crate::units::UnitState::Failed(Some(reason)) = &outcome.state {
        result["reason"] = json!(reason);
    }
    // Daemons the task started (`pg_ctl start`) outlive it on purpose; say so.
    if !outcome.leftovers.is_empty() {
        result["leftovers"] = json!(outcome.leftovers);
    }
    result
}

impl Inner {
    /// Wait limit of a request.
    pub(crate) fn timeout(&self, timeout_ms: Option<u64>) -> Duration {
        timeout_ms.map_or(self.opts.start_timeout, Duration::from_millis)
    }

    /// `start`: the lane pipeline, then waits for ready/failed (StartResult).
    pub(crate) async fn start_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        // First of all: the start gate serves requests in the order they came in,
        // whatever their lane lookup, infra or tasks take.
        let arrival = self.arrive();
        let selection: Selection = params(value)?;
        let ((project, lane), selectors) = self
            .lane_for_units(&selection.units, context, Policy::of(Method::Start))
            .await?;
        self.fresh_config(&project, &lane).await;
        Self::refuse_external(&project, &lane, &selectors)?;
        let start_id = self.new_start_id();
        let stream = selection
            .events
            .then(|| LaneEvents::starts(vec![(project.clone(), lane.clone())], vec![start_id]));
        let start = self.start_lane(
            &project,
            &lane,
            StartOptions {
                selectors,
                profiles: selection.profile_names(),
                transient: selection.transient,
                env: carries_env(context),
                overrides: selection.env.clone(),
                actor: context.actor.clone(),
                timeout: self.timeout(selection.timeout_ms),
                session: None,
                setup_only: false,
                all_at_once: selection.all_at_once,
                start_id: Some(start_id),
                arrival,
                restoring: false,
            },
        );
        let mut result = self.with_lane_events(session, stream, start).await?;
        // Only this lane came back; the shell's env may not suit the others, so this
        // project's other lanes are named for `lane resume` instead of started too.
        // `others` counts the other projects' lanes, which `lane resume` also starts;
        // naming them on every start in any project would only be noise.
        if selection.units.is_empty() && selection.profile_names().is_empty() {
            let mut waiting: Vec<String> = Vec::new();
            let mut others = 0;
            for (other_project, other) in self.all_lanes() {
                let st = other.st();
                if Arc::ptr_eq(&other, &lane) || !st.present || !st.needs_env() {
                    continue;
                }
                if other_project.id == project.id {
                    waiting.push(st.reference(&other_project.name()));
                } else {
                    others += 1;
                }
            }
            if !waiting.is_empty() {
                result.warnings.push(devlanes_proto::LaneWarning {
                    code: "lanes_to_resume".into(),
                    data: json!({"lanes": waiting, "others": others}),
                });
            }
        }
        serde_json::to_value(result).map_err(|e| internal("encode", e))
    }

    /// `stop`: selected units, the whole lane, or (`mine`) the caller's units.
    pub(crate) async fn stop_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let selection: Selection = params(value)?;
        let mut stopped: Vec<UnitSummary> = Vec::new();
        // `stop --mine` across the lanes of its scope (the current project, or every
        // project outside one): one lane's failure must not leave the rest running.
        // In a project nobody started, `stop` with or without units stops nothing.
        let mut errors: Vec<Value> = Vec::new();
        let mut warnings: Vec<Value> = Vec::new();
        let policy = Policy::of(Method::Stop);
        if selection.mine && selection.units.is_empty() && context.lane.is_none() {
            let scope = self.read_scope(context, false, policy).await?.lanes();
            for (project, lane) in &scope {
                if lane.st().any_running() {
                    match self
                        .stop_lane(project, lane, None, Some(&context.actor), false)
                        .await
                    {
                        Ok(units) => {
                            stopped.extend(units);
                            self.prune_drift(lane);
                        }
                        Err(error) => errors.push(json!({
                            "lane": lane.st().reference(&project.name()),
                            "error": error,
                        })),
                    }
                }
            }
            if let Some(warning) = self.mine_elsewhere(&scope, &context.actor) {
                warnings.push(warning);
            }
        } else if let Some(((project, lane), selectors)) = self
            .units_or_nothing(&selection.units, context, policy)
            .await?
        {
            Self::refuse_external(&project, &lane, &selectors)?;
            let selectors = (!selectors.is_empty()).then_some(selectors);
            let mine = selection.mine.then_some(&context.actor);
            stopped = self
                .stop_lane(&project, &lane, selectors, mine, selection.transient)
                .await?;
            // A stopped unit has no drift left to report.
            self.prune_drift(&lane);
        }
        let mut result =
            serde_json::to_value(Page::complete(stopped)).map_err(|e| internal("encode", e))?;
        if !errors.is_empty() {
            result["errors"] = json!(errors);
        }
        if !warnings.is_empty() {
            result["warnings"] = json!(warnings);
        }
        Ok(result)
    }

    /// A `mine_elsewhere` warning naming the lanes outside `scope` where `actor`'s
    /// units still run, so a narrowed `stop --mine` does not leave them unnoticed.
    fn mine_elsewhere(&self, scope: &[LanePair], actor: &devlanes_proto::Actor) -> Option<Value> {
        let mut lanes = Vec::new();
        let mut projects = BTreeSet::new();
        for (project, lane) in self.all_lanes() {
            if scope.iter().any(|(_, inside)| Arc::ptr_eq(inside, &lane)) {
                continue;
            }
            let st = lane.st();
            let mine = st
                .units
                .values()
                .any(|rec| rec.host.is_alive() && is_mine(&rec.host.info().actor, actor))
                || st
                    .orphans
                    .values()
                    .any(|orphan| is_mine(&orphan.actor, actor));
            if mine {
                lanes.push(st.reference(&project.name()));
                projects.insert(project.name());
            }
        }
        (!lanes.is_empty()).then(
            || json!({"code": "mine_elsewhere", "data": {"lanes": lanes, "projects": projects}}),
        )
    }

    /// `restart`: stops and starts named units with fresh plans; without units, the
    /// whole lane, as a stop and a plain start that keep what the lane saved.
    pub(crate) async fn restart_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let arrival = self.arrive();
        let selection: Selection = params(value)?;
        let ((project, lane), selectors) = self
            .lane_for_units(&selection.units, context, Policy::of(Method::Restart))
            .await?;
        self.fresh_config(&project, &lane).await;
        Self::refuse_external(&project, &lane, &selectors)?;
        let start_id = self.new_start_id();
        let stream = selection
            .events
            .then(|| LaneEvents::starts(vec![(project.clone(), lane.clone())], vec![start_id]));
        let options = StartOptions {
            selectors: selectors.clone(),
            profiles: selection.profile_names(),
            transient: false,
            env: carries_env(context),
            overrides: selection.env.clone(),
            actor: context.actor.clone(),
            timeout: self.timeout(selection.timeout_ms),
            session: None,
            setup_only: false,
            all_at_once: selection.all_at_once,
            start_id: Some(start_id),
            arrival,
            restoring: false,
        };
        let result = if selectors.is_empty() {
            let restart = self.restart_lane(&project, &lane, options);
            self.with_lane_events(session, stream, restart).await?
        } else {
            let restart = self.restart_units(&project, &lane, &selectors, options);
            self.with_lane_events(session, stream, restart).await?
        };
        serde_json::to_value(result).map_err(|e| internal("encode", e))
    }

    /// `resume`: starts every lane whose desired state is running, with this shell's env.
    pub(crate) async fn resume_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let arrival = self.arrive();
        let args: ResumeParams = params(value)?;
        let Some(env) = carries_env(context) else {
            return Err(RpcError::new("needs_env", json!({}), 1));
        };
        let wanted: Vec<LanePair> = self
            .all_lanes()
            .into_iter()
            .filter(|(_, lane)| {
                let st = lane.st();
                st.present
                    && !st.services_alive()
                    && st
                        .record
                        .as_ref()
                        .is_some_and(|r| r.desired_state == DesiredState::Running)
            })
            .collect();
        let starts: Vec<u64> = wanted.iter().map(|_| self.new_start_id()).collect();
        let stream = args
            .events
            .then(|| LaneEvents::starts(wanted.clone(), starts.clone()));
        // Concurrently, under the one arrival of this request: the lanes queue in
        // the start gate together instead of one after another's readiness.
        let resume = async {
            let results = futures_util::future::join_all(wanted.iter().zip(starts).map(
                |((project, lane), start_id)| {
                    self.start_lane(
                        project,
                        lane,
                        StartOptions {
                            selectors: Vec::new(),
                            profiles: Vec::new(),
                            transient: false,
                            env: Some(env.clone()),
                            overrides: Some(EnvMap::new()),
                            actor: context.actor.clone(),
                            timeout: self.timeout(args.timeout_ms),
                            session: None,
                            setup_only: false,
                            all_at_once: args.all_at_once,
                            start_id: Some(start_id),
                            arrival,
                            restoring: false,
                        },
                    )
                },
            ))
            .await;
            let mut items = Vec::new();
            for result in results {
                items.push(match result {
                    Ok(result) => {
                        serde_json::to_value(result).map_err(|e| internal("encode", e))?
                    }
                    Err(error) => json!({"lane": error.data.get("lane"), "error": error}),
                });
            }
            Ok(json!({"items": items, "truncated": false, "next_cursor": null}))
        };
        self.with_lane_events(session, stream, resume).await
    }

    /// Starts the lanes of a replacing daemon's resume plan, concurrently under
    /// one arrival as [`Self::resume_method`] does, with the plan's services,
    /// profiles, environment and actor. Ordinary starts: their events reach
    /// every subscriber. A lane that no longer resolves or fails to start is
    /// logged and never stops the daemon. The plan, with the client's
    /// environment, is dropped when this returns.
    pub(crate) async fn resume_planned(self: &Arc<Self>, plan: ResumePlan) {
        let arrival = self.arrive();
        let ResumePlan { lanes, env, actor } = plan;
        let context = Context {
            env: Some(env),
            actor,
            ..Context::default()
        };
        let env = carries_env(&context);
        tracing::info!(
            lanes = lanes.len(),
            "resuming the lanes that ran before the restart"
        );
        let starts = lanes.into_iter().map(|planned| {
            let (context, env) = (&context, env.clone());
            async move {
                let context = &Context {
                    lane: Some(planned.lane.clone()),
                    ..context.clone()
                };
                let (project, lane) = match self.lane(context, Policy::of(Method::Resume)).await {
                    Ok(pair) => pair,
                    Err(error) => {
                        tracing::warn!(lane = %planned.lane, code = %error.code, "a lane to resume was not found");
                        return;
                    }
                };
                self.fresh_config(&project, &lane).await;
                let started = self
                    .start_lane(
                        &project,
                        &lane,
                        StartOptions {
                            selectors: planned.units,
                            profiles: planned.profiles,
                            transient: false,
                            env,
                            overrides: Some(EnvMap::new()),
                            actor: context.actor.clone(),
                            timeout: self.opts.start_timeout,
                            session: None,
                            setup_only: false,
                            all_at_once: false,
                            start_id: None,
                            arrival,
                            restoring: true,
                        },
                    )
                    .await;
                match started {
                    Ok(result) => tracing::info!(
                        lane = %planned.lane,
                        ready = result.ready,
                        failures = result.failures.len(),
                        timed_out = result.timed_out,
                        "lane resumed"
                    ),
                    Err(error) => {
                        tracing::warn!(lane = %planned.lane, code = %error.code, "a lane did not resume");
                    }
                }
            }
        });
        futures_util::future::join_all(starts).await;
    }

    /// `lane.new`: another lane for the cwd's folder. Clients start it with a
    /// plain `start` on the returned lane.
    pub(crate) async fn lane_new(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: LaneNewParams = required(value)?;
        if let Some(branch) = &args.branch {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": format!(
                    "devlanes no longer creates worktrees; run `git worktree add <path> {branch}`, then `devlanes start` in it"
                )}),
                2,
            ));
        }
        if args.start {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": "this devlanes client is older than the daemon; run devlanes lane new, then devlanes start --lane <name>"}),
                2,
            ));
        }
        // The folder the new lane shares: `--lane`'s (an extra lane stands for its
        // folder's first lane), else the cwd's, never a guessed one.
        let (project, lane) = self.lane(context, Policy::of(Method::LaneNew)).await?;
        let path = lane.st().path.clone();
        let base = Self::lane_at(&project, &path).unwrap_or(lane);
        let (lane, extra) = self
            .extra_lane(&project, &base, args.name.as_deref())
            .await?;
        let name = project.name();
        let (summary, path) = {
            let st = lane.st();
            (lane_summary(&name, &st), st.path.clone())
        };
        // The project's folder, which is below the worktree root when the
        // project file is in a subfolder. `join("")` would add a separator.
        let (config_dir, _) = project.config_path();
        let folder = if config_dir.as_os_str().is_empty() {
            path
        } else {
            path.join(config_dir)
        };
        Ok(json!({"lane": summary, "name": extra, "folder": folder}))
    }

    /// Another lane for `base`'s folder (`lane new`): its own worktree row,
    /// `<base identity>#lane=<name>`, so it gets its own index, ports and domain
    /// while the folder's first lane stays the default. The row and the runtime
    /// exist while the lane does or waits for one (`lanes.max`), and the name
    /// stays taken that long; `lane clean` ends both, so the name gives a new
    /// lane afterwards.
    async fn extra_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        base: &Arc<crate::model::LaneRt>,
        name: Option<&str>,
    ) -> Result<(Arc<crate::model::LaneRt>, String), RpcError> {
        self.claim_lane(project, base).await;
        let (identity, path, branch, reason) = {
            let st = base.st();
            let reason = st.unassigned.clone().or_else(|| match &st.config {
                crate::model::ConfigState::Invalid(error) => Some(error.clone()),
                _ => None,
            });
            (
                st.identity.clone(),
                st.path.clone(),
                st.branch.clone(),
                reason,
            )
        };
        if base.st().record.is_none() {
            // The folder's first lane comes first; it could not get one.
            return Err(reason.unwrap_or_else(|| {
                RpcError::new(
                    "unassigned",
                    json!({"lane": base.st().reference(&project.name())}),
                    1,
                )
            }));
        }
        // Under `sync` from the name check until the lane is in the map, so two
        // requests cannot both take one name.
        let sync = project.sync.lock().await;
        // The lane a name already reaches; record names (`main`, `lane<N>`) are
        // refused below.
        let holder = |candidate: &str| {
            project.lanes().iter().find_map(|lane| {
                let st = lane.st();
                let holds = st.alias().is_some_and(|(_, alias)| alias == candidate)
                    || st.branch.as_deref() == Some(candidate);
                holds.then(|| st.reference(&project.name()))
            })
        };
        let name = match name {
            Some(name) => {
                let index_name = name
                    .strip_prefix("lane")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
                let valid = !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                    && !name.bytes().all(|b| b.is_ascii_digit())
                    && name != "main"
                    && !index_name;
                if !valid {
                    return Err(RpcError::new(
                        "invalid_reference",
                        json!({"reference": name, "message": "a lane name uses letters, digits, `-`, `_` and `.`, is not a number, and is not main or lane<N>"}),
                        2,
                    ));
                }
                if let Some(lane) = holder(name) {
                    return Err(RpcError::new(
                        "name_taken",
                        json!({"name": name, "project": project.name(), "kind": "lane", "lane": lane}),
                        1,
                    ));
                }
                name.to_owned()
            }
            None => {
                let dir = crate::model::dir_name(&path);
                (2..)
                    .map(|n| format!("{dir}-{n}"))
                    .find(|candidate| holder(candidate).is_none())
                    .unwrap_or_else(|| format!("{dir}-extra"))
            }
        };
        let identity = format!("{identity}{}{name}", crate::model::ALIAS_MARK);
        let record = self
            .store
            .upsert_worktree(devlanes_state::WorktreeInput {
                project_id: project.id,
                identity: identity.clone(),
                path: path.clone(),
                branch: branch.clone(),
                present: true,
            })
            .await
            .map_err(store_error)?;
        let lane = crate::registry::new_lane(
            record.id,
            crate::model::LaneSt::new(identity, path, branch, true),
        );
        project.st().lanes.insert(record.id, lane.clone());
        drop(sync);
        self.reload_config(project, &lane).await;
        self.claim_lane(project, &lane).await;
        if lane.st().record.is_none() {
            let st = lane.st();
            return Err(st.unassigned.clone().unwrap_or_else(|| {
                RpcError::new(
                    "unassigned",
                    json!({"lane": st.reference(&project.name())}),
                    1,
                )
            }));
        }
        Ok((lane, name))
    }

    /// `lane.clean` (requires `yes`). A lane another clean ended first is
    /// `not_found`.
    pub(crate) async fn lane_clean_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: LaneCleanParams = params(value)?;
        let context = &Context {
            lane: args.lane.or_else(|| context.lane.clone()),
            ..context.clone()
        };
        // Never the main lane by guess: with `--yes` there is no prompt to stop it.
        let (project, lane) = self.lane(context, Policy::of(Method::LaneClean)).await?;
        let name = lane.st().reference(&project.name());
        if !context.yes {
            return Err(confirmation(json!({"lane": name, "flag": "--yes"})));
        }
        match self
            .clean_lane(
                &project,
                &lane,
                Some(&context.actor),
                crate::lifecycle::Logs::Delete,
            )
            .await?
        {
            Cleaned::Done(result) => Ok(result),
            Cleaned::Gone => Err(RpcError::new(
                "not_found",
                json!({"reference": name, "candidates": []}),
                3,
            )),
        }
    }

    /// `lane.gc`: cleans lanes whose worktrees are gone or whose worktree no longer
    /// holds the project's file (deleted, or naming another project after a rename),
    /// drops port offsets no config uses any more, and deletes the lane data folders
    /// a clean could not (`data_left` lists the ones still held). `shared_infra`
    /// holds, by lane, what a clean of a project's last lane did to its shared
    /// containers, as `lane.clean` reports it. Needs `yes` only
    /// when there are lanes to clean; the confirmation lists them under `lanes`.
    pub(crate) async fn lane_gc(self: &Arc<Self>, context: &Context) -> Result<Value, RpcError> {
        // At a folder shared by several projects, each of them is re-listed; in a
        // project nobody started, none.
        let projects = match self.projects(context, Policy::of(Method::LaneGc)).await? {
            Found::Projects { projects, .. } => projects,
            Found::Unregistered { .. } | Found::Nothing { .. } => Vec::new(),
        };
        for project in &projects {
            let _ = self.relist_only(project).await;
        }
        if !context.yes {
            let lanes: Vec<String> = self
                .all_lanes()
                .iter()
                .filter_map(|(project, lane)| {
                    let st = lane.st();
                    let running = st.any_running() || st.busy.is_some();
                    // A gone worktree that never was a lane is dropped unasked.
                    (stale(&st) && !running && st.record.is_some())
                        .then(|| st.reference(&project.name()))
                })
                .collect();
            if !lanes.is_empty() {
                return Err(confirmation(json!({"flag": "--yes", "lanes": lanes})));
            }
        }
        let mut cleaned = Vec::new();
        let mut kept_running = Vec::new();
        let mut shared = serde_json::Map::new();
        for (project, lane) in self.all_lanes() {
            let (present, stale, assigned, reference, running) = {
                let st = lane.st();
                (
                    st.present,
                    stale(&st),
                    st.record.is_some(),
                    st.reference(&project.name()),
                    st.any_running() || st.busy.is_some(),
                )
            };
            if !stale || present && !assigned {
                continue;
            }
            // A presence flag can be wrong (a re-list from elsewhere, a racing
            // watcher); a lane that runs something is never collected.
            if running {
                kept_running.push(reference);
                continue;
            }
            // Gone only after the check above: it was not confirmed. A gone
            // worktree that never was a lane is dropped unasked.
            if assigned && !context.yes {
                continue;
            }
            let done = self
                .clean_lane(
                    &project,
                    &lane,
                    Some(&context.actor),
                    crate::lifecycle::Logs::Keep,
                )
                .await?;
            // `Gone`: another clean got there first.
            let Cleaned::Done(done) = done else {
                continue;
            };
            // The project's last lane, record-less or not, stopped its shared
            // containers or says why it could not.
            if let Some(stop) = done.get("shared_infra") {
                shared.insert(reference.clone(), stop.clone());
            }
            if assigned {
                cleaned.push(reference);
            }
        }
        let mut removed = 0;
        for project in self.all_projects() {
            let configs: Vec<_> = project
                .lanes()
                .iter()
                .filter_map(|lane| lane.st().config.loaded().cloned())
                .collect();
            if configs.is_empty() {
                continue;
            }
            // Offsets are per project, so a name any worktree's config needs stays;
            // one config that cannot say what it needs keeps everything.
            let needs: Result<Vec<_>, _> = configs
                .iter()
                .map(|config| devlanes_resolve::port_needs(config, &self.integ.contributors))
                .collect();
            let Ok(needs) = needs else {
                continue;
            };
            let mut live: BTreeSet<PortName> = BTreeSet::new();
            let mut reserved: BTreeSet<u16> = BTreeSet::new();
            for need in needs {
                live.extend(need.names);
                reserved.extend(need.reserved);
            }
            let existing = self
                .store
                .allocation(devlanes_state::BlockOwner::Project(project.id))
                .await
                .ok()
                .flatten();
            for lane in project.lanes() {
                if let Some(allocation) = &lane.st().allocation {
                    // Infra names come from the provider; keep them while their lane exists.
                    live.extend(
                        allocation
                            .offsets
                            .keys()
                            .filter(|name| name.unit.starts_with("infra."))
                            .cloned(),
                    );
                }
            }
            if let Some(existing) = existing {
                live.extend(
                    existing
                        .offsets
                        .keys()
                        .filter(|name| name.unit.starts_with("infra."))
                        .cloned(),
                );
            }
            removed += self
                .store
                .gc_offsets(project.id, live, reserved)
                .await
                .map_err(store_error)?;
        }
        for project in self.all_projects() {
            self.refresh_views(&project).await;
        }
        let mut result =
            json!({"cleaned": cleaned, "kept_running": kept_running, "offsets_removed": removed});
        if !shared.is_empty() {
            result["shared_infra"] = Value::Object(shared);
        }
        let data_left = self.delete_pending_data().await;
        if !data_left.is_empty() {
            result["data_left"] = json!(data_left);
        }
        Ok(result)
    }

    /// `exec`: an ad-hoc unit in the cwd's lane (kind `exec`).
    pub(crate) async fn exec_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: ExecParams = required(value)?;
        let Some((program, rest)) = args.command.split_first() else {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": "empty command"}),
                2,
            ));
        };
        if args.name.is_empty() || args.name.contains(['#', ':', '@', '/']) {
            return Err(RpcError::new(
                "invalid_reference",
                json!({"reference": args.name}),
                2,
            ));
        }
        let (project, lane) = self.lane(context, Policy::of(Method::Exec)).await?;
        self.fresh_config(&project, &lane).await;
        // An exec unit under a configured name would take that unit's place: its
        // logs, its stop and the next start of the real one.
        let taken = {
            let st = lane.st();
            let config = st.config.loaded();
            let service = config.is_some_and(|c| c.services.contains_key(&args.name))
                || st
                    .view
                    .as_ref()
                    .is_some_and(|view| view.services.iter().any(|s| s.unit == args.name));
            let task = config.is_some_and(|c| c.tasks.contains_key(&args.name));
            let external = config.is_some_and(|c| c.infra.external.contains_key(&args.name));
            match (service, task, external) {
                (true, ..) => Some("service"),
                (_, true, _) => Some("task"),
                (.., true) => Some(devlanes_proto::EXTERNAL_KIND),
                _ => None,
            }
            .map(|kind| (kind, st.reference(&project.name())))
        };
        if let Some((kind, reference)) = taken {
            return Err(RpcError::new(
                "name_taken",
                json!({"name": args.name, "kind": kind, "lane": reference}),
                2,
            ));
        }
        if let Some(env) = carries_env(context) {
            lane.st().env = Some(env.clone());
            project.st().last_env = Some(env);
        }
        // A client that sends no env (`devlanes mcp` first tries without) gets the
        // lane's, or `needs_env` instead of a process without PATH.
        if lane.st().env.is_none() {
            let reference = lane.st().reference(&project.name());
            return Err(RpcError::new("needs_env", json!({"lane": reference}), 1));
        }
        let (resolved, layers) = self
            .resolve_lane_env(&project, &lane, Arc::default(), None)
            .await?;
        let mut left_out = layers.unset.clone();
        left_out.retain(|key, _| !args.env.contains_key(key));
        let (client, reference) = {
            let st = lane.st();
            (
                st.env.as_deref().cloned().unwrap_or_default(),
                st.reference(&project.name()),
            )
        };
        let lane_env = resolved.lane_env();
        let runtime_ca = runtime_ca_env(lane_env, |name| {
            client.contains_key(name) || args.env.contains_key(name)
        });
        let cwd = if context.cwd.as_os_str().is_empty() {
            lane.st().path.clone()
        } else {
            context.cwd.clone()
        };
        let environment = assemble([
            (EnvLayer::Client, client),
            (EnvLayer::Contributor, runtime_ca),
            (EnvLayer::Devlanes, lane_env.clone()),
            (EnvLayer::Global, layers.global),
            (EnvLayer::Profile, layers.profile),
            (EnvLayer::Override, args.env.clone()),
        ]);
        let request = SpawnRequest {
            name: args.name.clone(),
            kind: Kind::Exec,
            spec: LaunchSpec {
                program: program.clone(),
                args: rest.to_vec(),
                cwd,
                env: environment.values.clone(),
                pty: args.pty,
                size: TerminalSize::default(),
            },
            env: Arc::new(environment),
            files: Vec::new(),
            actor: context.actor.clone(),
            ports: BTreeMap::new(),
            urls: Vec::new(),
            tags: Vec::new(),
            hidden: false,
            autostart: false,
            stop: devlanes_core::StopPolicy::default(),
            literal_port: None,
            preset: None,
            color: None,
            sync: Vec::new(),
            display_name: None,
            goal: RunGoal::Exit,
            role: "exec",
            keep_awake: false,
            fingerprint: None,
            notes: Vec::new(),
        };
        // Held until the unit is recorded, so a clean or reset that begins meanwhile
        // waits and then stops it, or refuses it once the lane is released.
        let (adhoc, _) = lane.enter_adhoc(&self.shutdown).await;
        self.spawn_unit(&project, &lane, request).await?;
        drop(adhoc);
        let mut st = lane.st();
        let summary = match st.units.get_mut(&args.name) {
            Some(rec) => {
                rec.left_out = left_out.clone();
                unit_summary(&reference, &args.name, rec)
            }
            None => UnitSummary::default(),
        };
        drop(st);
        let mut result = serde_json::to_value(summary).map_err(|e| internal("encode", e))?;
        if !left_out.is_empty() {
            result["left_out"] = crate::resolve::left_out(&left_out);
        }
        Ok(result)
    }

    /// `run`: one task, streamed as `log` events unless detached.
    pub(crate) async fn run_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: RunArgs = required(value)?;
        let run = args.run;
        let (project, lane) = self.lane(context, Policy::of(Method::Run)).await?;
        self.fresh_config(&project, &lane).await;
        let reference = lane.st().reference(&project.name());
        if let Some(env) = carries_env(context) {
            lane.st().env = Some(env.clone());
            project.st().last_env = Some(env);
        }
        self.ensure_infra_env(&project, &lane).await;
        // `--env` applies to this run only; the lane keeps its own overrides.
        let resolved = self
            .resolve_task(&project, &lane, &run.task, &run.env)
            .await?;
        if let Some((phase, entry)) = infra_entry(&run.task) {
            let entry = InfraEntry {
                phase,
                entry: entry.to_owned(),
                detach: run.detach,
                timeout_ms: args.timeout_ms,
            };
            return self
                .run_infra_entry(project, lane, resolved, entry, context, session)
                .await;
        }
        if let Some(error) = resolved.errors.get(&format!("tasks.{}", run.task)) {
            return Err(crate::resolve::resolve_error(error, &reference));
        }
        let task = resolved.tasks.get(&run.task).ok_or_else(|| {
            RpcError::new(
                "not_found",
                json!({"reference": run.task, "candidates": resolved.tasks.keys().collect::<Vec<_>>()}),
                3,
            )
        })?;
        if let Some(prompt) = &task.confirm
            && !context.yes
        {
            return Err(confirmation(
                json!({"task": run.task, "prompt": prompt, "flag": "--yes"}),
            ));
        }
        let unit = task.launch.as_ref().map(|_| run.task.clone());
        if task.interactive && !run.terminal {
            return Err(RpcError::new(
                "terminal_required",
                json!({"task": run.task}),
                2,
            ));
        }
        let background = task.interactive || run.detach;
        if background {
            // A previous run of the task stays listed until the new one replaces it.
            let previous = unit
                .as_ref()
                .and_then(|unit| lane.st().units.get(unit).map(|rec| rec.host.clone()));
            let engine = self.clone();
            let actor = context.actor.clone();
            let name = run.task.clone();
            let (task_project, task_lane) = (project.clone(), lane.clone());
            let resolved = resolved.clone();
            // Ends with the daemon or a clean or reset of the lane; `stop` of the
            // task's unit ends it otherwise.
            let (adhoc, cancel) = lane.enter_adhoc(&self.shutdown).await;
            // The run's error, or `None` once it ended without one.
            let (ended, mut outcome) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let _adhoc = adhoc;
                let result = engine
                    .run_task(
                        &task_project,
                        &task_lane,
                        &resolved,
                        &name,
                        "manual",
                        &actor,
                        None,
                        &cancel,
                        0,
                    )
                    .await;
                let _ = ended.send(result.err());
            });
            if let Some(unit) = &unit {
                await_spawn(&lane, unit, previous.as_ref(), &mut outcome).await?;
            }
            return Ok(json!({
                "task": run.task,
                "lane": reference,
                "unit": unit,
                "interactive": task.interactive,
                "detached": run.detach && !task.interactive,
            }));
        }
        // `run_task` itself stops the unit, finishes the row and emits the closing
        // event before it returns.
        let (task_project, task_lane, task_resolved) = (&project, &lane, &resolved);
        let (name, actor) = (&run.task, &context.actor);
        let outcome = self
            .run_limited(&lane, args.timeout_ms, &run.task, |cancel| async move {
                self.run_task(
                    task_project,
                    task_lane,
                    task_resolved,
                    name,
                    "manual",
                    actor,
                    Some(session),
                    &cancel,
                    0,
                )
                .await
            })
            .await?;
        Ok(run_result(&run.task, &reference, unit.as_deref(), &outcome))
    }

    /// `run infra.setup.<entry>` or `run infra.teardown.<entry>`: one entry, as a
    /// task step would run it, done-record included. Not a task, so no
    /// `task_runs` row and no `task` events.
    async fn run_infra_entry(
        self: &Arc<Self>,
        project: Arc<ProjectRt>,
        lane: Arc<LaneRt>,
        resolved: Arc<ResolvedLane>,
        entry: InfraEntry,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let task = format!("infra.{}.{}", entry.phase, entry.entry);
        let reference = lane.st().reference(&project.name());
        if let Some(error) = resolved.errors.get(&task) {
            return Err(crate::resolve::resolve_error(error, &reference));
        }
        let commands = if entry.phase == "setup" {
            &resolved.setup
        } else {
            &resolved.teardown
        };
        if !commands.contains_key(&entry.entry) {
            // An unknown entry plans the whole lane, so these are all of them.
            let candidates: Vec<String> = resolved
                .setup
                .keys()
                .map(|name| format!("infra.setup.{name}"))
                .chain(
                    resolved
                        .teardown
                        .keys()
                        .map(|name| format!("infra.teardown.{name}")),
                )
                .collect();
            return Err(RpcError::new(
                "not_found",
                json!({"reference": task, "candidates": candidates}),
                3,
            ));
        }
        let unit = format!("{}.{}", entry.phase, entry.entry);
        if entry.detach {
            let previous = lane.st().units.get(&unit).map(|rec| rec.host.clone());
            let engine = self.clone();
            let actor = context.actor.clone();
            let (step_project, step_lane) = (project.clone(), lane.clone());
            // Ends with the daemon or a clean or reset of the lane; `stop` of the
            // entry's unit ends it otherwise.
            let (adhoc, cancel) = lane.enter_adhoc(&self.shutdown).await;
            let (ended, mut outcome) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let _adhoc = adhoc;
                let result = engine
                    .step_command(
                        &step_project,
                        &step_lane,
                        &resolved,
                        entry.phase,
                        &entry.entry,
                        &actor,
                        &cancel,
                        None,
                    )
                    .await;
                let _ = ended.send(result.err());
            });
            await_spawn(&lane, &unit, previous.as_ref(), &mut outcome).await?;
            return Ok(json!({
                "task": task,
                "lane": reference,
                "unit": unit,
                "interactive": false,
                "detached": true,
            }));
        }
        let (step_project, step_lane, step_resolved) = (&project, &lane, &resolved);
        let (phase, name, actor) = (entry.phase, &entry.entry, &context.actor);
        let outcome = self
            .run_limited(&lane, entry.timeout_ms, &task, |cancel| async move {
                self.step_command(
                    step_project,
                    step_lane,
                    step_resolved,
                    phase,
                    name,
                    actor,
                    &cancel,
                    Some(session),
                )
                .await
            })
            .await?;
        Ok(run_result(&task, &reference, Some(&unit), &outcome))
    }

    /// Runs `work` inside the lane's ad-hoc gate with the request's time limit,
    /// which fires the token `work` gets: the run stops its unit before it
    /// returns. A clean or reset of the lane fires the token too, then waits for
    /// the gate.
    async fn run_limited<F, Fut>(
        &self,
        lane: &LaneRt,
        timeout_ms: Option<u64>,
        task: &str,
        work: F,
    ) -> Result<TaskOutcome, RpcError>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<TaskOutcome, RpcError>>,
    {
        let (adhoc, cancel) = lane.enter_adhoc(&self.shutdown).await;
        let expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let timer = timeout_ms.map(|ms| {
            let (cancel, expired) = (cancel.clone(), expired.clone());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                expired.store(true, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            })
        });
        let outcome = work(cancel).await;
        drop(adhoc);
        if let Some(timer) = timer {
            timer.abort();
        }
        // A clean, reset or shutdown that fired the token is no timeout, and a task
        // that succeeded just as the limit fired still succeeded.
        let succeeded = matches!(
            &outcome,
            Ok(outcome) if matches!(outcome.state, crate::units::UnitState::Exited(_))
        );
        let expired = expired.load(std::sync::atomic::Ordering::SeqCst);
        if expired && !self.shutdown.is_cancelled() && !succeeded {
            return Err(RpcError::new("timeout", json!({"task": task}), 124));
        }
        outcome
    }

    /// `profile.list`: the profiles of the lane's config, for pickers. The lane's
    /// current choice is `LaneSummary.profiles`.
    pub(crate) async fn profile_list(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let (project, lane) = match self
            .lane_or_file(context, Policy::of(Method::ProfileList))
            .await?
        {
            LaneOrFile::Lane(pair) => pair,
            LaneOrFile::File { config, .. } => {
                let items = devlanes_resolve::profile_summaries(&config);
                return Ok(json!({"lane": null, "items": items}));
            }
        };
        let st = lane.st();
        let reference = st.reference(&project.name());
        let config = match &st.config {
            crate::model::ConfigState::Loaded(config) => config,
            crate::model::ConfigState::Invalid(error) => return Err(error.clone()),
            crate::model::ConfigState::Missing => {
                return Err(RpcError::new("no_config", json!({"lane": reference}), 1));
            }
            crate::model::ConfigState::Elsewhere(other) => {
                return Err(crate::model::elsewhere_error(other, Some(&reference)));
            }
        };
        let items = devlanes_resolve::profile_summaries(config);
        Ok(json!({"lane": reference, "items": items}))
    }

    /// `task.list`: tasks plus the `run_script` files the config references.
    pub(crate) async fn task_list(self: &Arc<Self>, context: &Context) -> Result<Value, RpcError> {
        let (config, path, lane_id, reference) = match self
            .lane_or_file(context, Policy::of(Method::TaskList))
            .await?
        {
            LaneOrFile::Lane((project, lane)) => {
                let (config_dir, _) = project.config_path();
                let st = lane.st();
                let config = st
                    .config
                    .loaded()
                    .cloned()
                    .ok_or_else(|| RpcError::new("no_config", json!({"path": st.path}), 1))?;
                let lane_id = st.record.as_ref().map(|record| record.id);
                let reference = st.reference(&project.name());
                (config, st.path.join(config_dir), lane_id, Some(reference))
            }
            // A project nobody started has run nothing.
            LaneOrFile::File { file, config } => {
                let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
                (config, dir, None, None)
            }
        };
        let runs = match lane_id {
            Some(id) => self.store.last_task_runs(id).await.map_err(store_error)?,
            None => Vec::new(),
        };
        let mut items = Vec::new();
        let mut sources: BTreeSet<String> = BTreeSet::new();
        for (name, task) in &config.tasks {
            if let Some(script) = &task.run_script
                && let Ok((file, _)) = script.file_and_name()
            {
                sources.insert(file.to_owned());
            }
            // Never-run tasks get `null`, so clients can say so.
            // A finished run keeps the word its end got; one without is still going.
            let last_run = runs.iter().find(|run| &run.task == name).map(|run| {
                let state = run
                    .state
                    .as_deref()
                    .unwrap_or(crate::units::UnitState::Running.word());
                json!({
                    "state": state,
                    "trigger": run.trigger,
                    "started_at_ms": run.started_at,
                    "finished_at_ms": run.ended_at,
                    "exit_code": run.exit_code,
                })
            });
            items.push(json!({
                "last_run": last_run,
                "name": name,
                "when": task.when,
                "after": task.after,
                "profiles": task.profiles,
                "steps": task.steps,
                "confirm": task.confirm.is_some(),
                "interactive": matches!(task.interactive, Some(devlanes_config::Templated::Literal(true))),
                "run_script": task.run_script.as_ref().and_then(|s| s.file_and_name().ok()).map(|(file, name)| json!({"file": file, "name": name})),
            }));
        }
        for service in config.services.values() {
            if let Some(script) = &service.run_script
                && let Ok((file, _)) = script.file_and_name()
            {
                sources.insert(file.to_owned());
            }
        }
        let sources: Vec<Value> = sources
            .into_iter()
            .map(|file| {
                let runner = self
                    .integ
                    .script_runners
                    .iter()
                    .find(|runner| runner.matches(&path.join(&file)))
                    .map(|runner| runner.id());
                json!({"file": file, "runner": runner})
            })
            .collect();
        Ok(
            json!({"lane": reference, "items": items, "sources": sources, "truncated": false, "next_cursor": null}),
        )
    }

    /// `daemon.reset-state` (requires `yes`): stops everything and empties state.db.
    pub(crate) async fn reset_state(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        if !context.yes {
            return Err(confirmation(
                json!({"processes": self.all_lanes().iter().map(|(_, l)| l.st().units.values().filter(|u| u.host.is_alive()).count()).sum::<usize>(), "flag": "--yes"}),
            ));
        }
        // Every lane stays locked until the reset is done, so no start spawns a
        // unit after the stop and before the records it would need are gone.
        let lanes = self.all_lanes();
        let _ops = self.lock_lanes(&lanes).await;
        self.wake_all().await;
        // `exec` and `run` take no `op`: interrupt runs and wait for spawns in
        // flight, so the stop below sees every unit.
        let mut closed = Vec::with_capacity(lanes.len());
        for (_, lane) in &lanes {
            closed.push(lane.close_adhoc().await);
        }
        let stopped = Self::stop_lanes_locked(&lanes, None).await.len();
        // The reset would erase the only record of orphans (of known and unknown
        // lanes) and their escaped descendants, so they are terminated first.
        let terminated = self.end_recorded_runs().await?;
        for project in self.all_projects() {
            project.watch.cancel();
        }
        // A refused reset leaves the loaded projects in place.
        self.store.reset().await.map_err(store_error)?;
        // The pairings went with the store: no client keeps its session, and
        // no code handed out before stays valid.
        self.forget_pairings();
        // A start waiting for one of these lanes finds it unassigned once it runs.
        for (_, lane) in &lanes {
            let mut st = lane.st();
            st.record = None;
            st.units.clear();
            st.orphans.clear();
        }
        lock(&self.projects).clear();
        // The store lost them too, and the routes below must not follow them.
        self.forget_all_choices();
        self.publish_routes();
        self.bus
            .emit(events::RESYNC, json!({"reason": "reset_state"}));
        Ok(json!({"stopped": stopped, "terminated": terminated}))
    }

    /// Fills the lane's Compose env (`COMPOSE_PROJECT_NAME` and the rest) when
    /// no start or `infra up` did since the daemon began: `run` reaches infra
    /// entries on such a lane too, after a daemon restart for one. Starts
    /// nothing; a lane without infra, or Docker being down, leaves it empty.
    async fn ensure_infra_env(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        if !lane.st().setup_env.is_empty() {
            return;
        }
        let Ok(spec) = self.infra_spec(project, lane, &[]).await else {
            return;
        };
        if spec.is_empty() {
            return;
        }
        let needs = self.integ.infra.port_needs(&spec).await.unwrap_or_default();
        let spec = match self.infra_spec(project, lane, &needs).await {
            Ok(with_ports) => with_ports,
            Err(_) => spec,
        };
        match self.integ.infra.namespace(&spec).await {
            Ok(up) => {
                // Only the env: the namespace and containers make status list
                // the lane's infra as up, which nothing here checked.
                let mut st = lane.st();
                // A start that finished meanwhile knows better.
                if st.setup_env.is_empty() {
                    st.setup_env = up.setup_env;
                }
            }
            Err(error) => {
                tracing::debug!(code = %error.code, "infra env for a run unavailable");
            }
        }
    }

    /// Spec with ports for the request's lane, allocating infra ports first.
    async fn infra_for(
        self: &Arc<Self>,
        context: &Context,
        method: Method,
    ) -> Result<(LanePair, crate::integrations::InfraSpec), RpcError> {
        let (project, lane) = self.lane(context, Policy::of(method)).await?;
        let spec = self.infra_spec(&project, &lane, &[]).await?;
        let needs = self
            .integ
            .infra
            .port_needs(&spec)
            .await
            .map_err(RpcError::from)?;
        self.allocate_lane(&project, &lane, &needs).await?;
        let spec = self.infra_spec(&project, &lane, &needs).await?;
        Ok(((project, lane), spec))
    }

    /// `infra.up` for the request's lane (shared plus per-lane containers).
    pub(crate) async fn infra_up_method(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let ((project, lane), spec) = self.infra_for(context, Method::InfraUp).await?;
        let _op = lane.op.write().await;
        // Compose and Docker refuse to start paused containers.
        self.wake_lane(&project, &lane, Some(crate::idle::Activity::Command))
            .await;
        // The containers bind the lane's ports, so box lanes it clashes with
        // give them up first. Its own containers keep it counted once up.
        let claim = self.claim_ports(&lane).await;
        let up = match self.infra_up(&project, &spec).await {
            Ok(up) => up,
            Err(error) => {
                // Before the claim drops, so what runs anyway keeps the lane.
                self.store_after_failed_up(&lane, &spec).await;
                return Err(RpcError::from(error));
            }
        };
        {
            let mut st = lane.st();
            st.infra_ns = Some(up.namespace.clone());
            st.infra_replaced();
            st.store_containers(up.containers.clone());
            st.setup_env = up.setup_env.clone();
        }
        drop(claim);
        self.refresh_views_held(&project, &lane).await;
        Ok(
            json!({"lane": lane.st().reference(&project.name()), "infra": up.report, "endpoints": up.namespace}),
        )
    }

    /// `infra.down`: stops shared containers (data kept). Refused while lanes run
    /// unless confirmed, because shared infra serves every lane of the project.
    pub(crate) async fn infra_down_method(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let (project, lane) = self.lane(context, Policy::of(Method::InfraDown)).await?;
        let env = lane.st().env.clone();
        let spec = self.shared_infra_spec(&project, env.as_deref()).await?;
        let running: Vec<String> = project
            .lanes()
            .iter()
            .filter(|lane| lane.st().any_running())
            .map(|lane| lane.st().reference(&project.name()))
            .collect();
        if !running.is_empty() && !context.yes {
            return Err(confirmation(
                json!({"lanes_running": running, "flag": "--yes"}),
            ));
        }
        self.infra_down(&project, &spec)
            .await
            .map_err(RpcError::from)
    }

    /// `infra.status` for the request's lane.
    pub(crate) async fn infra_status_method(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let ((project, lane), spec) = self.infra_for(context, Method::InfraStatus).await?;
        let status = self
            .integ
            .infra
            .status(&spec)
            .await
            .map_err(RpcError::from)?;
        Ok(json!({"lane": lane.st().reference(&project.name()), "infra": status}))
    }

    /// `config.check` on the request's project file (`--file`, the target lane's,
    /// else the nearest `devlanes.yaml`), with preset option validation. Hostname
    /// advice comes from the lane's layout, or from a provisional lane 0 when the
    /// lane has none yet; the reply names that lane under `provisional`, and lists
    /// `hostnames` under `skipped` when neither can be laid out.
    pub(crate) async fn config_check(
        self: &Arc<Self>,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let target = self
            .lane_or_file(context, Policy::of(Method::ConfigCheck))
            .await?;
        // The lane's warnings below come from its loaded config; bring it up to date.
        // A worktree without its own devlanes.local.yaml uses the primary
        // worktree's, as the lane does; for a project nobody started, the primary
        // worktree git reports.
        let (file, primary) = match &target {
            LaneOrFile::Lane((project, lane)) => {
                self.fresh_config(project, lane).await;
                let file = context.file.clone().unwrap_or_else(|| {
                    let (dir, name) = project.config_path();
                    lane.st().path.join(dir).join(name)
                });
                let branch = project.st().primary_branch.clone();
                let primary = PrimaryWorktree {
                    root: self.primary_path(project),
                    branch: Some(branch),
                    relative: project.config_path().0,
                };
                (file, primary)
            }
            LaneOrFile::File { file, config } => {
                let primary = PrimaryWorktree::of(file, config, self.opts.ci).await?;
                (file.clone(), primary)
            }
        };
        let primary_dir = primary.root.join(&primary.relative);
        let checks = self.config_checks();
        let known = crate::platforms::known(&self.integ.contributors);
        let path = file.clone();
        let (config, report, unknown) = tokio::task::spawn_blocking(move || {
            let config =
                devlanes_config::load_lane_project_with::<OpaqueExtensions>(&path, &primary_dir)?;
            let report = crate::registry::check_config(&config, &checks)?;
            let unknown = crate::platforms::unknown(&config, &known);
            Ok::<_, devlanes_config::ConfigError>((config, report, unknown))
        })
        .await
        .map_err(|error| internal("config_check", error))?
        .map_err(|error| crate::model::config_error(&error))?;
        let mut warnings: Vec<Value> = match serde_json::to_value(&report.warnings) {
            Ok(Value::Array(list)) => list,
            Ok(_) => Vec::new(),
            Err(error) => return Err(internal("encode", error)),
        };
        warnings.extend(unknown);
        let mut result = json!({"project": report.project, "path": file});
        let laid_out = match &target {
            LaneOrFile::Lane((project, lane)) if lane.st().view.is_some() => {
                let st = lane.st();
                let reference = st.reference(&project.name());
                warnings.extend(self.host_warnings(&reference, st.view.as_deref()));
                true
            }
            // Hostnames depend on the lane's index, domain and vars, not on its
            // ports, so lane 0 laid out from the file gives the same advice before
            // anything started. `host_conflict` needs other lanes and is left out.
            _ => {
                let facts = self.provisional_facts(
                    &config,
                    &primary.root,
                    primary.branch.as_deref(),
                    &primary.relative,
                );
                let reference = format!("{}@0", config.project);
                match self.provisional_view(&config, &facts).await {
                    Some(view) => {
                        warnings.extend(self.host_warnings(&reference, Some(&view)));
                        result["provisional"] = json!(reference);
                        true
                    }
                    None => false,
                }
            }
        };
        if !laid_out {
            result["skipped"] = json!(["hostnames"]);
        }
        if let LaneOrFile::Lane((project, lane)) = &target {
            let st = lane.st();
            warnings.extend(self.config_check_warnings(&st.reference(&project.name()), &st));
        }
        result["warnings"] = json!(warnings);
        Ok(result)
    }

    /// `host_outside_ca` warnings of a lane's layout `view`, shaped for `config check`
    /// with the config path that names each host.
    fn host_warnings(&self, reference: &str, view: Option<&LaneView>) -> Vec<Value> {
        self.ca_warnings(reference, view)
            .into_iter()
            .map(|warning| {
                let data = &warning.data;
                let path = view
                    .and_then(|view| {
                        let service = view
                            .routes
                            .iter()
                            .find(|route| route.host == data["host"])
                            .map(|route| format!("services.{}.hosts", route.service));
                        service.or_else(|| {
                            view.externals
                                .iter()
                                .find(|external| {
                                    external.routes.iter().any(|r| r.host == data["host"])
                                })
                                .map(|external| format!("infra.external.{}.hosts", external.name))
                        })
                    })
                    .unwrap_or_else(|| "infra.hosts".into());
                json!({
                    "path": path,
                    "message": format!(
                        "{} is outside the names the devlanes CA can certify, so HTTPS fails there (plain HTTP works); use a name like {}",
                        data["host"].as_str().unwrap_or_default(),
                        data["suggestion"].as_str().unwrap_or_default(),
                    ),
                    "code": warning.code,
                    "host": data["host"],
                    "suggestion": data["suggestion"],
                })
            })
            .collect()
    }
}
