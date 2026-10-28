//! Lane lifecycle: unit spawning, readiness, tasks and setup commands, the start
//! pipeline of ordered [`LanePhase`]s (decision 5), stop, restart and clean.
//! Nothing here stops a process implicitly (decision 16) or restarts one (decision 6).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use devlanes_config::{TaskWhen, Templated};
use devlanes_core::{
    ExitOutcome, LaunchSpec, StopPolicy, TerminalSize, Unit, contrib::PreparedFile,
    env::Environment,
};
use devlanes_proto::{
    Actor, Leftovers, LogQuery, RpcError, StartResult, StoppingStep, UnitFailureData, UnitSummary,
    events,
};
use devlanes_resolve::{Launch, ReadySpec, ResolvedCommand, ResolvedLane, ResolvedService};
use devlanes_state::{DesiredState, TaskRunInput};
use futures_util::{
    StreamExt,
    future::{BoxFuture, join_all},
    stream::FuturesUnordered,
};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    Session,
    engine::{Inner, with_ownership},
    integrations::{InfraPortNeed, InfraSpec, ManagedUnit, PlannedUnit},
    model::{
        EnvMap, Kind, LaneRt, ProjectRt, StartCtl, UnitRec, core_actor, is_mine, unit_summary,
    },
    ready::Readiness,
    registry::{internal, store_error},
    stopping::StopSteps,
    units::{UnitHost, UnitInfo, UnitState, now_ms},
};

/// Log lines attached to every failure (cli-design 3.4).
pub(crate) const TAIL: usize = 20;
/// How long a restart waits for the old process's ports to close.
const PORT_RELEASE: Duration = Duration::from_secs(15);
/// How often a single-unit stop waiting for `op` looks for a start that took it.
const START_POLL: Duration = Duration::from_millis(25);
/// Nested `steps` deeper than this are treated as a cycle.
const MAX_STEP_DEPTH: usize = 16;
/// A unit stop longer than this is logged, to explain a slow daemon stop.
const SLOW_STOP: Duration = Duration::from_secs(2);

/// Whether the lane's `lane.stop` and `lane.clean` tasks can run: a lane whose
/// worktree is gone (or that never got an env or record) has no directory or
/// environment to run them in.
fn hooks_runnable(st: &crate::model::LaneSt) -> bool {
    st.present && st.env.is_some() && st.record.is_some()
}

/// Everything needed to spawn one unit.
pub(crate) struct SpawnRequest {
    pub name: String,
    pub kind: Kind,
    pub spec: LaunchSpec,
    pub env: Arc<Environment>,
    pub files: Vec<PreparedFile>,
    pub actor: Actor,
    pub ports: BTreeMap<String, u16>,
    pub urls: Vec<String>,
    pub tags: Vec<String>,
    pub hidden: bool,
    pub autostart: bool,
    pub stop: StopPolicy,
    pub literal_port: Option<u16>,
    pub preset: Option<String>,
    /// Palette name; `None` derives one from the name at spawn.
    pub color: Option<String>,
    /// Folders mirrored while the unit runs (services only).
    pub sync: Vec<devlanes_resolve::SyncSpec>,
    /// `services.<name>.name` (services only).
    pub display_name: Option<String>,
    /// The readiness check word (services only).
    pub ready_check: Option<&'static str>,
    /// `UnitSummary.role`: `service`, `task`, `setup`, `teardown` or `exec`.
    pub role: &'static str,
    /// `services.<name>.keep_awake`: the lane never goes idle while this runs.
    pub keep_awake: bool,
    /// A service run's [`crate::drift::Fingerprint`]; `None` for one-shot units.
    pub fingerprint: Option<crate::drift::Fingerprint>,
}

/// Copies an environment with its provenance (the type is deliberately not `Clone`).
pub(crate) fn copy_env(env: &Environment) -> Environment {
    Environment {
        values: env.values.clone(),
        sources: env.sources.clone(),
    }
}

impl SpawnRequest {
    /// Request for one service replica.
    pub(crate) fn service(service: &ResolvedService, actor: &Actor) -> Self {
        let mut ports = service.ports.clone();
        ports.insert("default".into(), service.port);
        let mut urls = vec![service.url.clone()];
        urls.extend(service.public_url.clone());
        Self {
            name: service.unit.clone(),
            kind: Kind::Service,
            spec: service.launch.to_spec(service.pty, TerminalSize::default()),
            env: Arc::new(copy_env(&service.launch.env)),
            files: service.launch.files.clone(),
            actor: actor.clone(),
            ports,
            urls,
            tags: service.tags.clone(),
            hidden: service.hidden,
            autostart: service.autostart,
            stop: service.stop,
            literal_port: service.literal_port.then_some(service.port),
            preset: service.preset.clone(),
            color: Some(service.color.clone()),
            sync: service.sync.clone(),
            display_name: service.display_name.clone(),
            ready_check: Some(ready_word(&service.ready.check)),
            role: "service",
            keep_awake: service.keep_awake,
            fingerprint: Some(crate::drift::Fingerprint::of(service)),
        }
    }

    /// Request for a one-shot unit (task or setup/teardown entry).
    pub(crate) fn oneshot(
        name: &str,
        kind: Kind,
        launch: &Launch,
        pty: bool,
        extra: &EnvMap,
        actor: &Actor,
    ) -> Self {
        let mut spec = launch.to_spec(pty, TerminalSize::default());
        spec.env
            .extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        Self {
            name: name.to_owned(),
            kind,
            spec,
            env: Arc::new(copy_env(&launch.env)),
            files: launch.files.clone(),
            actor: actor.clone(),
            ports: BTreeMap::new(),
            urls: Vec::new(),
            tags: Vec::new(),
            hidden: false,
            autostart: false,
            stop: StopPolicy::default(),
            literal_port: None,
            preset: None,
            color: None,
            sync: Vec::new(),
            display_name: None,
            ready_check: None,
            role: kind.word(),
            keep_awake: false,
            fingerprint: None,
        }
    }
}

/// `UnitSummary.ready_check` for a resolved check.
fn ready_word(spec: &ReadySpec) -> &'static str {
    match spec {
        ReadySpec::Started => "started",
        ReadySpec::Tcp { .. } => "tcp",
        ReadySpec::Http { .. } => "http",
        ReadySpec::Log { .. } => "log",
        ReadySpec::Cmd { .. } => "cmd",
    }
}

/// What [`Inner::clean_lane`] does with the lane's logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Logs {
    /// `lane clean` names the lane: its logs go with it.
    Delete,
    /// A lane cleaned because its worktree is gone (`lane gc`, the removed
    /// worktree policy, the sweep after a re-list) keeps them, so what it
    /// last did can still be read.
    Keep,
}

/// `<logs>/<project>/<lane index>/<unit>`, where a unit's log segments live.
pub(crate) fn unit_log_dir(
    logs: &std::path::Path,
    project: &str,
    index: u32,
    unit: &str,
) -> std::path::PathBuf {
    lane_log_dir(logs, project, index).join(unit_folder(unit))
}

/// `<logs>/<project>/<lane index>`, the logs of every unit of one lane.
pub(crate) fn lane_log_dir(
    logs: &std::path::Path,
    project: &str,
    index: u32,
) -> std::path::PathBuf {
    logs.join(devlanes_core::paths::name_on_disk(project).as_ref())
        .join(index.to_string())
}

/// The folder name of a unit's logs: `/` becomes `_`, as it always has on Unix.
#[cfg(unix)]
fn unit_folder(unit: &str) -> String {
    unit.replace('/', "_")
}

/// The folder name of a unit's logs: the injective Windows encoding, so
/// `db:migrate` or `aux` get a folder and `a/b` never meets `a_b`.
#[cfg(windows)]
fn unit_folder(unit: &str) -> String {
    devlanes_core::paths::name_on_disk(unit).into_owned()
}

/// Writes contributor files atomically. Paths must stay inside one of `roots` (the lane
/// worktree, which relative paths are joined to, then the lane data folder); the files
/// must not contain secrets because they land on disk.
async fn write_files(
    roots: &[std::path::PathBuf],
    files: Vec<PreparedFile>,
) -> Result<(), RpcError> {
    if files.is_empty() {
        return Ok(());
    }
    let roots = roots.to_vec();
    tokio::task::spawn_blocking(move || {
        for file in files {
            let path = match roots.first() {
                Some(worktree) if !file.path.is_absolute() => worktree.join(&file.path),
                _ => file.path.clone(),
            };
            if !roots
                .iter()
                .any(|root| devlanes_core::paths::path_starts_with(&path, root))
                || path
                    .components()
                    .any(|c| c == std::path::Component::ParentDir)
            {
                return Err(RpcError::new(
                    "prepared_file_outside_lane",
                    json!({"path": path}),
                    1,
                ));
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| internal("io", e))?;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let temp = path.with_file_name(format!(".{name}.devlanes-tmp"));
            std::fs::write(&temp, &file.contents).map_err(|e| internal("io", e))?;
            crate::files::rename(&temp, &path).map_err(|e| internal("io", e))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| internal("io", e))?
}

/// Task keys that decide when or how a task is shown or waited for, not what it runs:
/// editing them must not run a once-only task again.
const TASK_SCHEDULING: &[&str] = &[
    "when",
    "after",
    "profiles",
    "confirm",
    "interactive",
    "timeout",
    "pty",
];

/// Setup-entry keys that decide how a failure is treated, not what runs.
const SETUP_HANDLING: &[&str] = &["timeout", "ignore_errors"];

/// FNV-1a over the canonical JSON of a config declaration, such as one `infra.setup`
/// entry or task as written, so an edited entry counts as not done. The declaration,
/// not the evaluated command, keeps environment values from `.env` and shells out of
/// state.db. Unset and empty values are dropped first, so a field a later release
/// adds does not change every hash; FNV because std's hasher is not stable across
/// releases.
pub(crate) fn declaration_hash<T: serde::Serialize>(
    declaration: Option<&T>,
    ignored: &[&str],
) -> Option<String> {
    let mut value = serde_json::to_value(declaration?).ok()?;
    if let Value::Object(map) = &mut value {
        map.retain(|key, _| !ignored.contains(&key.as_str()));
    }
    let text = canonical_json(value).map_or_else(String::new, |value| value.to_string());
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(format!("{hash:016x}"))
}

/// `value` without nulls and empty lists or mappings, at every level; `None` when
/// nothing is left.
fn canonical_json(value: Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::Object(map) => {
            let map: serde_json::Map<String, Value> = map
                .into_iter()
                .filter_map(|(key, value)| canonical_json(value).map(|value| (key, value)))
                .collect();
            (!map.is_empty()).then_some(Value::Object(map))
        }
        Value::Array(items) => {
            let items: Vec<Value> = items.into_iter().filter_map(canonical_json).collect();
            (!items.is_empty()).then_some(Value::Array(items))
        }
        other => Some(other),
    }
}

/// Whether a completion recorded with `recorded` still covers a declaration hashing to
/// `current`. Records from before hashing match, so an upgrade reruns nothing.
pub(crate) fn same_declaration(recorded: Option<&str>, current: Option<&str>) -> bool {
    recorded.is_none() || recorded == current
}

/// The [`declaration_hash`] of task `name` in `config`.
pub(crate) fn task_hash(config: &devlanes_config::ProjectConfig, name: &str) -> Option<String> {
    declaration_hash(config.tasks.get(name), TASK_SCHEDULING)
}

/// The [`declaration_hash`] of `infra.setup.<name>` in `config`.
pub(crate) fn setup_hash(config: &devlanes_config::ProjectConfig, name: &str) -> Option<String> {
    declaration_hash(config.infra.setup.get(name), SETUP_HANDLING)
}

/// The lane's loaded config, read when a run begins so its completion records the
/// declaration it ran, not one edited while it ran.
fn loaded_config(lane: &LaneRt) -> Option<Arc<devlanes_config::ProjectConfig>> {
    lane.st().config.loaded().cloned()
}

/// Maps a resolved readiness spec to the checker's form. `env` is the unit's launch
/// environment, which a `cmd` probe runs with.
pub(crate) fn readiness(spec: &ReadySpec, env: BTreeMap<String, String>) -> Readiness {
    match spec {
        ReadySpec::Started => Readiness::Started,
        ReadySpec::Tcp { port } => Readiness::Tcp { port: *port },
        ReadySpec::Http {
            port,
            path,
            status,
            host,
        } => Readiness::Http {
            port: *port,
            path: path.clone(),
            status: *status,
            host: host.clone(),
        },
        ReadySpec::Log { pattern } => Readiness::Log {
            pattern: pattern.clone(),
        },
        ReadySpec::Cmd { shell, cwd } => Readiness::Cmd {
            command: shell.clone(),
            cwd: cwd.clone(),
            env,
            // Set with `Readiness::with_shell` from the daemon's integrations.
            #[cfg(windows)]
            shell: None,
        },
    }
}

/// Whether a spawned leader's identity can be recorded as a run: a PID and
/// the session it leads.
#[cfg(unix)]
fn recordable(process: &devlanes_core::ProcessIdentity) -> bool {
    process.pid > 0 && process.sid > 0
}

/// Whether a spawned leader's identity can be recorded as a run: a PID;
/// Windows has no sessions.
#[cfg(windows)]
fn recordable(process: &devlanes_core::ProcessIdentity) -> bool {
    process.pid > 0
}

/// True once the state no longer moves toward ready on its own.
fn settled(state: &UnitState) -> bool {
    matches!(
        state,
        UnitState::Ready | UnitState::Failed(_) | UnitState::Exited(_)
    )
}

/// Waits until every host is ready, failed or exited; false on deadline.
pub(crate) async fn wait_settled(hosts: &[UnitHost], deadline: Instant) -> bool {
    for host in hosts {
        let mut watch = host.watch_state();
        loop {
            if settled(&watch.borrow_and_update()) {
                break;
            }
            match tokio::time::timeout_at(deadline, watch.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => return false,
            }
        }
    }
    true
}

/// Failure data for a unit: exit code, reason and last log lines.
pub(crate) async fn failure_of(lane: &str, name: &str, host: &UnitHost) -> UnitFailureData {
    let reason = match host.state() {
        UnitState::Failed(reason) => reason,
        _ => None,
    };
    let (log_tail, log_tail_styled) =
        devlanes_proto::split_styled(host.tail_lines(TAIL, true).await);
    UnitFailureData {
        lane: lane.to_owned(),
        unit: name.to_owned(),
        exit_code: host.exit_outcome().and_then(|o| o.code),
        log_tail,
        reason,
        log_tail_styled,
    }
}

/// What the ended run behind `host` left behind; `None` when nothing was or a
/// newer run of `unit` replaced it.
pub(crate) fn ended_leftovers(lane: &LaneRt, unit: &str, host: &UnitHost) -> Option<Leftovers> {
    let st = lane.st();
    let rec = st.units.get(unit).filter(|rec| rec.host.same_run(host))?;
    Some(rec.leftovers(unit)).filter(|leftovers| !leftovers.is_empty())
}

/// Outcome of one pipeline phase.
pub(crate) enum PhaseResult {
    Done,
    /// Not applicable; the reason travels in the `lane.phase` event.
    Skipped(RpcError),
    /// Failed with failures recorded in the result; the start returns normally.
    Halt(RpcError),
    /// Failed in a way that turns the start into an error response.
    Error(RpcError),
}

impl PhaseResult {
    /// The `state` and `reason` of the phase's closing `lane.phase` event.
    fn event_state(&self) -> (&'static str, Option<&RpcError>) {
        match self {
            Self::Done => ("done", None),
            Self::Skipped(why) => ("skipped", Some(why)),
            Self::Halt(why) | Self::Error(why) => ("failed", Some(why)),
        }
    }
}

/// Mutable context threaded through the start pipeline.
pub(crate) struct StartCx {
    pub project: Arc<ProjectRt>,
    pub lane: Arc<LaneRt>,
    pub reference: String,
    pub actor: Actor,
    pub services: Vec<String>,
    /// `Selection::autostart_pending`: entries of `services` that resolution drops
    /// when their `autostart` expression evaluates to false.
    pub autostart_pending: Vec<String>,
    /// Profiles in effect, in the order given.
    pub profiles: Vec<String>,
    /// False when the profiles turn infra off. Set from the selection before infra
    /// (literal values), then from [`ResolvedLane::infra_enabled`] once resolved.
    pub infra_enabled: bool,
    /// Some profile's `infra` is an expression, so only resolution can decide.
    pub infra_templated: bool,
    pub whole_lane: bool,
    pub setup_only: bool,
    pub session: Option<Session>,
    pub resolved: Option<Arc<ResolvedLane>>,
    pub deadline: Instant,
    pub started: Vec<String>,
    pub already_running: Vec<String>,
    pub failures: Vec<UnitFailureData>,
    pub skipped_tasks: Vec<String>,
    /// Tasks and setup commands of this start that left processes behind.
    pub leftovers: Vec<Leftovers>,
    pub infra_needs: Vec<InfraPortNeed>,
    pub timed_out: bool,
    /// Bypass the start gate (`Selection.all_at_once`).
    pub all_at_once: bool,
    /// On this start's `lane.phase` events and its result.
    pub start_id: u64,
    /// Fired by a stop, clean or reset of the lane (or daemon shutdown); phases
    /// end early and stop the task they run. `ctl.cancel`.
    pub cancel: CancellationToken,
    /// What single-unit stops and restarts did to this start: withdrawn units
    /// are skipped, not waited for and reported as stopped.
    pub ctl: Arc<StartCtl>,
    /// The config the services were selected from. Resolution uses it too, so a
    /// reload in the middle of the start cannot change what it plans.
    pub config: Arc<devlanes_config::ProjectConfig>,
    /// Place in the start gate's line, taken when the request arrived.
    pub arrival: Option<devlanes_core::gate::Arrival>,
    /// Saved profiles and services the config no longer has, reported as warnings.
    pub dropped: Dropped,
}

/// One ordered step of the lane pipeline (plan 5.5).
#[async_trait]
pub(crate) trait LanePhase: Send + Sync {
    /// Name in `lane.phase` events; CI log groups use it.
    fn name(&self) -> &'static str;
    /// Internal steps (resolution) emit no events.
    fn visible(&self) -> bool {
        true
    }
    /// Runs the phase.
    async fn run(&self, engine: &Arc<Inner>, cx: &mut StartCx) -> PhaseResult;
}

/// Brings shared and per-lane infra up (skipped by `infra: false`).
struct InfraPhase;
/// Plans every unit once infra endpoints are known.
struct ResolvePhase;
/// `infra.setup`: first time per lane, named entries in parallel.
struct SetupPhase;
/// `lane.create`/`lane.start` tasks before or after services, in file order.
struct TasksPhase {
    after: bool,
}
/// All selected services together.
struct ServicesPhase;

/// The start pipeline in decision-5 order.
fn start_pipeline() -> Vec<Box<dyn LanePhase>> {
    vec![
        Box::new(InfraPhase),
        Box::new(ResolvePhase),
        Box::new(SetupPhase),
        Box::new(TasksPhase { after: false }),
        Box::new(ServicesPhase),
        Box::new(TasksPhase { after: true }),
    ]
}

/// A reason value for skipped phases.
fn reason(code: &str, data: Value) -> RpcError {
    RpcError::new(code, data, 0)
}

/// Unit names running `task` can spawn: the task itself, its steps' tasks and
/// the setup and teardown entries they name.
fn task_units(resolved: &ResolvedLane, task: &str) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    let mut pending = vec![task.to_owned()];
    while let Some(name) = pending.pop() {
        // Also ends step cycles, which `run_task` refuses by depth.
        if units.contains(&name) {
            continue;
        }
        let steps = resolved.tasks.get(&name).map(|t| t.steps.as_slice());
        for step in steps.unwrap_or_default() {
            if let Some(entry) = step.strip_prefix("infra.setup.") {
                units.push(format!("setup.{entry}"));
            } else if let Some(entry) = step.strip_prefix("infra.teardown.") {
                units.push(format!("teardown.{entry}"));
            } else {
                pending.push(step.clone());
            }
        }
        units.push(name);
    }
    units
}

/// The reason on a unit a `stop` (`by`: `stop`) or `restart` took out of a start.
pub(crate) fn withdrawn_reason(lane: &str, unit: &str, by: &str) -> RpcError {
    RpcError::new(
        "cancelled_by_stop",
        json!({"lane": lane, "unit": unit, "by": by}),
        0,
    )
}

/// A stop, clean or reset of the lane (or daemon shutdown) interrupted the start.
fn start_cancelled(cx: &StartCx) -> RpcError {
    RpcError::new("start_cancelled", json!({"lane": cx.reference}), 1)
}

/// Why infra and setup were skipped for the lane's profiles.
fn profile_disables_infra(cx: &StartCx) -> RpcError {
    reason(
        "profile_disables_infra",
        json!({"profile": cx.profiles.first(), "profiles": cx.profiles}),
    )
}

/// Infra failures as RPC errors.
fn infra_error(error: crate::integrations::InfraError) -> RpcError {
    RpcError::from(error)
}

#[async_trait]
impl LanePhase for InfraPhase {
    fn name(&self) -> &'static str {
        "infra"
    }

    async fn run(&self, engine: &Arc<Inner>, cx: &mut StartCx) -> PhaseResult {
        use tracing::Instrument as _;
        // Adding units to a lane that is up needs nothing from Docker here: the
        // bring-up stored the containers' namespace, and the setup phase checks
        // its entries against the containers on its own.
        if !cx.whole_lane && !cx.setup_only && cx.lane.st().infra_ns.is_some() {
            return PhaseResult::Skipped(reason("lane_running", json!({})));
        }
        let empty_spec = match engine
            .infra_spec(&cx.project, &cx.lane, &[])
            .instrument(tracing::debug_span!("infra spec"))
            .await
        {
            Ok(spec) => spec,
            Err(error) => return PhaseResult::Error(error),
        };
        if empty_spec.is_empty() {
            return PhaseResult::Skipped(reason("no_infra", json!({})));
        }
        let needs = match engine
            .integ
            .infra
            .port_needs(&empty_spec)
            .instrument(tracing::debug_span!("infra port needs"))
            .await
        {
            Ok(needs) => needs,
            Err(error) => return PhaseResult::Error(infra_error(error)),
        };
        if let Err(error) = engine
            .allocate_lane_held(&cx.project, &cx.lane, &needs)
            .instrument(tracing::debug_span!("allocate"))
            .await
        {
            return PhaseResult::Error(error);
        }
        cx.infra_needs = needs;
        let spec = match engine
            .infra_spec(&cx.project, &cx.lane, &cx.infra_needs)
            .instrument(tracing::debug_span!("infra spec"))
            .await
        {
            Ok(spec) => spec,
            Err(error) => return PhaseResult::Error(error),
        };
        let skip = if !cx.infra_enabled {
            Some(profile_disables_infra(cx))
        } else if !cx.whole_lane && !cx.setup_only {
            Some(reason("lane_running", json!({})))
        } else if cx.infra_templated && !engine.profiles_enable_infra(cx, &spec).await {
            cx.infra_enabled = false;
            Some(profile_disables_infra(cx))
        } else {
            None
        };
        // The containers' state last reported on `progress` events.
        let mut shown = Vec::new();
        let result = match &skip {
            Some(_) => {
                engine
                    .integ
                    .infra
                    .namespace(&spec)
                    .instrument(tracing::debug_span!("infra namespace"))
                    .await
            }
            // A hung `up` must not keep a stop waiting for the lane.
            None => tokio::select! {
                up = engine.with_container_progress(cx, &spec, &mut shown, engine.integ.infra.up(&spec)).instrument(tracing::debug_span!("infra up")) => up,
                () = cx.cancel.cancelled() => return PhaseResult::Error(start_cancelled(cx)),
            },
        };
        match result {
            Ok(up) => {
                let mut st = cx.lane.st();
                // The views route to the containers; resolution refreshes them
                // when the namespace it plans with changed.
                if st.infra_ns.as_ref() != Some(&up.namespace) {
                    st.refresh_pending = true;
                }
                st.infra_ns = Some(up.namespace);
                st.infra_containers = up.containers;
                st.setup_env = up.setup_env;
            }
            Err(error) if skip.is_some() => {
                tracing::debug!(code = %error.code, "infra namespace unavailable");
            }
            Err(error) => return PhaseResult::Error(infra_error(error)),
        }
        if skip.is_none() {
            // Setup and services rely on the containers, so their `ready` checks
            // finish the phase; a stop still interrupts the wait.
            tokio::select! {
                result = engine.with_container_progress(cx, &spec, &mut shown, engine.containers_ready(&cx.lane, &spec)).instrument(tracing::debug_span!("containers ready")) => {
                    if let Err(error) = result {
                        return PhaseResult::Error(error);
                    }
                }
                () = cx.cancel.cancelled() => return PhaseResult::Error(start_cancelled(cx)),
            }
        }
        match skip {
            Some(reason) => PhaseResult::Skipped(reason),
            None => PhaseResult::Done,
        }
    }
}

#[async_trait]
impl LanePhase for ResolvePhase {
    fn name(&self) -> &'static str {
        "resolve"
    }

    fn visible(&self) -> bool {
        false
    }

    async fn run(&self, engine: &Arc<Inner>, cx: &mut StartCx) -> PhaseResult {
        // A bring-up's infra phase may have changed the namespace the views
        // route to; a start that adds units refreshes only for a reload that
        // waited for the lane.
        if cx.whole_lane || cx.lane.st().refresh_pending {
            engine.refresh_views_held(&cx.project, &cx.lane).await;
        }
        // A start that adds units to a running lane runs no tasks, so it plans only
        // its services and the setup entries; planning every service can cost a
        // contributor call each.
        let only: Option<BTreeSet<String>> = (!cx.whole_lane && !cx.setup_only).then(|| {
            cx.services
                .iter()
                .map(|name| format!("services.{name}"))
                .chain(
                    cx.config
                        .infra
                        .setup
                        .keys()
                        .map(|entry| format!("infra.setup.{entry}")),
                )
                .collect()
        });
        match engine
            .resolve_config(&cx.project, &cx.lane, cx.config.clone(), only.as_ref())
            .await
        {
            Ok(resolved) => {
                // Infra came up already when it was on; this governs `infra.setup`.
                cx.infra_enabled &= resolved.infra_enabled;
                // A failed plan keeps its service, so the services phase reports it.
                let pending = std::mem::take(&mut cx.autostart_pending);
                devlanes_resolve::retain_autostart(&mut cx.services, &pending, |name| {
                    resolved.autostart(name)
                });
                // From here a stop can name the tasks and setup entries this start
                // may run, also between their spawn and their record on the lane.
                for (name, task) in &resolved.tasks {
                    if task.active && matches!(task.when, Some(TaskWhen::Create | TaskWhen::Start))
                    {
                        cx.ctl.plan_tasks(task_units(&resolved, name));
                    }
                }
                cx.ctl
                    .plan_tasks(resolved.setup.keys().map(|entry| format!("setup.{entry}")));
                cx.resolved = Some(resolved);
                PhaseResult::Done
            }
            // The edited config needs ports the running lane's block cannot hold:
            // say so, rather than naming the missing offset.
            Err(error) if error.code == "port_error" && cx.lane.st().ports_pending => {
                PhaseResult::Error(RpcError::new(
                    "ports_pending",
                    json!({
                        "lane": cx.reference,
                        "unit": error.data.get("unit"),
                        "port": error.data.get("port"),
                        "message": format!(
                            "the edited config needs ports the running lane's block cannot hold; stop the lane, then start it again (devlanes stop --lane {})",
                            cx.reference
                        ),
                    }),
                    1,
                ))
            }
            Err(error) => PhaseResult::Error(error),
        }
    }
}

#[async_trait]
impl LanePhase for SetupPhase {
    fn name(&self) -> &'static str {
        "setup"
    }

    async fn run(&self, engine: &Arc<Inner>, cx: &mut StartCx) -> PhaseResult {
        let Some(resolved) = cx.resolved.clone() else {
            return PhaseResult::Skipped(reason("not_resolved", json!({})));
        };
        if resolved.setup.is_empty() {
            return PhaseResult::Skipped(reason("no_setup", json!({})));
        }
        if !cx.infra_enabled {
            return PhaseResult::Skipped(profile_disables_infra(cx));
        }
        let Some(lane_id) = cx.lane.st().record.as_ref().map(|r| r.id) else {
            return PhaseResult::Error(RpcError::new(
                "unassigned",
                json!({"lane": cx.reference}),
                1,
            ));
        };
        // Starts that add units run this phase side by side; one of them checks
        // and runs the entries while the others wait, then finds them done.
        let lane = cx.lane.clone();
        let _setup = lane.setup.lock().await;
        let records = match engine.store.setup_records(lane_id).await {
            Ok(records) => records,
            Err(error) => return PhaseResult::Error(store_error(error)),
        };
        let containers: BTreeSet<String> = resolved
            .setup
            .values()
            .filter_map(|command| command.container.clone())
            .collect();
        let started = tracing::Instrument::instrument(
            engine.containers_started_at(&cx.project, &cx.lane, &cx.infra_needs, containers),
            tracing::debug_span!("container start times"),
        )
        .await;
        let mut pending: Vec<(String, Option<String>)> = Vec::new();
        for (name, command) in &resolved.setup {
            let started_at = command
                .container
                .as_ref()
                .and_then(|container| started.get(container).cloned());
            let done = records.iter().find(|r| &r.entry == name);
            let hash = setup_hash(&cx.config, name);
            let current = match (done, &command.container) {
                (None, _) => false,
                (Some(record), None) => {
                    same_declaration(record.spec_hash.as_deref(), hash.as_deref())
                }
                // A restarted backing container lost the entry's effect (tmpfs data).
                (Some(record), Some(_)) => {
                    same_declaration(record.spec_hash.as_deref(), hash.as_deref())
                        && record.container_started_at == started_at
                }
            };
            if !current {
                pending.push((name.clone(), started_at));
            }
        }
        if pending.is_empty() {
            return PhaseResult::Skipped(reason("already_done", json!({})));
        }
        // A stop naming one of these entries interrupts the start.
        let _running = cx.ctl.running_tasks(
            pending
                .iter()
                .map(|(name, _)| format!("setup.{name}"))
                .collect(),
        );
        // Entries a stop withdrew before they ran are left for a later start.
        pending.retain(|(name, _)| cx.ctl.withdrawn_by(&format!("setup.{name}")).is_none());
        let runs = pending.iter().map(|(name, _)| {
            let engine = engine.clone();
            let project = cx.project.clone();
            let lane = cx.lane.clone();
            let actor = cx.actor.clone();
            let resolved = resolved.clone();
            let name = name.clone();
            let cancel = cx.cancel.clone();
            async move {
                let Some(command) = resolved.setup.get(&name) else {
                    return (name, Err(RpcError::new("not_found", json!({}), 3)));
                };
                let result = engine
                    .run_command(&project, &lane, "setup", command, &actor, &cancel, None)
                    .await;
                (name, result)
            }
        });
        // Entries run together; progress names one still running and counts the finished.
        let mut runs: FuturesUnordered<_> = runs.collect();
        let count = pending.len();
        let mut running: Vec<&String> = pending.iter().map(|(name, _)| name).collect();
        if let Some(first) = running.first() {
            engine.phase_step(cx, "setup", first, 1, count);
        }
        let mut results = Vec::with_capacity(count);
        while let Some((name, result)) = runs.next().await {
            running.retain(|running| **running != name);
            results.push((name, result));
            if let Some(next) = running.first() {
                engine.phase_step(cx, "setup", next, results.len() + 1, count);
            }
        }
        drop(runs);
        // Back in config order, as the failures are reported.
        results.sort_by_key(|(name, _)| pending.iter().position(|(entry, _)| entry == name));
        let mut failed = Vec::new();
        for (name, result) in results {
            if let Ok((_, host)) = &result {
                cx.leftovers
                    .extend(ended_leftovers(&cx.lane, &format!("setup.{name}"), host));
            }
            // An entry a stop interrupted did not run, whatever `ignore_errors` says.
            let ignore = resolved.setup.get(&name).is_some_and(|c| c.ignore_errors)
                && !cx.cancel.is_cancelled();
            let ok = matches!(&result, Ok((outcome, _)) if outcome.code == Some(0));
            if ok || ignore {
                let started_at = pending
                    .iter()
                    .find(|(n, _)| *n == name)
                    .and_then(|(_, at)| at.clone());
                let hash = setup_hash(&cx.config, &name);
                if let Err(error) = engine
                    .store
                    .record_setup(lane_id, name.clone(), started_at, hash)
                    .await
                {
                    return PhaseResult::Error(store_error(error));
                }
                continue;
            }
            let unit = format!("setup.{name}");
            match result {
                Ok((_, host)) => cx
                    .failures
                    .push(failure_of(&cx.reference, &unit, &host).await),
                Err(error) => cx.failures.push(UnitFailureData {
                    lane: cx.reference.clone(),
                    unit,
                    reason: Some(error),
                    ..UnitFailureData::default()
                }),
            }
            failed.push(name);
        }
        if failed.is_empty() {
            PhaseResult::Done
        } else {
            PhaseResult::Halt(reason("setup_failed", json!({"entries": failed})))
        }
    }
}

#[async_trait]
impl LanePhase for TasksPhase {
    fn name(&self) -> &'static str {
        if self.after {
            "after-services"
        } else {
            "tasks"
        }
    }

    async fn run(&self, engine: &Arc<Inner>, cx: &mut StartCx) -> PhaseResult {
        let Some(resolved) = cx.resolved.clone() else {
            return PhaseResult::Skipped(reason("not_resolved", json!({})));
        };
        if !cx.whole_lane && !cx.setup_only {
            return PhaseResult::Skipped(reason("lane_running", json!({})));
        }
        if self.after && cx.setup_only {
            return PhaseResult::Skipped(reason("setup_only", json!({})));
        }
        let Some(lane_id) = cx.lane.st().record.as_ref().map(|r| r.id) else {
            return PhaseResult::Error(RpcError::new(
                "unassigned",
                json!({"lane": cx.reference}),
                1,
            ));
        };
        let mut chosen = Vec::new();
        for (name, task) in &resolved.tasks {
            let hook = match task.when {
                Some(TaskWhen::Create) => "lane.create",
                Some(TaskWhen::Start) if !cx.setup_only => "lane.start",
                _ => continue,
            };
            if task.after_services != self.after || !task.active {
                continue;
            }
            if hook == "lane.create" {
                // Once per lane, and again after its declaration changed.
                let hash = task_hash(&cx.config, name);
                match engine.store.task_completion(lane_id, name.clone()).await {
                    Ok(Some(done))
                        if same_declaration(done.spec_hash.as_deref(), hash.as_deref()) =>
                    {
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => return PhaseResult::Error(store_error(error)),
                }
            }
            chosen.push((name.clone(), hook));
        }
        if chosen.is_empty() {
            return PhaseResult::Skipped(reason("no_tasks", json!({})));
        }
        if self.after {
            let hosts: Vec<UnitHost> = {
                let st = cx.lane.st();
                cx.started
                    .iter()
                    .chain(&cx.already_running)
                    .filter_map(|unit| st.units.get(unit).map(|rec| rec.host.clone()))
                    .collect()
            };
            let settled = tokio::select! {
                settled = wait_settled(&hosts, cx.deadline) => settled,
                () = cx.cancel.cancelled() => return PhaseResult::Error(start_cancelled(cx)),
            };
            if !settled {
                cx.timed_out = true;
                for (name, _) in &chosen {
                    cx.skipped_tasks.push(name.clone());
                    engine.task_event(
                        &cx.reference,
                        name,
                        "skipped",
                        None,
                        Some(json!({"code": "timed_out"})),
                    );
                }
                return PhaseResult::Skipped(reason("timed_out", json!({})));
            }
            let failed: Vec<String> = {
                let st = cx.lane.st();
                cx.started
                    .iter()
                    .chain(&cx.already_running)
                    .filter(|unit| {
                        st.units
                            .get(*unit)
                            .is_some_and(|rec| matches!(rec.host.state(), UnitState::Failed(_)))
                    })
                    .cloned()
                    .collect()
            };
            if !failed.is_empty() || !cx.failures.is_empty() {
                let mut units = failed;
                units.extend(cx.failures.iter().map(|f| f.unit.clone()));
                for (name, _) in &chosen {
                    cx.skipped_tasks.push(name.clone());
                    engine.task_event(
                        &cx.reference,
                        name,
                        "skipped",
                        None,
                        Some(json!({"code": "service_failed", "units": units})),
                    );
                }
                return PhaseResult::Skipped(reason("service_failed", json!({"units": units})));
            }
        }
        let count = chosen.len();
        for (index, (name, hook)) in chosen.into_iter().enumerate() {
            engine.phase_step(cx, self.name(), &name, index + 1, count);
            if !self.after {
                engine.note_step(cx, Some(&name));
            }
            // A stop naming the task or one of its steps interrupts the start.
            let _running = cx.ctl.running_tasks(task_units(&resolved, &name));
            // Checked after registering: a stop either saw it running (and
            // interrupts the start) or withdrew it before; never neither.
            if cx.ctl.withdrawn_by(&name).is_some() {
                cx.skipped_tasks.push(name.clone());
                engine.task_event(
                    &cx.reference,
                    &name,
                    "skipped",
                    None,
                    Some(json!({"code": "cancelled_by_stop"})),
                );
                continue;
            }
            let outcome = engine
                .run_task(
                    &cx.project,
                    &cx.lane,
                    &resolved,
                    &name,
                    hook,
                    &cx.actor,
                    cx.session.as_ref(),
                    &cx.cancel,
                    0,
                )
                .await;
            if cx.cancel.is_cancelled() {
                return PhaseResult::Error(start_cancelled(cx));
            }
            if let Ok(outcome) = &outcome {
                cx.leftovers.extend(outcome.leftovers.iter().cloned());
            }
            match outcome {
                Ok(TaskOutcome { code: Some(0), .. }) => {}
                Ok(TaskOutcome { code, host, .. }) => {
                    let mut failure = match &host {
                        Some(host) => failure_of(&cx.reference, &name, host).await,
                        None => UnitFailureData {
                            lane: cx.reference.clone(),
                            unit: name.clone(),
                            ..UnitFailureData::default()
                        },
                    };
                    failure.exit_code = code;
                    failure.reason.get_or_insert_with(|| {
                        RpcError::new("task_failed", json!({"task": name, "exit_code": code}), 1)
                    });
                    cx.failures.push(failure);
                    return PhaseResult::Halt(reason("task_failed", json!({"task": name})));
                }
                Err(error) => {
                    cx.failures.push(UnitFailureData {
                        lane: cx.reference.clone(),
                        unit: name.clone(),
                        reason: Some(error),
                        ..UnitFailureData::default()
                    });
                    return PhaseResult::Halt(reason("task_failed", json!({"task": name})));
                }
            }
        }
        PhaseResult::Done
    }
}

#[async_trait]
impl LanePhase for ServicesPhase {
    fn name(&self) -> &'static str {
        "services"
    }

    async fn run(&self, engine: &Arc<Inner>, cx: &mut StartCx) -> PhaseResult {
        if cx.setup_only {
            return PhaseResult::Skipped(reason("setup_only", json!({})));
        }
        let Some(resolved) = cx.resolved.clone() else {
            return PhaseResult::Skipped(reason("not_resolved", json!({})));
        };
        if cx.services.is_empty() {
            return PhaseResult::Skipped(reason("no_services", json!({})));
        }
        let mut to_spawn: Vec<&ResolvedService> = Vec::new();
        // A copy, because the paced spawn below needs `cx` mutably.
        let selected = cx.services.clone();
        for name in &selected {
            if let Some(error) = resolved.errors.get(&format!("services.{name}")) {
                cx.failures.push(UnitFailureData {
                    lane: cx.reference.clone(),
                    unit: name.clone(),
                    reason: Some(crate::resolve::resolve_error(error)),
                    ..UnitFailureData::default()
                });
                continue;
            }
            for service in resolved.units_of(name) {
                // A stop or restart took it out of this start; the result says so.
                if cx.ctl.withdrawn_by(&service.unit).is_some() {
                    continue;
                }
                // Never adopted: the user stops it explicitly; other units go on.
                let orphaned = cx.lane.st().orphan_error(&cx.project.name(), &service.unit);
                if let Some(error) = orphaned {
                    cx.failures.push(UnitFailureData {
                        lane: cx.reference.clone(),
                        unit: service.unit.clone(),
                        reason: Some(error),
                        ..UnitFailureData::default()
                    });
                    continue;
                }
                let alive = cx
                    .lane
                    .st()
                    .units
                    .get(&service.unit)
                    .is_some_and(|rec| rec.host.is_alive());
                if alive {
                    cx.already_running.push(service.unit.clone());
                    continue;
                }
                if service.literal_port
                    && let Some((lane, unit)) = engine.literal_port_holder(service.port, &cx.lane)
                {
                    cx.failures.push(UnitFailureData {
                        lane: cx.reference.clone(),
                        unit: service.unit.clone(),
                        reason: Some(RpcError::new(
                            "port_in_use_by_lane",
                            json!({"port": service.port, "lane": lane, "unit": unit}),
                            1,
                        )),
                        ..UnitFailureData::default()
                    });
                    continue;
                }
                // A connected box's mirror may hold one of these ports on loopback,
                // where a ready check would reach the box instead of this service.
                let ports: BTreeSet<u16> = service
                    .ports
                    .values()
                    .copied()
                    .chain([service.port])
                    .filter(|port| *port != 0)
                    .collect();
                if let Err(error) = engine.release_for_local(&ports).await {
                    cx.failures.push(UnitFailureData {
                        lane: cx.reference.clone(),
                        unit: service.unit.clone(),
                        reason: Some(error),
                        ..UnitFailureData::default()
                    });
                    continue;
                }
                to_spawn.push(service);
            }
        }
        // A unit another start of the lane is about to spawn stays with that
        // start; this one waits for it as for a running unit. Checked and
        // claimed under the lane lock, so two starts never both spawn it.
        let joined: Vec<(String, Arc<StartCtl>)> = {
            let st = cx.lane.st();
            let mut joined = Vec::new();
            let mut alive = Vec::new();
            to_spawn.retain(|service| {
                // Another start spawned it since the check above.
                if st
                    .units
                    .get(&service.unit)
                    .is_some_and(|rec| rec.host.is_alive())
                {
                    alive.push(service.unit.clone());
                    return false;
                }
                let other = st
                    .starts
                    .iter()
                    .filter(|other| !Arc::ptr_eq(other, &cx.ctl))
                    .find(|other| other.will_spawn(&service.unit));
                match other {
                    Some(other) => {
                        joined.push((service.unit.clone(), other.clone()));
                        false
                    }
                    None => true,
                }
            });
            cx.ctl
                .plan_spawns(to_spawn.iter().map(|service| service.unit.clone()));
            cx.already_running.extend(alive);
            joined
        };
        if !engine.integ.launch_checks.is_empty() {
            let starting: Vec<PlannedUnit> = to_spawn
                .iter()
                .map(|service| PlannedUnit {
                    label: format!("{}:{}", cx.reference, service.unit),
                    preset: service.preset.clone(),
                    files: service.launch.files.clone(),
                })
                .collect();
            let running = engine.running_planned(&cx.project);
            for check in &engine.integ.launch_checks {
                if let Err(message) = check.check(&starting, &running) {
                    return PhaseResult::Error(RpcError::new(
                        "launch_check_failed",
                        json!({"message": message}),
                        1,
                    ));
                }
            }
        }
        // Each spawn waits for a start slot; see `pacing`.
        engine.spawn_services(cx, &to_spawn, &resolved).await;
        for (unit, other) in joined {
            match joined_outcome(&cx.ctl, cx.deadline, &unit, &other).await {
                // What the other start got for the unit is this start's answer too;
                // waited for like a running unit, so its exit still counts.
                Joined::Spawned => cx.already_running.push(unit),
                Joined::Failed(error) => cx.failures.push(UnitFailureData {
                    lane: cx.reference.clone(),
                    unit,
                    reason: Some(error),
                    ..UnitFailureData::default()
                }),
                Joined::TimedOut => cx.timed_out = true,
                Joined::Gone => {}
            }
        }
        PhaseResult::Done
    }
}

/// Exit of a task: its code (`None` when killed or timed out) and its last unit.
pub(crate) struct TaskOutcome {
    pub code: Option<i32>,
    /// The last unit's own exit code. It differs from `code` only for an infra
    /// entry with `ignore_errors`, which counts a failure as done for starts
    /// and steps; a `run` of that one entry reports what really happened.
    pub raw_code: Option<i32>,
    pub host: Option<UnitHost>,
    /// Every unit of the task (steps included) that left processes behind.
    pub leftovers: Vec<Leftovers>,
}

/// Saved names a start brought back without, because the config no longer has them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Dropped {
    pub profiles: Vec<String>,
    pub services: Vec<String>,
}

impl Dropped {
    /// One `saved_profile_removed` / `saved_service_removed` warning per kind.
    pub(crate) fn warnings(&self, lane: &str) -> Vec<devlanes_proto::LaneWarning> {
        let mut warnings = Vec::new();
        for (code, names, what) in [
            ("saved_profile_removed", &self.profiles, "profile"),
            ("saved_service_removed", &self.services, "service"),
        ] {
            if names.is_empty() {
                continue;
            }
            let plural = if names.len() == 1 { "" } else { "s" };
            warnings.push(devlanes_proto::LaneWarning {
                code: code.into(),
                data: json!({
                    "lane": lane,
                    "names": names,
                    "message": format!(
                        "{lane} ran {what}{plural} {} before, which the config no longer has; started without",
                        names.join(", ")
                    ),
                }),
            });
        }
        warnings
    }
}

/// Splits saved service or unit names into the services the config still declares,
/// deduplicated, and the names it dropped.
fn split_saved(
    config: &devlanes_config::ProjectConfig,
    names: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut kept: Vec<String> = Vec::new();
    let mut dropped = Vec::new();
    for name in names {
        match crate::drift::service_of(config, name) {
            Some(service) if !kept.iter().any(|known| known == service) => {
                kept.push(service.to_owned());
            }
            Some(_) => {}
            None => dropped.push(name.clone()),
        }
    }
    (kept, dropped)
}

/// The services a start selects, and the saved names it left out. `saved` is the
/// lane's record and `lane_profiles` the profiles in effect; `whole_lane` says the
/// start brings the lane up. Saved profiles and services the config no longer has
/// are dropped instead of failing the start, which a profile typed on the command line
/// still does. Pure, so a start can name its units before it waits for `op`.
fn start_selection(
    config: &devlanes_config::ProjectConfig,
    saved: Option<&devlanes_state::Lane>,
    lane_profiles: &[String],
    options: &StartOptions,
    whole_lane: bool,
) -> Result<(devlanes_resolve::Selection, Dropped), devlanes_resolve::ResolveError> {
    let mut dropped = Dropped::default();
    // The saved record, not the profiles in effect: a transient start changes
    // those but must not be what the next plain start picks up.
    let mut requested = if options.profiles.is_empty() {
        saved.map_or_else(|| lane_profiles.to_vec(), |r| r.profiles.clone())
    } else {
        options.profiles.clone()
    };
    if options.profiles.is_empty() || options.restoring {
        let (known, gone): (Vec<String>, Vec<String>) = requested.into_iter().partition(|name| {
            name == devlanes_proto::DEFAULT_PROFILE || config.profiles.contains_key(name)
        });
        requested = known;
        dropped.profiles = gone;
    }
    let restored = if options.selectors.is_empty() && options.profiles.is_empty() {
        let names = saved
            .map(|record| record.services.clone())
            .unwrap_or_default();
        let (kept, gone) = split_saved(config, &names);
        dropped.services = gone;
        kept
    } else {
        Vec::new()
    };
    // An idle lane comes back as it ran; a running one also gets what its
    // profiles select and it lacks, such as a service added to the config.
    let selectors = if whole_lane && !restored.is_empty() {
        restored.clone()
    } else if options.restoring {
        let (kept, gone) = split_saved(config, &options.selectors);
        dropped.services.extend(gone);
        kept
    } else {
        options.selectors.clone()
    };
    let mut selection = devlanes_resolve::select_profiles(config, &requested, &selectors)?;
    if !whole_lane && !restored.is_empty() {
        selection.services = config
            .services
            .keys()
            .filter(|name| selection.services.contains(name) || restored.contains(name))
            .cloned()
            .collect();
        selection
            .autostart_pending
            .retain(|name| !restored.contains(name));
    }
    Ok((selection, dropped))
}

/// How a unit another start was spawning ended for a start that joined it.
enum Joined {
    Spawned,
    Failed(RpcError),
    /// This start's deadline passed first.
    TimedOut,
    /// Taken out by a stop, or the other start ended without trying it.
    Gone,
}

/// Waits until `other` settled `unit`: its spawn finished, a stop withdrew it,
/// or the other start ended. Only this unit counts, not the other start's
/// remaining services.
async fn joined_outcome(
    ours: &StartCtl,
    deadline: Instant,
    unit: &str,
    other: &StartCtl,
) -> Joined {
    loop {
        let changed = other.changed.notified();
        tokio::pin!(changed);
        // Registered before the checks, so an outcome recorded in between wakes it.
        changed.as_mut().enable();
        match other.outcome(unit) {
            Some(None) => return Joined::Spawned,
            Some(Some(error)) => return Joined::Failed(error),
            None => {}
        }
        if other.withdrawn_by(unit).is_some() || ours.withdrawn_by(unit).is_some() {
            return Joined::Gone;
        }
        tokio::select! {
            () = changed => {}
            // Ended without trying it (cancelled); a spawn still queued past the
            // other start's deadline records its outcome later, so keep waiting.
            () = other.cancel.cancelled() => return Joined::Gone,
            () = ours.cancel.cancelled() => return Joined::Gone,
            () = tokio::time::sleep_until(deadline) => return Joined::TimedOut,
        }
    }
}

/// The words a waiting unit shows for the bring-up's current phase; `None`
/// once the bring-up reached its services.
fn bringup_step(phase: &str) -> Option<&'static str> {
    match phase {
        "infra" => Some("containers"),
        "setup" => Some("setup"),
        "tasks" => Some("tasks"),
        _ => None,
    }
}

/// How a start holds its lane's `op`.
struct Lease<'a> {
    guard: LeaseGuard<'a>,
    /// The start brings the lane up (it took `op` exclusively).
    brings_up: bool,
}

/// The guard behind a [`Lease`].
enum LeaseGuard<'a> {
    Exclusive(tokio::sync::RwLockWriteGuard<'a, ()>),
    /// Only held, never read: dropping it lets an exclusive holder in.
    Shared {
        _guard: tokio::sync::RwLockReadGuard<'a, ()>,
    },
}

impl Lease<'_> {
    /// Whether the start brings the lane up.
    fn brings_up(&self) -> bool {
        self.brings_up
    }

    /// Shares `op` from here on, so starts waiting to add units go ahead while
    /// this one spawns its services.
    fn share(self) -> Self {
        let guard = match self.guard {
            LeaseGuard::Exclusive(guard) => LeaseGuard::Shared {
                _guard: guard.downgrade(),
            },
            shared => shared,
        };
        Self {
            guard,
            brings_up: self.brings_up,
        }
    }
}

/// Where one start spent its time, for the line every start leaves in
/// `daemon.log`. A bug report then shows the slow step on the reporter's
/// machine without a rerun under `-v`.
struct StartTimings {
    began: Instant,
    /// Waiting for `op`: behind a bring-up's early phases or a whole-lane stop.
    lock: Duration,
    phases: Vec<(&'static str, Duration)>,
}

impl StartTimings {
    /// Logs `start finished` with the total, the wait for the lane, each
    /// phase that took time and the longest waits for a start slot.
    fn log(&self, lane: &str, start_id: u64, whole_lane: bool, ctl: &StartCtl, outcome: &str) {
        let ms = |duration: Duration| duration.as_millis() as u64;
        let phases: Vec<String> = self
            .phases
            .iter()
            .filter(|(_, took)| ms(*took) > 0)
            .map(|(name, took)| format!("{name} {}ms", ms(*took)))
            .collect();
        let mut waits = ctl.gate_waits();
        let queued = waits.iter().filter(|(_, waited)| *waited > 0).count();
        waits.sort_by_key(|(_, waited)| std::cmp::Reverse(*waited));
        let longest: Vec<String> = waits
            .iter()
            .filter(|(_, waited)| *waited > 0)
            .take(3)
            .map(|(unit, waited)| format!("{unit} {waited}ms"))
            .collect();
        tracing::info!(
            lane,
            start_id,
            whole_lane,
            outcome,
            total_ms = ms(self.began.elapsed()),
            lane_wait_ms = ms(self.lock),
            phases = %phases.join(", "),
            slot_waits = queued,
            longest_slot_waits = %longest.join(", "),
            "start finished"
        );
    }
}

/// Clears the lane's bring-up mark when the bring-up ends, however it ends.
struct BringupMark {
    lane: Arc<LaneRt>,
    start_id: u64,
}

impl Drop for BringupMark {
    /// Clears the mark unless a later bring-up owns it.
    fn drop(&mut self) {
        let mut st = self.lane.st();
        if st
            .bringup
            .as_ref()
            .is_some_and(|bringup| bringup.start_id == self.start_id)
        {
            st.bringup = None;
        }
    }
}

/// One start's or `infra up`'s share of the lane's `claiming` count.
pub(crate) struct ClaimMark {
    lane: Arc<LaneRt>,
    engine: std::sync::Weak<Inner>,
}

impl Drop for ClaimMark {
    /// Ends this share; the lane's units and containers count from then on.
    /// Publishes, so a start that failed or left only containers up gives
    /// box lanes back or reports what holds the ports, whichever way it
    /// returned.
    fn drop(&mut self) {
        {
            let mut st = self.lane.st();
            st.claiming = st.claiming.saturating_sub(1);
        }
        if let Some(engine) = self.engine.upgrade() {
            engine.publish_routes();
        }
    }
}

impl Inner {
    /// Counts `lane` as running for box lanes it clashes with until the mark
    /// drops, and waits until the mirrors gave those lanes up, so their ports
    /// are free before infra or a service binds them.
    pub(crate) async fn claim_ports(self: &Arc<Self>, lane: &Arc<LaneRt>) -> ClaimMark {
        lane.st().claiming += 1;
        let mark = ClaimMark {
            lane: lane.clone(),
            engine: Arc::downgrade(self),
        };
        let (version, _) = self.reassign();
        self.await_applied(version).await;
        mark
    }
}

/// One start's share of the lane's `starting` state.
struct StartingMark {
    lane: Arc<LaneRt>,
}

impl Drop for StartingMark {
    /// Lets the lane read its own state again once no start is in its phases.
    fn drop(&mut self) {
        let mut st = self.lane.st();
        st.starting = st.starting.saturating_sub(1);
        if st.starting == 0 && st.busy == Some("starting") {
            st.busy = None;
        }
    }
}

/// `kept` and then `planned`, without repeats and without the services a stop
/// took out of the start (a unit is `name` or `name.<replica>`).
fn remembered_services(kept: &[String], planned: &[String], ctl: &StartCtl) -> Vec<String> {
    let stopped: Vec<String> = ctl
        .withdrawn()
        .into_iter()
        .filter(|(_, by)| *by == "stop")
        .map(|(unit, _)| unit)
        .collect();
    let gone = |service: &str| {
        stopped.iter().any(|unit| {
            unit == service
                || unit
                    .strip_prefix(service)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    };
    let mut services: Vec<String> = Vec::new();
    for name in kept.iter().chain(planned) {
        if !gone(name) && !services.contains(name) {
            services.push(name.clone());
        }
    }
    services
}

/// `saved` without the services of `touched` units that no longer run: a stop
/// of one replica keeps its service while another replica still runs.
fn without_stopped(lane: &LaneRt, saved: &[String], touched: &[String]) -> Vec<String> {
    let st = lane.st();
    let service_of = |unit: &str| -> String {
        st.view
            .as_ref()
            .and_then(|view| view.services.iter().find(|s| s.unit == unit))
            .map_or_else(|| unit.to_owned(), |s| s.name.clone())
    };
    let alive: BTreeSet<String> = st
        .units
        .iter()
        .filter(|(_, rec)| rec.kind == Kind::Service && rec.host.is_alive())
        .map(|(unit, _)| service_of(unit))
        .chain(st.orphans.keys().map(|unit| service_of(unit)))
        .collect();
    let stopped: BTreeSet<String> = touched
        .iter()
        .map(|unit| service_of(unit))
        .filter(|service| !alive.contains(service))
        .collect();
    saved
        .iter()
        .filter(|name| !stopped.contains(*name))
        .cloned()
        .collect()
}

/// Options of one start request.
pub(crate) struct StartOptions {
    pub selectors: Vec<String>,
    /// Requested profiles; empty takes the lane's saved ones and, without
    /// selectors either, brings back its saved services.
    pub profiles: Vec<String>,
    /// Leaves the lane's saved profiles, services and desired state alone
    /// (`devlanes with`).
    pub transient: bool,
    pub env: Option<Arc<EnvMap>>,
    pub overrides: EnvMap,
    pub actor: Actor,
    pub timeout: Duration,
    pub session: Option<Session>,
    pub setup_only: bool,
    /// Bypass the start gate for this request.
    pub all_at_once: bool,
    /// The `start_id` to use, chosen by a caller that streams this start's
    /// events; `None` takes the next one.
    pub start_id: Option<u64>,
    /// Place in the start gate's line, taken when the request arrived; `None`
    /// takes one when the services phase begins.
    pub arrival: Option<devlanes_core::gate::Arrival>,
    /// `selectors` and `profiles` are what the lane ran before (a replacing
    /// daemon's resume plan), so names the config no longer has are left out with
    /// a warning instead of failing the start.
    pub restoring: bool,
}

type Spawned = (UnitHost, Arc<dyn ManagedUnit>);

use crate::idle::Activity;

impl Inner {
    /// Spawns a unit, records its run and starts its state watcher.
    pub(crate) async fn spawn_unit(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        request: SpawnRequest,
    ) -> Result<Spawned, RpcError> {
        // Counted before the shutdown check: once shutdown saw no spawn in flight,
        // every later one sees the cancelled token and refuses. The lane's own
        // `lane.stop` tasks of an explicit shutdown are the exception.
        let _spawning = lane.spawn_guard();
        if self.shutdown.is_cancelled() && !lane.st().shutdown_hooks {
            return Err(crate::engine::daemon_stopping());
        }
        // A new unit of a frozen lane would find its neighbours suspended.
        self.wake_lane(project, lane, Some(Activity::Command)).await;
        let (lane_path, record, reference, previous, color) = {
            let st = lane.st();
            let color = request
                .color
                .clone()
                .unwrap_or_else(|| crate::model::derived_color(&st, &request.name));
            let reference = st.reference(&project.name());
            if let Some(error) = st.orphan_error(&project.name(), &request.name) {
                return Err(error);
            }
            let record = st
                .record
                .clone()
                .ok_or_else(|| RpcError::new("unassigned", json!({"lane": reference}), 1))?;
            let previous = st
                .units
                .get(&request.name)
                .map(|rec| (rec.run_id, rec.host.clone()));
            (st.path.clone(), record, reference, previous, color)
        };
        if let Some((run_id, host)) = previous {
            if host.is_alive() {
                return Err(RpcError::new(
                    "unit_running",
                    json!({"lane": reference, "unit": request.name}),
                    1,
                ));
            }
            // The watcher may not have recorded the exit yet; finishing is idempotent.
            // While the old row stays unfinished, a new run could not be recorded.
            if let Some(id) = run_id {
                let outcome = host.exit_outcome().unwrap_or(ExitOutcome {
                    code: None,
                    signal: None,
                });
                if !self.finish_run(id, outcome).await {
                    return Err(RpcError::new(
                        "state_error",
                        json!({"lane": reference, "unit": request.name, "message": "the previous run could not be recorded as finished in state.db"}),
                        1,
                    ));
                }
            }
        }
        let data_path = self.lane_data_path(&project.name(), record.id);
        write_files(&[lane_path, data_path], request.files.clone()).await?;
        // The process exists before its run row does. A daemon crash between the
        // spawn and `record_unit_run` below leaves a process no later daemon knows
        // about: reconcile cannot list it as an orphan and nothing stops it. The
        // row needs the pid and start time, so it cannot be written first.
        let runner = &self.integ.runner;
        let attempt = crate::spawn_error::Attempt {
            unit: request.name.clone(),
            program: request.spec.program.clone(),
            cwd: request.spec.cwd.clone(),
            path: request.spec.env.get("PATH").cloned(),
        };
        // Keys only: values can hold secrets.
        tracing::debug!(
            unit = %request.name,
            program = %attempt.program,
            args = request.spec.args.len(),
            cwd = %attempt.cwd.display(),
            pty = request.spec.pty,
            env_keys = %request.spec.env.keys().map(String::as_str).collect::<Vec<_>>().join(","),
            "spawning"
        );
        let pty = request.spec.pty;
        let spawned = if request.kind == Kind::Task {
            runner.spawn_task(request.spec).await
        } else {
            runner.spawn(request.spec).await
        };
        let log_dir = unit_log_dir(
            &self.opts.logs,
            &project.name(),
            record.index,
            &request.name,
        );
        let managed = match spawned {
            Ok(managed) => managed,
            Err(error) => {
                return Err(self.spawn_failed(attempt, error, log_dir, &reference).await);
            }
        };
        let identity = managed.identity();
        tracing::debug!(unit = %request.name, pid = identity.map(|i| i.pid), "spawned");
        let info = UnitInfo {
            name: request.name.clone(),
            lane: reference.clone(),
            kind: request.kind.word(),
            actor: request.actor.clone(),
            started_at_ms: now_ms(),
            log_dir: log_dir.clone(),
            retention: crate::units::LogRetention::from_config(&self.opts.user.logs.retention),
            pty,
        };
        let unit: Arc<dyn Unit> = managed.clone();
        let host = match UnitHost::start(info, unit, identity).await {
            Ok(host) => host,
            Err(error) => {
                // Without a host nobody could see or stop it; this spawn is ours to undo.
                let _ = managed.stop(request.stop).await;
                return Err(error);
            }
        };
        let run_id = match identity {
            Some(process) if recordable(&process) => {
                let boot_id = tokio::task::spawn_blocking(devlanes_sys::boot_id)
                    .await
                    .ok()
                    .and_then(Result::ok);
                let recorded = self
                    .store
                    .record_unit_run(devlanes_state::UnitRunInput {
                        lane_id: record.id,
                        unit: request.name.clone(),
                        process,
                        actor: core_actor(&request.actor),
                        log_path: Some(log_dir),
                        boot_id,
                    })
                    .await;
                match recorded {
                    Ok(run) => Some(run.id),
                    Err(error) => {
                        // An unrecorded process would survive a daemon crash with no
                        // way to find or stop it, so it does not run at all.
                        tracing::warn!(%error, unit = %request.name, "unit run not recorded; stopping it");
                        let _ = host.stop(request.stop).await;
                        return Err(store_error(error));
                    }
                }
            }
            _ => None,
        };
        lane.st().units.insert(
            request.name.clone(),
            UnitRec {
                host: host.clone(),
                kind: request.kind,
                run_id,
                ports: request.ports,
                urls: request.urls,
                tags: request.tags,
                hidden: request.hidden,
                autostart: request.autostart,
                stop: request.stop,
                literal_port: request.literal_port,
                env: request.env,
                failure_tail: Vec::new(),
                failure_tail_styled: Vec::new(),
                preset: request.preset,
                files: request.files,
                color: Some(color),
                managed: managed.clone(),
                syncs: Vec::new(),
                display_name: request.display_name,
                ready_check: request.ready_check,
                role: request.role,
                keep_awake: request.keep_awake,
                fingerprint: request.fingerprint.map(Arc::new),
            },
        );
        if request.kind == Kind::Service {
            crate::local_sync::start(
                lane.clone(),
                request.name.clone(),
                host.clone(),
                request.sync,
            );
        }
        self.watch_unit(
            project.clone(),
            lane.clone(),
            request.name,
            host.clone(),
            run_id,
        );
        if let Some(run) = run_id {
            self.track_escaped(host.clone(), run);
        }
        Ok((host, managed))
    }

    /// Publishes every state transition and records the exit.
    fn watch_unit(
        self: &Arc<Self>,
        project: Arc<ProjectRt>,
        lane: Arc<LaneRt>,
        name: String,
        host: UnitHost,
        run_id: Option<devlanes_core::UnitRunId>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            let mut watch = host.watch_state();
            loop {
                let state = watch.borrow_and_update().clone();
                if let UnitState::Failed(_) = state {
                    let (tail, styled) =
                        devlanes_proto::split_styled(host.tail_lines(TAIL, true).await);
                    if let Some(rec) = lane
                        .st()
                        .units
                        .get_mut(&name)
                        .filter(|r| r.host.same_run(&host))
                    {
                        rec.failure_tail = tail;
                        rec.failure_tail_styled = styled;
                    }
                }
                engine.emit_unit(&project, &lane, &name, &host);
                if matches!(state, UnitState::Exited(_)) || !host.is_alive() {
                    break;
                }
                if watch.changed().await.is_err() {
                    break;
                }
            }
            let outcome = host.wait_exit().await;
            // A sync mirrors a running service's output; it ends with the run.
            crate::local_sync::stop(&lane, &name, &host).await;
            // On failure the lane's record keeps `run_id`; the next start of this
            // unit finishes the row again before recording a new run.
            if let Some(id) = run_id {
                engine.finish_run(id, outcome).await;
            }
            engine.emit_unit(&project, &lane, &name, &host);
            if let Some(id) = run_id {
                // The log closes right after the exit; its segments are final then.
                for segment in host.run_segments(Duration::from_secs(5)).await {
                    let _ = engine
                        .store
                        .record_log_segment(devlanes_state::LogSegmentInput {
                            unit_run_id: id,
                            first_seq: segment.first_seq,
                            last_seq: segment.last_seq,
                            min_ts_ms: segment.min_timestamp_ms,
                            max_ts_ms: segment.max_timestamp_ms,
                            path: segment.path,
                            compressed: segment.compressed,
                            bytes: segment.bytes,
                        })
                        .await;
                }
            }
        });
    }

    /// Records why the selected units that did not start stayed idle (`phase_failed`)
    /// and emits `unit.changed` for each, so clients see it on the unit itself.
    fn mark_unstarted(
        &self,
        project: &ProjectRt,
        lane: &LaneRt,
        cx: &StartCx,
        phase: &str,
        error: &RpcError,
    ) {
        let reason = RpcError::new(
            "phase_failed",
            json!({
                "lane": cx.reference,
                "phase": phase,
                "error": {"code": error.code, "data": error.data},
            }),
            1,
        );
        let summaries = {
            let mut st = lane.st();
            let units: Vec<String> = st
                .view
                .as_ref()
                .map(|view| {
                    view.services
                        .iter()
                        .filter(|service| cx.services.contains(&service.name))
                        .map(|service| service.unit.clone())
                        .filter(|unit| {
                            !cx.started.contains(unit)
                                && !cx.already_running.contains(unit)
                                && cx.ctl.withdrawn_by(unit).is_none()
                        })
                        .collect()
                })
                .unwrap_or_default();
            for unit in &units {
                st.phase_reasons.insert(unit.clone(), reason.clone());
            }
            let summary = crate::model::lane_summary(&project.name(), &st);
            summary
                .units
                .into_iter()
                .filter(|unit| units.contains(&unit.name))
                .collect::<Vec<_>>()
        };
        for summary in summaries {
            self.bus
                .emit(events::UNIT_CHANGED, json!({"unit": summary}));
        }
    }

    /// Emits `unit.changed` and `lane.changed` for one unit.
    pub(crate) fn emit_unit(
        &self,
        project: &ProjectRt,
        lane: &LaneRt,
        name: &str,
        host: &UnitHost,
    ) {
        let summary = {
            let st = lane.st();
            st.units
                .get(name)
                .filter(|rec| rec.host.same_run(host))
                .map(|rec| {
                    let mut summary = unit_summary(&st.reference(&project.name()), name, rec);
                    crate::model::apply_phase_reason(&st, &mut summary);
                    crate::model::apply_order(&st, &mut summary);
                    summary
                })
        };
        if let Some(summary) = summary {
            self.bus
                .emit(events::UNIT_CHANGED, json!({"unit": summary}));
        }
        self.emit_lane(project, lane);
    }

    /// Emits a `task` event.
    pub(crate) fn task_event(
        &self,
        lane: &str,
        task: &str,
        state: &str,
        exit_code: Option<i32>,
        reason: Option<Value>,
    ) {
        let mut data = json!({"lane": lane, "task": task, "state": state});
        if let Some(code) = exit_code {
            data["exit_code"] = json!(code);
        }
        if let Some(reason) = reason {
            data["reason"] = reason;
        }
        if matches!(state, "done" | "failed") {
            data["finished_at_ms"] = json!(now_ms());
        }
        self.bus.emit(events::TASK, data);
    }

    /// Emits a `lane.phase` event of the start `cx`; `units` are the units the
    /// services phase is about to spawn.
    fn phase_event(
        &self,
        cx: &StartCx,
        phase: &str,
        state: &str,
        reason: Option<&RpcError>,
        units: Option<Vec<String>>,
    ) {
        let mut data = json!({
            "lane": cx.reference,
            "phase": phase,
            "state": state,
            "start_id": cx.start_id,
        });
        if let Some(reason) = reason {
            // A whole error, so clients can word it as they word any other.
            data["reason"] = json!(reason);
        }
        if let Some(units) = units {
            data["units"] = json!(units);
        }
        self.bus.emit(events::PHASE, data);
    }

    /// Takes `op` for a start: exclusively when the start brings the lane up
    /// (nothing runs and no other start is bringing it up), shared otherwise.
    /// A shared start that finds the lane down once it gets `op` (the bring-up
    /// it waited for failed before anything ran) tries again to bring it up
    /// itself. `None` when `cancel` fired first.
    async fn lane_lease<'a>(
        &self,
        lane: &'a LaneRt,
        start_id: u64,
        cancel: &CancellationToken,
    ) -> Option<Lease<'a>> {
        loop {
            let deciding = tokio::select! {
                deciding = lane.decide.lock() => deciding,
                () = cancel.cancelled() => return None,
            };
            let brings_up = {
                let st = lane.st();
                !st.any_running() && st.bringup.is_none()
            };
            if brings_up {
                // `decide` stays held through this wait: a start arriving
                // meanwhile waits for the decision, then shares `op`.
                let guard = tokio::select! {
                    guard = lane.op.write() => guard,
                    () = cancel.cancelled() => return None,
                };
                let mut st = lane.st();
                // A stop may have left units running while this one waited.
                if !st.any_running() && st.bringup.is_none() {
                    st.bringup = Some(crate::model::Bringup {
                        start_id,
                        step: None,
                    });
                    drop(deciding);
                    return Some(Lease {
                        guard: LeaseGuard::Exclusive(guard),
                        brings_up: true,
                    });
                }
            } else {
                drop(deciding);
                let guard = tokio::select! {
                    guard = lane.op.read() => guard,
                    () = cancel.cancelled() => return None,
                };
                let st = lane.st();
                if st.any_running() || st.bringup.is_some() {
                    return Some(Lease {
                        guard: LeaseGuard::Shared { _guard: guard },
                        brings_up: false,
                    });
                }
            }
        }
    }

    /// Shows the units `options` asks for as `queued` right away, before the
    /// start waits for anything. Best effort: the start selects again once it
    /// holds `op`, and anything it does not spawn leaves the list with it.
    fn mark_waiting(
        &self,
        project: &ProjectRt,
        lane: &LaneRt,
        options: &StartOptions,
        start_id: u64,
    ) {
        if options.setup_only {
            return;
        }
        let (config, view, saved, lane_profiles, whole_lane) = {
            let st = lane.st();
            let (Some(config), Some(view)) = (st.config.loaded().cloned(), st.view.clone()) else {
                return;
            };
            (
                config,
                view,
                st.record.clone(),
                st.profiles.clone(),
                !st.any_running() && st.bringup.is_none(),
            )
        };
        let Ok((selection, _)) =
            start_selection(&config, saved.as_ref(), &lane_profiles, options, whole_lane)
        else {
            return;
        };
        let marked = {
            let mut st = lane.st();
            let mut marked = false;
            for service in &view.services {
                if !selection.services.contains(&service.name)
                    || st.orphans.contains_key(&service.unit)
                    || st
                        .units
                        .get(&service.unit)
                        .is_some_and(|rec| rec.host.is_alive())
                {
                    continue;
                }
                // An earlier start of the unit keeps it; this one joins it.
                st.waiting
                    .entry(service.unit.clone())
                    .or_insert_with(|| crate::model::Waiting {
                        start_id,
                        actor: options.actor.clone(),
                    });
                marked = true;
            }
            marked
        };
        if marked {
            self.emit_lane(project, lane);
        }
    }

    /// Drops the `queued` marks of start `start_id` once its services phase is
    /// over: what it spawned shows its own state, what it did not goes back to
    /// idle.
    fn forget_waiting(&self, project: &ProjectRt, lane: &LaneRt, start_id: u64) {
        let forgot = {
            let mut st = lane.st();
            let before = st.waiting.len();
            st.waiting.retain(|_, waiting| waiting.start_id != start_id);
            st.waiting.len() != before
        };
        if forgot {
            self.emit_lane(project, lane);
        }
    }

    /// Records what the lane's bring-up runs now, when `cx` is that bring-up,
    /// so units waiting for it say what they wait for.
    pub(crate) fn note_step(&self, cx: &StartCx, step: Option<&str>) {
        let waiting = {
            let mut st = cx.lane.st();
            let Some(bringup) = st
                .bringup
                .as_mut()
                .filter(|bringup| bringup.start_id == cx.start_id)
            else {
                return;
            };
            bringup.step = step.map(str::to_owned);
            !st.waiting.is_empty()
        };
        if waiting {
            self.emit_lane(&cx.project, &cx.lane);
        }
    }

    /// A fresh `start_id`.
    pub(crate) fn new_start_id(&self) -> u64 {
        self.next_start
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// The units the services phase of `cx` will spawn: every replica of the
    /// selected services that neither runs already nor is a previous daemon's
    /// orphan. Units already up are not part of this start's progress.
    fn planned_units(cx: &StartCx) -> Vec<String> {
        let Some(resolved) = &cx.resolved else {
            return Vec::new();
        };
        let st = cx.lane.st();
        cx.services
            .iter()
            .flat_map(|name| resolved.units_of(name))
            .map(|service| service.unit.clone())
            .filter(|unit| {
                !st.orphans.contains_key(unit)
                    && !st.units.get(unit).is_some_and(|rec| rec.host.is_alive())
                    && cx.ctl.withdrawn_by(unit).is_none()
            })
            .collect()
    }

    /// The other lane and unit currently running a literal `port`.
    fn literal_port_holder(&self, port: u16, except: &Arc<LaneRt>) -> Option<(String, String)> {
        for (project, lane) in self.all_lanes() {
            if Arc::ptr_eq(&lane, except) {
                continue;
            }
            let st = lane.st();
            for (name, rec) in &st.units {
                if rec.literal_port == Some(port) && rec.host.is_alive() {
                    return Some((st.reference(&project.name()), name.clone()));
                }
            }
        }
        None
    }

    /// Running units of a project as launch-check input.
    fn running_planned(&self, project: &ProjectRt) -> Vec<PlannedUnit> {
        let name = project.name();
        project
            .lanes()
            .iter()
            .flat_map(|lane| {
                let st = lane.st();
                let reference = st.reference(&name);
                st.units
                    .iter()
                    .filter(|(_, rec)| rec.host.is_alive())
                    .map(|(unit, rec)| PlannedUnit {
                        label: format!("{reference}:{unit}"),
                        preset: rec.preset.clone(),
                        files: rec.files.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Runs one `infra.setup`/`infra.teardown` entry to completion; with a
    /// `session` (`devlanes run infra.setup.<name>` in the foreground) its
    /// output streams there as `log` events.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_command(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        phase: &str,
        command: &ResolvedCommand,
        actor: &Actor,
        cancel: &CancellationToken,
        session: Option<&Session>,
    ) -> Result<(ExitOutcome, UnitHost), RpcError> {
        let extra = lane.st().setup_env.clone();
        let name = format!("{phase}.{}", command.name);
        let mut request =
            SpawnRequest::oneshot(&name, Kind::Task, &command.launch, false, &extra, actor);
        request.role = if phase == "teardown" {
            "teardown"
        } else {
            "setup"
        };
        let (host, _) = self.spawn_unit(project, lane, request).await?;
        let outcome = match session {
            Some(session) => {
                let follower = stream_logs(host.clone(), session.clone());
                let (exit, ()) = tokio::join!(finish(&host, command.timeout, cancel), follower);
                exit
            }
            None => finish(&host, command.timeout, cancel).await,
        };
        Ok((outcome, host))
    }

    /// Runs a task (or its steps) to completion, recording `task_runs`. When
    /// `cancel` fires, the running unit is stopped and no further step starts;
    /// the outcome then has no exit code. The row is finished and the closing
    /// `task` event emitted on every path, a dropped future included.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_task<'a>(
        self: &'a Arc<Self>,
        project: &'a Arc<ProjectRt>,
        lane: &'a Arc<LaneRt>,
        resolved: &'a ResolvedLane,
        name: &'a str,
        trigger: &'a str,
        actor: &'a Actor,
        session: Option<&'a Session>,
        cancel: &'a CancellationToken,
        depth: usize,
    ) -> BoxFuture<'a, Result<TaskOutcome, RpcError>> {
        Box::pin(async move {
            let reference = lane.st().reference(&project.name());
            if depth > MAX_STEP_DEPTH {
                return Err(RpcError::new(
                    "config_error",
                    json!({"path": format!("tasks.{name}.steps"), "message": "steps nest too deep"}),
                    1,
                ));
            }
            if let Some(error) = resolved.errors.get(&format!("tasks.{name}")) {
                return Err(crate::resolve::resolve_error(error));
            }
            let task = resolved.tasks.get(name).ok_or_else(|| {
                RpcError::new(
                    "not_found",
                    json!({"reference": name, "candidates": resolved.tasks.keys().collect::<Vec<_>>()}),
                    3,
                )
            })?;
            let lane_id = lane
                .st()
                .record
                .as_ref()
                .map(|r| r.id)
                .ok_or_else(|| RpcError::new("unassigned", json!({"lane": reference}), 1))?;
            self.task_event(&reference, name, "started", None, None);
            let hash = loaded_config(lane).and_then(|config| task_hash(&config, name));
            let run = self
                .store
                .start_task_run(TaskRunInput {
                    lane_id,
                    task: name.to_owned(),
                    trigger: trigger.to_owned(),
                    actor: core_actor(actor),
                })
                .await
                .map_err(store_error)?;
            // Every exit path finishes the row: an unfinished task run would block
            // `lane clean` until the next daemon start abandons it. The guard covers
            // a future dropped before the end (a client's limit, a cancelled request).
            let mut guard = TaskRunGuard {
                engine: self.clone(),
                run: Some(run.id),
                reference: reference.clone(),
                task: name.to_owned(),
            };
            let result: Result<TaskOutcome, RpcError> = async {
                let mut outcome = TaskOutcome {
                    code: Some(0),
                    raw_code: Some(0),
                    host: None,
                    leftovers: Vec::new(),
                };
                let mut leftovers = Vec::new();
                match &task.launch {
                    None => {
                        for step in &task.steps {
                            if cancel.is_cancelled() {
                                outcome.code = None;
                                outcome.raw_code = None;
                                break;
                            }
                            let step_outcome = if let Some(entry) =
                                step.strip_prefix("infra.setup.")
                            {
                                self.step_command(
                                    project, lane, resolved, "setup", entry, actor, cancel, None,
                                )
                                .await?
                            } else if let Some(entry) = step.strip_prefix("infra.teardown.") {
                                self.step_command(
                                    project, lane, resolved, "teardown", entry, actor, cancel, None,
                                )
                                .await?
                            } else {
                                self.run_task(
                                    project,
                                    lane,
                                    resolved,
                                    step,
                                    "step",
                                    actor,
                                    session,
                                    cancel,
                                    depth + 1,
                                )
                                .await?
                            };
                            leftovers.extend(step_outcome.leftovers.iter().cloned());
                            if step_outcome.code != Some(0) {
                                outcome = step_outcome;
                                break;
                            }
                        }
                    }
                    Some(launch) => {
                        let extra = lane.st().setup_env.clone();
                        let mut request = SpawnRequest::oneshot(
                            name,
                            Kind::Task,
                            launch,
                            task.pty,
                            &extra,
                            actor,
                        );
                        request.color = Some(task.color.clone());
                        let (host, _) = self.spawn_unit(project, lane, request).await?;
                        let exit = match session {
                            Some(session) => {
                                let follower = stream_logs(host.clone(), session.clone());
                                let (exit, ()) =
                                    tokio::join!(finish(&host, task.timeout, cancel), follower);
                                exit
                            }
                            None => finish(&host, task.timeout, cancel).await,
                        };
                        leftovers.extend(ended_leftovers(lane, name, &host));
                        outcome = TaskOutcome {
                            code: exit.code,
                            raw_code: exit.code,
                            host: Some(host),
                            leftovers: Vec::new(),
                        };
                    }
                }
                outcome.leftovers = leftovers;
                Ok(outcome)
            }
            .await;
            let code = result.as_ref().ok().and_then(|outcome| outcome.code);
            let (state, reason) = match &result {
                Ok(outcome) if outcome.code == Some(0) => ("done", None),
                Ok(_) if cancel.is_cancelled() => ("failed", Some(json!({"code": "cancelled"}))),
                Ok(_) => ("failed", None),
                Err(error) => (
                    "failed",
                    Some(json!({"code": error.code, "data": error.data})),
                ),
            };
            self.close_task_run(run.id, &reference, name, code, state, reason, hash)
                .await;
            guard.run = None;
            result
        })
    }

    /// Finishes a task run row and emits its closing `task` event. `spec_hash` is the
    /// task's [`declaration_hash`], kept with a success so an edit runs it again.
    #[allow(clippy::too_many_arguments)]
    async fn close_task_run(
        &self,
        id: devlanes_state::TaskRunId,
        reference: &str,
        task: &str,
        code: Option<i32>,
        state: &str,
        reason: Option<Value>,
        spec_hash: Option<String>,
    ) {
        if let Err(error) = self.store.finish_task_run(id, code, spec_hash).await {
            tracing::warn!(%error, task, "task run not recorded as finished");
        }
        self.task_event(reference, task, state, code, reason);
    }

    /// `StartedAt` of the infra containers named by setup entries, by name; empty
    /// when the infra spec or the engine is unavailable. `needs` only fills ports,
    /// which the lookup does not read.
    async fn containers_started_at(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        needs: &[InfraPortNeed],
        containers: BTreeSet<String>,
    ) -> BTreeMap<String, String> {
        if containers.is_empty() {
            return BTreeMap::new();
        }
        let Ok(spec) = self.infra_spec(project, lane, needs).await else {
            return BTreeMap::new();
        };
        let containers: Vec<String> = containers.into_iter().collect();
        self.integ
            .infra
            .containers_started_at(&spec, &containers)
            .await
            .unwrap_or_default()
    }

    /// Runs a setup/teardown entry as a task step, or on its own through `run`,
    /// and updates its done-record; `session` streams its output.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn step_command(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        resolved: &ResolvedLane,
        phase: &str,
        entry: &str,
        actor: &Actor,
        cancel: &CancellationToken,
        session: Option<&Session>,
    ) -> Result<TaskOutcome, RpcError> {
        let commands = if phase == "setup" {
            &resolved.setup
        } else {
            &resolved.teardown
        };
        let command = commands.get(entry).ok_or_else(|| {
            RpcError::new(
                "not_found",
                json!({"reference": format!("infra.{phase}.{entry}")}),
                3,
            )
        })?;
        let hash = loaded_config(lane).and_then(|config| setup_hash(&config, entry));
        let (outcome, host) = self
            .run_command(project, lane, phase, command, actor, cancel, session)
            .await?;
        // An interrupted entry did not do its work, whatever `ignore_errors` says.
        let ok = !cancel.is_cancelled() && (outcome.code == Some(0) || command.ignore_errors);
        let lane_id = lane.st().record.as_ref().map(|r| r.id);
        if let Some(lane_id) = lane_id {
            if phase == "setup" && ok {
                // The start's setup phase compares this with the container's
                // `StartedAt`; left empty, the next start would run the entry again.
                let started_at = match &command.container {
                    Some(container) => self
                        .containers_started_at(
                            project,
                            lane,
                            &[],
                            BTreeSet::from([container.clone()]),
                        )
                        .await
                        .remove(container),
                    None => None,
                };
                let _ = self
                    .store
                    .record_setup(lane_id, entry.to_owned(), started_at, hash)
                    .await;
            } else if phase == "teardown" && ok {
                let _ = self
                    .store
                    .clear_setup(lane_id, Some(entry.to_owned()))
                    .await;
            }
        }
        let leftovers = ended_leftovers(lane, &format!("{phase}.{entry}"), &host)
            .into_iter()
            .collect();
        Ok(TaskOutcome {
            code: if ok { Some(0) } else { outcome.code },
            raw_code: outcome.code,
            host: Some(host),
            leftovers,
        })
    }

    /// Brings a lane up (decision 5), or adds units to a lane that runs or is
    /// being brought up. Idempotent: running units stay untouched.
    ///
    /// A start that finds nothing running brings the lane up: it holds `op`
    /// exclusively through infra, setup and tasks, then shares it for its
    /// services. Any other start shares `op` from the beginning, so starts of
    /// single units never wait for each other, only for a bring-up's earlier
    /// phases or for a whole-lane stop, clean or reset.
    pub(crate) async fn start_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        mut options: StartOptions,
    ) -> Result<StartResult, RpcError> {
        let start_id = options.start_id.unwrap_or_else(|| self.new_start_id());
        options.start_id = Some(start_id);
        // Installed before the wait for `op`, until the start answered: a
        // whole-lane stop fires its token, a single-unit stop withdraws units
        // through it without waiting.
        let slot = lane.begin_start(&self.shutdown, start_id);
        // Before anything that can wait, so the units read `queued` the moment
        // the request arrives.
        self.mark_waiting(project, lane, &options, start_id);
        let result = self.run_start(project, lane, options, slot).await;
        // Whatever the start did not get to goes back to its own state.
        self.forget_waiting(project, lane, start_id);
        result
    }

    /// The body of [`Self::start_lane`], from the wait for `op` to the answer.
    async fn run_start(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        options: StartOptions,
        slot: crate::model::StartSlot,
    ) -> Result<StartResult, RpcError> {
        use tracing::Instrument as _;
        let start_id = slot.ctl.start_id();
        let cancel = slot.ctl.cancel.clone();
        let began = Instant::now();
        let lease = self
            .lane_lease(lane, start_id, &cancel)
            .instrument(tracing::debug_span!("lane lock"))
            .await;
        let mut timings = StartTimings {
            began,
            lock: began.elapsed(),
            phases: Vec::new(),
        };
        let Some(mut lease) = lease else {
            let reference = lane.st().reference(&project.name());
            return Err(with_ownership(
                RpcError::new("start_cancelled", json!({"lane": reference}), 1),
                &reference,
                &[],
            ));
        };
        let whole_lane = lease.brings_up();
        // Ends the bring-up mark however this start returns.
        let bringup = whole_lane.then(|| BringupMark {
            lane: lane.clone(),
            start_id,
        });
        // Counted from here, so other starts and refreshes leave the block and
        // index this one plans on alone; ends however the start returns.
        lane.st().starting += 1;
        let starting = StartingMark { lane: lane.clone() };
        // Units already running are left alone, so a frozen lane would stay frozen.
        self.wake_lane(project, lane, Some(Activity::Command))
            .instrument(tracing::debug_span!("wake"))
            .await;
        let initial = lane.st().reference(&project.name());
        let fail = |error: RpcError| with_ownership(error, &initial, &[]);
        {
            let st = lane.st();
            if let Some(reason) = &st.unassigned {
                return Err(fail(reason.clone()));
            }
            match &st.config {
                crate::model::ConfigState::Invalid(error) => return Err(fail(error.clone())),
                crate::model::ConfigState::Elsewhere(other) => {
                    return Err(fail(crate::model::elsewhere_error(other, Some(&initial))));
                }
                _ => {}
            }
            if st.record.is_none() {
                return Err(fail(RpcError::new(
                    "unassigned",
                    json!({"lane": initial}),
                    1,
                )));
            }
            if !st.present {
                return Err(fail(RpcError::new(
                    "worktree_missing",
                    json!({"path": st.path}),
                    3,
                )));
            }
        }
        {
            let mut st = lane.st();
            if let Some(env) = options.env.clone() {
                st.env = Some(env);
            }
            if st.env.is_none() {
                return Err(fail(RpcError::new(
                    "needs_env",
                    json!({"lane": initial}),
                    1,
                )));
            }
            st.overrides = options.overrides.clone();
        }
        if let Some(env) = options.env.clone() {
            project.st().last_env = Some(env);
        }
        self.apply_move(project, lane)
            .instrument(tracing::debug_span!("apply move"))
            .await
            .map_err(fail)?;
        // A start that adds units resolves against the layout config reloads keep
        // current; it refreshes only for a reload that waited for the lane.
        if whole_lane || lane.st().refresh_pending {
            self.refresh_views_held(project, lane).await;
        }
        let reference = lane.st().reference(&project.name());
        let fail = |error: RpcError| with_ownership(error, &reference, &[]);
        let config = lane.st().config.require(&reference).map_err(fail)?;
        let (relative, _) = project.config_path();
        let config_dir = lane.st().path.join(relative);
        check_compose_files(&config, config_dir)
            .instrument(tracing::debug_span!("compose files"))
            .await
            .map_err(fail)?;
        let (saved, lane_profiles) = {
            let st = lane.st();
            (st.record.clone(), st.profiles.clone())
        };
        let (selection, dropped) = start_selection(
            &config,
            saved.as_ref(),
            &lane_profiles,
            &options,
            whole_lane,
        )
        .map_err(|error| fail(crate::resolve::resolve_error(&error)))?;
        // Kept only once valid, so a typo does not stick to the lane.
        lane.st().profiles = selection.profiles.clone();
        // From here the lane counts as running for box lanes that clash with
        // it, whatever it starts, so it takes their hostnames and ports and a
        // choice of a box for them is cleared. Not before: a start its checks
        // refuse takes nothing from a box.
        let _claim = self
            .claim_ports(lane)
            .instrument(tracing::debug_span!("claim ports"))
            .await;
        let infra_enabled = !matches!(
            selection
                .profile_options
                .as_ref()
                .and_then(|o| o.infra.as_ref()),
            Some(Templated::Literal(false))
        );
        let infra_templated = selection.profiles.iter().any(|name| {
            matches!(
                config.profiles.get(name),
                Some(devlanes_config::ProfileConfig::Object(options))
                    if matches!(options.infra, Some(Templated::Expression(_)))
            )
        });
        {
            let mut st = lane.st();
            st.busy = Some("starting");
            st.last_error = None;
            if whole_lane {
                st.started_at_ms = Some(now_ms());
                st.usage.clear();
                st.config_changed = false;
                st.drift.clear();
                st.drift_error = None;
                st.drift_generation += 1;
                st.infra_changed = false;
                st.started_with =
                    Some(crate::drift::StartedWith::new(&config, &selection.profiles));
                st.skipped_tasks.clear();
            }
        }
        self.emit_lane(project, lane);
        let mut cx = StartCx {
            project: project.clone(),
            lane: lane.clone(),
            reference: reference.clone(),
            actor: options.actor.clone(),
            services: selection.services.clone(),
            autostart_pending: selection.autostart_pending.clone(),
            profiles: selection.profiles.clone(),
            infra_enabled,
            infra_templated,
            whole_lane,
            setup_only: options.setup_only,
            session: options.session.clone(),
            resolved: None,
            deadline: Instant::now() + options.timeout,
            started: Vec::new(),
            already_running: Vec::new(),
            failures: Vec::new(),
            skipped_tasks: Vec::new(),
            leftovers: Vec::new(),
            infra_needs: Vec::new(),
            timed_out: false,
            all_at_once: options.all_at_once,
            start_id,
            cancel,
            ctl: slot.ctl.clone(),
            config: config.clone(),
            arrival: options.arrival,
            dropped,
        };
        tracing::debug!(
            lane = %reference,
            whole_lane,
            profiles = %cx.profiles.join(","),
            services = %cx.services.join(","),
            infra_enabled,
            timeout_ms = options.timeout.as_millis() as u64,
            start_id = cx.start_id,
            "start planned"
        );
        let mut hard_error = None;
        let mut failed_phase: Option<(&'static str, RpcError)> = None;
        for phase in start_pipeline() {
            if cx.cancel.is_cancelled() {
                let error = start_cancelled(&cx);
                failed_phase = Some((phase.name(), error.clone()));
                hard_error = Some(error);
                break;
            }
            if phase.name() == "services" {
                // From here other starts add their units alongside this one's.
                lease = lease.share();
            }
            if phase.visible() {
                // Resolution ran before services, so the plan is known by now.
                let units = (phase.name() == "services" && !cx.setup_only)
                    .then(|| Self::planned_units(&cx));
                self.phase_event(&cx, phase.name(), "started", None, units);
                self.note_step(&cx, bringup_step(phase.name()));
            }
            // Reaching services is what makes earlier phase failures stale.
            if phase.name() == "services" && !cx.setup_only {
                lane.st().phase_reasons.clear();
            }
            let span = tracing::debug_span!("phase", name = phase.name(), lane = %cx.reference);
            let phase_began = Instant::now();
            let mut result = tracing::Instrument::instrument(phase.run(self, &mut cx), span).await;
            timings.phases.push((phase.name(), phase_began.elapsed()));
            // Whatever the phase made of it, an interrupted start ends here.
            if cx.cancel.is_cancelled() && !matches!(result, PhaseResult::Error(_)) {
                result = PhaseResult::Error(start_cancelled(&cx));
            }
            if phase.name() == "services" {
                // Nothing more spawns: a waiting restart of a withdrawn unit may go.
                cx.ctl.services_done.cancel();
                self.forget_waiting(project, lane, cx.start_id);
            }
            let (state, why) = result.event_state();
            if phase.visible() {
                self.phase_event(&cx, phase.name(), state, why, None);
            }
            match result {
                PhaseResult::Done | PhaseResult::Skipped(_) => {}
                PhaseResult::Halt(why) => {
                    failed_phase = Some((phase.name(), why));
                    break;
                }
                PhaseResult::Error(error) => {
                    failed_phase = Some((phase.name(), error.clone()));
                    hard_error = Some(error);
                    break;
                }
            }
        }
        // A start that arrives from now on finds the lane up, or brings it up
        // again when this one failed before anything ran.
        drop(bringup);
        // Before the failure is recorded, so the units show it instead of `queued`.
        self.forget_waiting(project, lane, start_id);
        if let Some((phase, error)) = &failed_phase
            && !cx.setup_only
        {
            self.mark_unstarted(project, lane, &cx, phase, error);
        }
        let desired_guard = lane.desired.lock().await;
        let record = lane.st().record.clone();
        if let Some(record) = record
            && !options.transient
        {
            let (desired, services) = if options.setup_only {
                (record.desired_state, record.services.clone())
            } else {
                // Only a start of the whole selection (restored, or the profiles
                // given) rebuilds the set; named units add to it, since what
                // they leave out was never stopped.
                let kept = if whole_lane && options.selectors.is_empty() {
                    &[][..]
                } else {
                    &record.services[..]
                };
                (
                    DesiredState::Running,
                    remembered_services(kept, &cx.services, &cx.ctl),
                )
            };
            self.save_desired(lane, &record, desired, selection.profiles.clone(), services)
                .await;
        }
        drop(desired_guard);
        persist("project start time", || {
            self.store.mark_project_started(project.id)
        })
        .await;
        // Only a bring-up runs tasks; a start that adds units must not erase
        // what the bring-up skipped.
        if whole_lane {
            lane.st().skipped_tasks = cx.skipped_tasks.clone();
        }
        drop(starting);
        drop(lease);
        let started: Vec<String> = cx
            .started
            .iter()
            .map(|u| format!("{reference}:{u}"))
            .collect();
        if let Some(error) = hard_error {
            lane.st().last_error = Some(error.clone());
            self.emit_lane(project, lane);
            timings.log(&reference, start_id, whole_lane, &slot.ctl, &error.code);
            return Err(with_ownership(error, &reference, &started));
        }
        let result = self.finish_start(cx, whole_lane).await;
        self.emit_lane(project, lane);
        let outcome = if result.ready {
            "ready"
        } else if result.timed_out {
            "timed_out"
        } else {
            "failed"
        };
        timings.log(&reference, start_id, whole_lane, &slot.ctl, outcome);
        // Answered: stops no longer reach this start.
        drop(slot);
        Ok(result)
    }

    /// Waits for started units and assembles the [`StartResult`]. Units a stop or
    /// restart withdrew are neither waited for nor failures: they are listed as
    /// `stopped` with a `cancelled_by_stop` reason.
    async fn finish_start(self: &Arc<Self>, mut cx: StartCx, whole_lane: bool) -> StartResult {
        let kept = |unit: &String| cx.ctl.withdrawn_by(unit).is_none();
        let selected: Vec<String> = cx
            .started
            .iter()
            .chain(&cx.already_running)
            .filter(|unit| kept(unit))
            .cloned()
            .collect();
        let hosts: Vec<UnitHost> = {
            let st = cx.lane.st();
            selected
                .iter()
                .filter_map(|unit| st.units.get(unit).map(|rec| rec.host.clone()))
                .collect()
        };
        if !wait_settled(&hosts, cx.deadline).await {
            cx.timed_out = true;
        }
        // `ready: started` calls a unit ready at once; one that exits right after
        // its spawn (a bad flag, a missing file) failed to start all the same.
        let fresh: Vec<UnitHost> = {
            let st = cx.lane.st();
            cx.started
                .iter()
                .filter_map(|unit| st.units.get(unit).map(|rec| rec.host.clone()))
                .collect()
        };
        settle_early_exits(&fresh, cx.deadline).await;
        // Withdrawals may have come in while waiting.
        let withdrawn = cx.ctl.withdrawn();
        let selected: Vec<String> = selected
            .into_iter()
            .filter(|unit| !withdrawn.contains_key(unit))
            .collect();
        let mut units: Vec<UnitSummary> = Vec::new();
        let mut failed_hosts = Vec::new();
        {
            let st = cx.lane.st();
            for unit in &selected {
                if let Some(rec) = st.units.get(unit) {
                    units.push(unit_summary(&cx.reference, unit, rec));
                    if matches!(rec.host.state(), UnitState::Failed(_))
                        || (!rec.host.is_alive() && !rec.host.stop_was_requested())
                    {
                        failed_hosts.push((unit.clone(), rec.host.clone()));
                    }
                }
            }
        }
        for (unit, host) in failed_hosts {
            if !cx.failures.iter().any(|f| f.unit == unit) {
                cx.failures
                    .push(failure_of(&cx.reference, &unit, &host).await);
            }
        }
        let all_ready = units.iter().all(|u| u.state == "ready");
        if !withdrawn.is_empty() {
            units.extend(self.withdrawn_summaries(&cx, &withdrawn));
            cx.started.retain(|unit| !withdrawn.contains_key(unit));
            cx.already_running
                .retain(|unit| !withdrawn.contains_key(unit));
            cx.failures
                .retain(|failure| !withdrawn.contains_key(&failure.unit));
        }
        let branch = cx.lane.st().branch.clone();
        let mut warnings = cx.dropped.warnings(&cx.reference);
        warnings.extend(self.ca_warnings(&cx.reference, &cx.lane.st()));
        let path = cx.lane.st().path.clone();
        warnings.extend(self.windows_drive_warning(&cx.reference, &path).await);
        for warning in &warnings {
            self.bus.emit(
                events::LANE_WARNING,
                json!({"lane": cx.reference, "code": warning.code, "data": warning.data}),
            );
        }
        // What it spawned matches the file, unless the file changed during the start.
        self.settle_drift(&cx.project, &cx.lane, &cx.config, &cx.started);
        StartResult {
            warnings,
            started: cx
                .started
                .iter()
                .map(|u| format!("{}:{u}", cx.reference))
                .collect(),
            already_running: cx
                .already_running
                .iter()
                .map(|u| format!("{}:{u}", cx.reference))
                .collect(),
            lane: cx.reference,
            branch,
            whole_lane,
            ready: cx.failures.is_empty() && !cx.timed_out && all_ready,
            timed_out: cx.timed_out,
            failures: cx.failures,
            skipped_tasks: cx.skipped_tasks,
            leftovers: cx.leftovers,
            units,
            start_id: Some(cx.start_id),
        }
    }

    /// `stopped` summaries of the units of `cx` that a stop or restart withdrew.
    fn withdrawn_summaries(
        &self,
        cx: &StartCx,
        withdrawn: &BTreeMap<String, &'static str>,
    ) -> Vec<UnitSummary> {
        let mut ours: BTreeSet<String> = cx
            .started
            .iter()
            .chain(&cx.already_running)
            .cloned()
            .collect();
        if let Some(resolved) = &cx.resolved {
            for name in &cx.services {
                ours.extend(resolved.units_of(name).map(|service| service.unit.clone()));
            }
        }
        let st = cx.lane.st();
        withdrawn
            .iter()
            .filter(|(unit, _)| ours.contains(*unit))
            .map(|(unit, by)| {
                let service = st
                    .view
                    .as_ref()
                    .and_then(|view| view.services.iter().find(|s| &s.unit == unit));
                let mut summary = match (st.units.get(unit), service) {
                    (Some(rec), _) => unit_summary(&cx.reference, unit, rec),
                    (None, Some(service)) => crate::model::idle_summary(&cx.reference, service),
                    (None, None) => UnitSummary {
                        name: unit.clone(),
                        lane: cx.reference.clone(),
                        ..UnitSummary::default()
                    },
                };
                summary.state = "stopped".into();
                summary.reason = Some(withdrawn_reason(&cx.reference, unit, by));
                summary
            })
            .collect()
    }

    /// Stops units of a lane. Without selectors (and without `mine`) the whole lane
    /// stops after its `lane.stop` tasks. `transient` (`devlanes with`) leaves what
    /// the lane saved for its next plain start alone. Returns the stopped units.
    pub(crate) async fn stop_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        selectors: Option<Vec<String>>,
        mine: Option<&Actor>,
        transient: bool,
    ) -> Result<Vec<UnitSummary>, RpcError> {
        // Before the lock: a start holding it may be waiting on these queued units,
        // or on a task that never ends (a whole-lane stop interrupts that start).
        let queued_reference = lane.st().reference(&project.name());
        // Interrupted starts and tasks are otherwise hard to explain after the fact.
        tracing::info!(
            lane = %queued_reference,
            units = %selectors.as_deref().map(|s| s.join(",")).unwrap_or_default(),
            mine = mine.is_some(),
            interrupts_start = !lane.running_starts().is_empty(),
            "stop requested"
        );
        self.cancel_queued(lane, &queued_reference, selectors.as_deref(), mine);
        let named = selectors.as_ref().is_some_and(|s| !s.is_empty());
        if !named {
            // A whole-lane stop interrupts the start; `--mine` alone waits for it.
            if mine.is_none() {
                lane.cancel_start();
            }
            let _op = lane.op.write().await;
            return self
                .stop_locked(project, lane, selectors, mine, transient)
                .await;
        }
        // Single units never wait for a start: they leave it through its control.
        // `op` held by anything else (clean, a whole stop) is waited for, with a
        // look at the lane now and then for a start that took it meanwhile.
        loop {
            let starts = lane.running_starts();
            if !starts.is_empty() {
                return self
                    .stop_in_start(project, lane, &starts, selectors, mine, "stop", transient)
                    .await;
            }
            tokio::select! {
                op = lane.op.write() => {
                    let _op = op;
                    return self.stop_locked(project, lane, selectors, mine, transient).await;
                }
                () = tokio::time::sleep(START_POLL) => {}
            }
        }
    }

    /// Stops single units of a lane whose starts (or restarts) are running,
    /// without their `op`: each start skips units it has not spawned, stops
    /// waiting for the rest and reports them as stopped. Naming a task a start
    /// runs right now interrupts that start, as a whole-lane stop does. `by` is
    /// `stop` or `restart`; only a `stop` that is not `transient` takes the
    /// units out of what a plain start brings back.
    #[allow(clippy::too_many_arguments)]
    async fn stop_in_start(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        starts: &[Arc<StartCtl>],
        selectors: Option<Vec<String>>,
        mine: Option<&Actor>,
        by: &'static str,
        transient: bool,
    ) -> Result<Vec<UnitSummary>, RpcError> {
        let transient = transient || by != "stop";
        let reference = lane.st().reference(&project.name());
        // Tasks the starts run or plan count even before the lane records their
        // unit: the runner may have spawned one that is not registered yet.
        let tasks: BTreeSet<String> = starts.iter().flat_map(|start| start.task_units()).collect();
        let mut names = Self::matching_units(lane, &reference, selectors.as_deref(), &tasks)?;
        if let Some(caller) = mine {
            // Only the caller's running units: the start's own were not theirs yet.
            let st = lane.st();
            names.retain(|name| {
                st.units.get(name).is_some_and(|rec| {
                    rec.host.is_alive() && is_mine(&rec.host.info().actor, caller)
                })
            });
        }
        for start in starts {
            start.withdraw(&names, by);
        }
        // Units waiting for their turn read idle again at once.
        {
            let mut st = lane.st();
            for name in &names {
                st.waiting.remove(name);
            }
        }
        // Again: one may have joined the queue before the withdrawal was visible.
        self.cancel_queued(lane, &reference, selectors.as_deref(), mine);
        for start in starts {
            if names.iter().any(|name| start.runs_task(name)) {
                start.cancel.cancel();
            }
        }
        // A spawn past its last withdrawal check is recorded before the targets
        // are picked; every later one sees the withdrawal.
        lane.spawns_settled().await;
        let mut stopped = self
            .stop_named(
                project, lane, &reference, &names, mine, false, transient, false,
            )
            .await;
        // Units the start had not spawned yet are stopped too, as far as it goes.
        let st = lane.st();
        for name in &names {
            if stopped.iter().any(|summary| &summary.name == name) {
                continue;
            }
            if st.units.get(name).is_some_and(|rec| rec.host.is_alive()) {
                continue;
            }
            let service = st
                .view
                .as_ref()
                .and_then(|view| view.services.iter().find(|s| &s.unit == name));
            let mut summary = match (service, st.units.get(name)) {
                (Some(service), _) => crate::model::idle_summary(&reference, service),
                // A task of the start that has already ended: the start's own
                // cancellation may have stopped it before this stop got to it.
                (None, Some(rec)) if tasks.contains(name) => unit_summary(&reference, name, rec),
                // A task the start had not run yet; it skips it now.
                (None, None) if tasks.contains(name) => UnitSummary {
                    name: name.clone(),
                    lane: reference.clone(),
                    kind: "task".into(),
                    ..UnitSummary::default()
                },
                _ => continue,
            };
            summary.state = "stopped".into();
            summary.reason = Some(withdrawn_reason(&reference, name, by));
            stopped.push(summary);
        }
        Ok(stopped)
    }

    /// [`Inner::stop_lane`] for a caller that holds `op` and already withdrew the
    /// queued starts it covers.
    async fn stop_locked(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        selectors: Option<Vec<String>>,
        mine: Option<&Actor>,
        transient: bool,
    ) -> Result<Vec<UnitSummary>, RpcError> {
        // Under `op`, so no freeze can begin after it. Stop signals and
        // `lane.stop` tasks expect running processes, and paused containers
        // would stay paused after the lane stopped.
        self.wake_lane(project, lane, Some(Activity::Stop)).await;
        // A unit between its last cancellation check and its record would be
        // missed by the target list below.
        lane.spawns_settled().await;
        let reference = lane.st().reference(&project.name());
        let whole = selectors.as_ref().is_none_or(Vec::is_empty) && mine.is_none();
        let runnable = hooks_runnable(&lane.st());
        if whole && runnable {
            // Daemon shutdown interrupts a hook that never ends; it waits for this lane.
            let cancel = self.shutdown.child_token();
            self.run_hook(project, lane, TaskWhen::Stop, &Actor::default(), &cancel)
                .await;
        }
        let names = Self::matching_units(lane, &reference, selectors.as_deref(), &BTreeSet::new())?;
        Ok(self
            .stop_named(
                project, lane, &reference, &names, mine, whole, transient, true,
            )
            .await)
    }

    /// The lane's units (running, configured or orphaned, plus `extra`) that
    /// `selectors` match; every one without selectors.
    fn matching_units(
        lane: &LaneRt,
        reference: &str,
        selectors: Option<&[String]>,
        extra: &BTreeSet<String>,
    ) -> Result<Vec<String>, RpcError> {
        let st = lane.st();
        let mut candidates: Vec<(String, Vec<String>)> = st
            .units
            .iter()
            .map(|(name, rec)| (name.clone(), rec.tags.clone()))
            .collect();
        for name in extra {
            if !st.units.contains_key(name) {
                candidates.push((name.clone(), Vec::new()));
            }
        }
        if let Some(view) = &st.view {
            for service in &view.services {
                if !candidates.iter().any(|(n, _)| *n == service.unit) {
                    candidates.push((service.unit.clone(), service.tags.clone()));
                }
            }
        }
        for name in st.orphans.keys() {
            if !candidates.iter().any(|(n, _)| n == name) {
                candidates.push((name.clone(), Vec::new()));
            }
        }
        match selectors.filter(|s| !s.is_empty()) {
            Some(selectors) => crate::engine::match_units(selectors, &candidates, reference),
            None => Ok(candidates.into_iter().map(|(name, _)| name).collect()),
        }
    }

    /// Stops the live units and orphans among `names` (for `mine`, the caller's
    /// only) and returns them. `whole` settles crashed services and records the
    /// lane as stopped, forgetting what a plain start would bring back; otherwise
    /// the stopped services leave that set. `transient` saves nothing.
    /// `owns_lane` (the caller holds `op`) shows the lane `stopping` meanwhile,
    /// which a running start's `starting` must not be replaced by.
    #[allow(clippy::too_many_arguments)]
    async fn stop_named(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        reference: &str,
        names: &[String],
        mine: Option<&Actor>,
        whole: bool,
        transient: bool,
        owns_lane: bool,
    ) -> Vec<UnitSummary> {
        let (targets, orphans): (Vec<String>, Vec<String>) = {
            let st = lane.st();
            let names = names.to_vec();
            let orphans = names
                .iter()
                .filter(|name| {
                    st.orphans
                        .get(*name)
                        .is_some_and(|o| mine.is_none_or(|caller| is_mine(&o.actor, caller)))
                })
                .cloned()
                .collect();
            let targets = names
                .into_iter()
                .filter(|name| {
                    st.units.get(name).is_some_and(|rec| {
                        rec.host.is_alive()
                            && mine.is_none_or(|caller| is_mine(&rec.host.info().actor, caller))
                    })
                })
                .collect();
            (targets, orphans)
        };
        if owns_lane && (!targets.is_empty() || !orphans.is_empty()) {
            lane.st().busy = Some("stopping");
            self.emit_lane(project, lane);
        }
        let stops: Vec<(&String, UnitHost, StopPolicy)> = {
            let st = lane.st();
            targets
                .iter()
                .filter_map(|name| {
                    st.units
                        .get(name)
                        .map(|rec| (name, rec.host.clone(), rec.stop))
                })
                .collect()
        };
        let supervised = join_all(stops.iter().map(|(name, host, policy)| async move {
            if let Err(error) = host.stop(*policy).await {
                tracing::warn!(code = %error.code, "unit stop failed");
            }
            // The unit's watcher stops its syncs too, but only after this answer:
            // a stopped service must not have its folder mirrored any more.
            if !host.is_alive() {
                crate::local_sync::stop(lane, name, host).await;
            }
        }));
        let (_, terminated) =
            tokio::join!(supervised, self.terminate_orphans(project, lane, &orphans));
        if whole {
            // An explicit full stop settles crashed services: the lane reads idle.
            // Failed tasks keep their state, since a stop hook may just have failed.
            let st = lane.st();
            for rec in st.units.values().filter(|rec| rec.kind == Kind::Service) {
                rec.host.acknowledge_failure();
            }
        }
        let desired = lane.desired.lock().await;
        let record = lane.st().record.clone();
        match record {
            // The profiles of a short-lived start go with it.
            Some(record) if transient => {
                if whole {
                    lane.st().profiles = record.profiles.clone();
                }
            }
            // A lane stopped by hand starts from its default next time.
            Some(record) if whole => {
                lane.st().profiles = Vec::new();
                self.save_desired(lane, &record, DesiredState::Stopped, Vec::new(), Vec::new())
                    .await;
            }
            Some(record) => {
                let touched: Vec<String> = if mine.is_some() {
                    targets.iter().chain(&orphans).cloned().collect()
                } else {
                    names.to_vec()
                };
                let services = without_stopped(lane, &record.services, &touched);
                if services != record.services {
                    self.save_desired(
                        lane,
                        &record,
                        record.desired_state,
                        record.profiles.clone(),
                        services,
                    )
                    .await;
                }
            }
            None => {}
        }
        drop(desired);
        if owns_lane {
            lane.st().busy = None;
        }
        self.emit_lane(project, lane);
        let st = lane.st();
        let mut stopped: Vec<UnitSummary> = targets
            .iter()
            .filter_map(|name| {
                st.units
                    .get(name)
                    .map(|rec| unit_summary(reference, name, rec))
            })
            .collect();
        stopped.extend(terminated);
        stopped
    }

    /// Runs `lane.stop` or `lane.clean` tasks in file order until `cancel`
    /// fires; failures are reported only.
    async fn run_hook(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        hook: TaskWhen,
        actor: &Actor,
        cancel: &CancellationToken,
    ) -> Vec<Value> {
        let Ok(resolved) = self.resolve_full(project, lane).await else {
            return Vec::new();
        };
        self.run_resolved_hook(project, lane, &resolved, hook, actor, cancel)
            .await
    }

    /// [`Self::run_hook`] with the lane already resolved.
    async fn run_resolved_hook(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        resolved: &ResolvedLane,
        hook: TaskWhen,
        actor: &Actor,
        cancel: &CancellationToken,
    ) -> Vec<Value> {
        let trigger = match hook {
            TaskWhen::Stop => "lane.stop",
            TaskWhen::Clean => "lane.clean",
            TaskWhen::Create => "lane.create",
            TaskWhen::Start => "lane.start",
        };
        let mut results = Vec::new();
        for (name, task) in &resolved.tasks {
            if task.when != Some(hook) || !task.active {
                continue;
            }
            // An interrupted hook starts nothing more; the rest read as cancelled.
            if cancel.is_cancelled() {
                results.push(json!({"task": name, "error": {"code": "cancelled", "data": null}}));
                continue;
            }
            let outcome = self
                .run_task(
                    project, lane, resolved, name, trigger, actor, None, cancel, 0,
                )
                .await;
            results.push(match outcome {
                Ok(outcome) => json!({"task": name, "exit_code": outcome.code}),
                Err(error) => {
                    json!({"task": name, "error": {"code": error.code, "data": error.data}})
                }
            });
        }
        results
    }

    /// Restarts selected services: stop, wait for their ports, spawn with fresh plans.
    ///
    /// With starts of the lane running, the units leave them first (as with
    /// `stop`), and the restart spawns them once those starts' services phases
    /// are over. It shares `op` with starts that add units, so it waits only for
    /// a bring-up's earlier phases or a whole-lane stop, clean or reset.
    pub(crate) async fn restart_units(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        selectors: &[String],
        options: StartOptions,
    ) -> Result<StartResult, RpcError> {
        let starts = lane.running_starts();
        if !starts.is_empty() {
            self.stop_in_start(
                project,
                lane,
                &starts,
                Some(selectors.to_vec()),
                None,
                "restart",
                false,
            )
            .await?;
            for start in &starts {
                start.services_done.cancelled().await;
            }
        }
        let start_id = options.start_id.unwrap_or_else(|| self.new_start_id());
        let slot = lane.begin_start(&self.shutdown, start_id);
        let ctl = slot.ctl.clone();
        let op = tokio::select! {
            op = lane.op.read() => op,
            () = ctl.cancel.cancelled() => {
                let reference = lane.st().reference(&project.name());
                return Err(with_ownership(
                    RpcError::new("start_cancelled", json!({"lane": reference}), 1),
                    &reference,
                    &[],
                ));
            }
        };
        // With `op` held no freeze can begin after this.
        self.wake_lane(project, lane, Some(Activity::Stop)).await;
        let reference = lane.st().reference(&project.name());
        if let Some(env) = options.env.clone() {
            lane.st().env = Some(env.clone());
            project.st().last_env = Some(env);
        }
        if !options.overrides.is_empty() {
            lane.st().overrides = options.overrides.clone();
        }
        if !options.profiles.is_empty() {
            let config = lane.st().config.loaded().cloned();
            if let Some(config) = config {
                let selection = devlanes_resolve::select_profiles(&config, &options.profiles, &[])
                    .map_err(|error| {
                        with_ownership(crate::resolve::resolve_error(&error), &reference, &[])
                    })?;
                lane.st().profiles = selection.profiles.clone();
                let _desired = lane.desired.lock().await;
                let record = lane.st().record.clone();
                if let Some(record) = record {
                    self.save_desired(
                        lane,
                        &record,
                        record.desired_state,
                        selection.profiles.clone(),
                        record.services.clone(),
                    )
                    .await;
                }
            }
        }
        let candidates: Vec<(String, Vec<String>)> = {
            let st = lane.st();
            st.view
                .as_ref()
                .map(|view| {
                    view.services
                        .iter()
                        .map(|s| (s.unit.clone(), s.tags.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let units = crate::engine::match_units(selectors, &candidates, &reference)?;
        let config = lane
            .st()
            .config
            .require(&reference)
            .map_err(|error| with_ownership(error, &reference, &[]))?;
        let resolved = self
            .resolve_config(project, lane, config.clone(), None)
            .await
            .map_err(|e| with_ownership(e, &reference, &[]))?;
        // A previous daemon's process is replaced like a running one: terminate it
        // first, so its ports are free for the new run.
        let orphans = self.terminate_orphans(project, lane, &units).await;
        for orphan in &orphans {
            for port in orphan.ports.values() {
                if !wait_port_free(*port, PORT_RELEASE).await {
                    let pid = self.integ.ports.holder(*port).await;
                    return Err(with_ownership(
                        RpcError::new(
                            "port_in_use",
                            json!({"port": port, "pid": pid, "unit": orphan.name}),
                            1,
                        ),
                        &reference,
                        &[],
                    ));
                }
            }
        }
        let old: Vec<(String, UnitHost, StopPolicy, Vec<u16>)> = {
            let st = lane.st();
            units
                .iter()
                .filter_map(|unit| {
                    st.units
                        .get(unit)
                        .filter(|rec| rec.host.is_alive())
                        .map(|rec| {
                            (
                                unit.clone(),
                                rec.host.clone(),
                                rec.stop,
                                rec.ports.values().copied().collect(),
                            )
                        })
                })
                .collect()
        };
        join_all(old.iter().map(|(_, host, policy, _)| host.stop(*policy))).await;
        for (unit, _, _, ports) in &old {
            for port in ports {
                if !wait_port_free(*port, PORT_RELEASE).await {
                    let pid = self.integ.ports.holder(*port).await;
                    return Err(with_ownership(
                        RpcError::new(
                            "port_in_use",
                            json!({"port": port, "pid": pid, "unit": unit}),
                            1,
                        ),
                        &reference,
                        &[],
                    ));
                }
            }
        }
        let mut services: Vec<String> = Vec::new();
        for unit in &units {
            if let Some(service) = resolved.services.get(unit)
                && !services.contains(&service.name)
            {
                services.push(service.name.clone());
            }
        }
        let mut cx = StartCx {
            project: project.clone(),
            lane: lane.clone(),
            reference: reference.clone(),
            actor: options.actor.clone(),
            services,
            autostart_pending: Vec::new(),
            profiles: lane.st().profiles.clone(),
            infra_enabled: true,
            infra_templated: false,
            whole_lane: false,
            setup_only: false,
            session: options.session.clone(),
            resolved: Some(resolved),
            deadline: Instant::now() + options.timeout,
            started: Vec::new(),
            already_running: Vec::new(),
            failures: Vec::new(),
            skipped_tasks: Vec::new(),
            leftovers: Vec::new(),
            infra_needs: Vec::new(),
            timed_out: false,
            all_at_once: options.all_at_once,
            start_id,
            cancel: ctl.cancel.clone(),
            ctl,
            config,
            arrival: options.arrival,
            dropped: Dropped::default(),
        };
        // A restart is a services phase alone; its events carry the plan as a start's do.
        let phase = ServicesPhase.name();
        self.phase_event(&cx, phase, "started", None, Some(Self::planned_units(&cx)));
        // A whole-lane stop, clean or reset came first: nothing is spawned.
        let result = if cx.cancel.is_cancelled() {
            PhaseResult::Error(start_cancelled(&cx))
        } else {
            ServicesPhase.run(self, &mut cx).await
        };
        cx.ctl.services_done.cancel();
        let (state, why) = result.event_state();
        self.phase_event(&cx, phase, state, why, None);
        // Replicas not named by the selectors stay as they are.
        cx.already_running.retain(|unit| units.contains(unit));
        // A restarted service runs again, so a plain start after a crash brings it back.
        let desired = lane.desired.lock().await;
        let record = lane.st().record.clone();
        if let Some(record) = record {
            let services = remembered_services(&record.services, &cx.services, &cx.ctl);
            if services != record.services {
                self.save_desired(
                    lane,
                    &record,
                    DesiredState::Running,
                    record.profiles.clone(),
                    services,
                )
                .await;
            }
        }
        drop(desired);
        drop(op);
        if let PhaseResult::Error(error) = result {
            let started: Vec<String> = cx
                .started
                .iter()
                .map(|u| format!("{reference}:{u}"))
                .collect();
            return Err(with_ownership(error, &reference, &started));
        }
        let result = self.finish_start(cx, false).await;
        drop(slot);
        Ok(result)
    }

    /// `lane clean`: stop, `lane.clean` tasks, teardown, remove per-lane containers,
    /// free the block and records, and with [`Logs::Delete`] the lane's logs.
    /// Other lanes and shared infra stay untouched.
    pub(crate) async fn clean_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        actor: Option<&Actor>,
        logs: Logs,
    ) -> Result<Value, RpcError> {
        let actor = actor.cloned().unwrap_or_default();
        let reference = lane.st().reference(&project.name());
        // One hold of `op` from the stop to the release of the records: a start
        // waiting for the lane must not run in between.
        self.cancel_queued(lane, &reference, None, None);
        lane.cancel_start();
        let _op = lane.op.write().await;
        // `exec` and `run` take no `op`: interrupt runs and wait for spawns in
        // flight, so the stop below sees every unit and none appears until the
        // records are gone.
        let _closed = lane.close_adhoc().await;
        let stopped = self.stop_locked(project, lane, None, None, false).await?;
        // A unit whose stop timed out (or an orphan that would not end) still runs;
        // releasing the lane would leave it untracked.
        self.refuse_running(project, lane)?;
        let mut tasks = Vec::new();
        let mut teardown = Vec::new();
        let runnable = hooks_runnable(&lane.st());
        if runnable {
            // Daemon shutdown interrupts a task or entry that never ends.
            let cancel = self.shutdown.child_token();
            tasks = self
                .run_hook(project, lane, TaskWhen::Clean, &actor, &cancel)
                .await;
            if let Ok(resolved) = self.resolve_full(project, lane).await {
                for (name, command) in &resolved.teardown {
                    let result = self
                        .run_command(project, lane, "teardown", command, &actor, &cancel, None)
                        .await;
                    teardown.push(match result {
                        Ok((outcome, _)) => json!({"entry": name, "exit_code": outcome.code, "ignored": command.ignore_errors}),
                        Err(error) => json!({"entry": name, "error": {"code": error.code}}),
                    });
                }
            }
            // A clean task or teardown entry may have left a unit that would not stop.
            self.refuse_running(project, lane)?;
        }
        let index = lane.st().index();
        let infra = match index {
            Some(index) => match self.integ.infra.remove_lane(&project.name(), index).await {
                Ok(value) => value,
                Err(error) => {
                    json!({"error": {"code": error.code, "message": error.message}})
                }
            },
            None => Value::Null,
        };
        let record = lane.st().record.clone();
        if let Some(record) = &record {
            // Every run must be finished before the lane's records may be released;
            // none is alive by now, so each row records a real exit.
            let runs: Vec<_> = lane
                .st()
                .units
                .values()
                .filter(|rec| !rec.host.is_alive())
                .filter_map(|rec| rec.run_id.map(|id| (id, rec.host.exit_outcome())))
                .collect();
            for (id, outcome) in runs {
                let outcome = outcome.unwrap_or(ExitOutcome {
                    code: None,
                    signal: None,
                });
                // A row left unfinished makes `clean_lane` refuse with `active_runs`.
                self.finish_run(id, outcome).await;
            }
            // Setup records, once-only markers and task history go in the same
            // transaction as the lane's block.
            self.store
                .clean_lane(record.id)
                .await
                .map_err(store_error)?;
            // Last, so a clean that fails earlier keeps the data its teardown may need.
            self.remove_lane_data(&project.name(), record.id).await;
            // Every unit stopped and closed its log above.
            if let (Logs::Delete, Some(index)) = (logs, index) {
                self.remove_lane_logs(&project.name(), index).await;
            }
        }
        let present = {
            let mut st = lane.st();
            st.record = None;
            // Cleaned, the worktree is no lane until devlanes runs in it again.
            st.wanted = false;
            st.allocation = None;
            st.view = None;
            st.units.clear();
            st.started_at_ms = None;
            st.infra_ns = None;
            st.infra_containers.clear();
            st.usage.clear();
            st.moves_to = None;
            st.present
        };
        self.bus
            .emit(events::LANE_REMOVED, json!({"lane": reference}));
        if !present {
            project.st().lanes.remove(&lane.worktree);
            lane.watch.cancel();
        }
        self.forget_if_empty(project).await;
        self.publish_routes();
        Ok(json!({
            "lane": reference,
            "stopped": stopped.iter().map(|u| u.name.clone()).collect::<Vec<_>>(),
            "tasks": tasks,
            "teardown": teardown,
            "infra": infra,
        }))
    }

    /// Forgets a project whose every lane was cleaned: its rows go (the store
    /// refuses while a run is unfinished) and so does it in memory, so neither a
    /// daemon restart nor its watchers bring it back; the next command run in its
    /// folder registers it again.
    pub(crate) async fn forget_if_empty(&self, project: &Arc<ProjectRt>) {
        let empty = project.lanes().iter().all(|lane| {
            let st = lane.st();
            st.record.is_none() && !st.any_running() && st.busy.is_none()
        });
        if !empty {
            return;
        }
        if let Err(error) = self.store.delete_project(project.id).await {
            tracing::warn!(project = %project.name(), %error, "keeping a project whose lanes were all cleaned");
            return;
        }
        crate::model::lock(&self.projects).remove(&project.id);
        project.watch.cancel();
        for lane in project.lanes() {
            lane.watch.cancel();
        }
        tracing::info!(project = %project.name(), "forgot a project whose lanes were all cleaned");
    }

    /// Worktree auto-start or setup with the project's last request env.
    pub(crate) async fn auto_start(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        setup_only: bool,
    ) -> Result<(), RpcError> {
        let env = project.st().last_env.clone();
        self.start_lane(
            project,
            lane,
            StartOptions {
                selectors: Vec::new(),
                profiles: Vec::new(),
                transient: false,
                env,
                overrides: EnvMap::new(),
                actor: Actor::default(),
                timeout: self.opts.start_timeout,
                session: None,
                setup_only,
                all_at_once: false,
                start_id: None,
                arrival: None,
                restoring: false,
            },
        )
        .await
        .map(drop)
    }

    /// Daemon shutdown: stops every live unit with its policy. With `hooks` (an
    /// explicit shutdown) each lane with live units first runs its `lane.stop`
    /// tasks, as a whole-lane stop does, until `hooks` fires. Returns the
    /// stopped units (`{lane, unit}`) and the task results. Reports the
    /// `thaw`, `tasks` and `units` steps of `steps`.
    pub(crate) async fn stop_everything(
        self: &Arc<Self>,
        hooks: Option<&CancellationToken>,
        steps: &StopSteps<'_>,
    ) -> (Vec<Value>, Vec<Value>) {
        let lanes = self.all_lanes();
        let _ops = self.lock_lanes(&lanes).await;
        let idle = lanes
            .iter()
            .filter(|(_, lane)| lane.st().power.is_some())
            .count();
        let thaw = steps.begin(StoppingStep::Thaw, idle, None);
        self.wake_all().await;
        if let Some(thaw) = thaw {
            steps.done(&thaw);
        }
        let tasks = match hooks {
            Some(cancel) => self.shutdown_hooks(&lanes, cancel, steps).await,
            None => {
                steps.skipped(StoppingStep::Tasks);
                Vec::new()
            }
        };
        (Self::stop_lanes_locked(&lanes, Some(steps)).await, tasks)
    }

    /// The `lane.stop` tasks of an explicit daemon shutdown, for every lane in
    /// `lanes` with live units (the caller holds their `op`): lanes run
    /// concurrently, each lane's tasks in file order. Each result is
    /// `{lane, task, exit_code | error}`; a failed task is logged and reported
    /// and never keeps the other tasks or the unit stops from running.
    /// Reported as the `tasks` step of `steps`.
    async fn shutdown_hooks(
        self: &Arc<Self>,
        lanes: &[crate::engine::LanePair],
        cancel: &CancellationToken,
        steps: &StopSteps<'_>,
    ) -> Vec<Value> {
        // Resolved first, so the step knows how many tasks it runs.
        let resolved = join_all(
            lanes
                .iter()
                .filter(|(_, lane)| {
                    let st = lane.st();
                    hooks_runnable(&st) && st.any_alive()
                })
                .map(|(project, lane)| async move {
                    let resolved = self.resolve_full(project, lane).await.ok()?;
                    Some((project, lane, resolved))
                }),
        )
        .await
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let total = resolved
            .iter()
            .map(|(_, _, resolved)| {
                resolved
                    .tasks
                    .values()
                    .filter(|task| task.when == Some(TaskWhen::Stop) && task.active)
                    .count()
            })
            .sum();
        let Some(run) = steps.begin(StoppingStep::Tasks, total, None) else {
            return Vec::new();
        };
        let runs = resolved
            .into_iter()
            .map(|(project, lane, resolved)| async move {
                lane.st().shutdown_hooks = true;
                let results = self
                    .run_resolved_hook(
                        project,
                        lane,
                        &resolved,
                        TaskWhen::Stop,
                        &Actor::default(),
                        cancel,
                    )
                    .await;
                lane.st().shutdown_hooks = false;
                let reference = lane.st().reference(&project.name());
                results
                    .into_iter()
                    .map(|mut result| {
                        if result.get("exit_code").and_then(Value::as_i64) != Some(0) {
                            tracing::warn!(lane = %reference, task = %result["task"], result = %result, "lane.stop task failed during daemon shutdown");
                        }
                        result["lane"] = json!(reference);
                        result
                    })
                    .collect::<Vec<_>>()
            });
        let results = join_all(runs).await.into_iter().flatten().collect();
        steps.done(&run);
        results
    }

    /// Withdraws every queued start, interrupts running starts and takes the `op`
    /// lock of each of `lanes`, in order, so nothing starts in them meanwhile.
    pub(crate) async fn lock_lanes<'a>(
        &self,
        lanes: &'a [crate::engine::LanePair],
    ) -> Vec<tokio::sync::RwLockWriteGuard<'a, ()>> {
        self.pacing.cancel_all();
        for (_, lane) in lanes {
            lane.cancel_start();
        }
        let mut guards = Vec::with_capacity(lanes.len());
        for (_, lane) in lanes {
            guards.push(lane.op.write().await);
        }
        guards
    }

    /// Stops every live unit of `lanes`, whose `op` locks the caller holds, once
    /// their spawns in flight landed; all at once, so the longest stop timeout
    /// bounds the wait. Returns the stopped units as `{lane, unit}`. With
    /// `steps` (a daemon shutdown), reported as its `units` step, counting
    /// down as each stop finishes. A unit slower than [`SLOW_STOP`] is logged.
    pub(crate) async fn stop_lanes_locked(
        lanes: &[crate::engine::LanePair],
        steps: Option<&StopSteps<'_>>,
    ) -> Vec<Value> {
        let mut stops = Vec::new();
        let mut stopped = Vec::new();
        let mut busy_lanes = 0;
        for (project, lane) in lanes {
            lane.spawns_settled().await;
            let st = lane.st();
            let reference = st.reference(&project.name());
            let before = stops.len();
            for (name, rec) in st.units.iter().filter(|(_, rec)| rec.host.is_alive()) {
                stops.push((reference.clone(), name.clone(), rec.host.clone(), rec.stop));
                stopped.push(json!({"lane": reference, "unit": name}));
            }
            if stops.len() > before {
                busy_lanes += 1;
            }
        }
        let total = stops.len();
        let run = steps.and_then(|steps| steps.begin(StoppingStep::Units, total, Some(busy_lanes)));
        let mut pending: FuturesUnordered<_> = stops
            .into_iter()
            .map(|(lane, unit, host, policy)| async move {
                let begun = Instant::now();
                if let Err(error) = host.stop(policy).await {
                    tracing::warn!(%lane, %unit, code = %error.code, "unit stop failed");
                }
                (lane, unit, begun.elapsed())
            })
            .collect();
        let mut left = total;
        while let Some((lane, unit, elapsed)) = pending.next().await {
            left -= 1;
            if elapsed > SLOW_STOP {
                tracing::info!(
                    %lane,
                    %unit,
                    elapsed_ms = elapsed.as_millis() as u64,
                    "unit was slow to stop"
                );
            }
            if let (Some(steps), Some(run)) = (steps, &run) {
                steps.progress(run, left);
            }
        }
        if let (Some(steps), Some(run)) = (steps, &run) {
            steps.done(run);
        }
        stopped
    }

    /// `lane_running` while a unit or a previous daemon's orphan still runs in `lane`.
    fn refuse_running(&self, project: &ProjectRt, lane: &LaneRt) -> Result<(), RpcError> {
        let st = lane.st();
        let mut units: Vec<String> = st
            .units
            .iter()
            .filter(|(_, rec)| rec.host.is_alive())
            .map(|(name, _)| name.clone())
            .collect();
        units.extend(st.orphans.keys().cloned());
        if units.is_empty() {
            return Ok(());
        }
        Err(RpcError::new(
            "lane_running",
            json!({"lane": st.reference(&project.name()), "units": units}),
            1,
        ))
    }

    /// Stores the lane's desired state, profiles and the services a plain start
    /// brings back, then refreshes its record.
    async fn save_desired(
        &self,
        lane: &LaneRt,
        record: &devlanes_state::Lane,
        desired: DesiredState,
        profiles: Vec<String>,
        services: Vec<String>,
    ) {
        let id = record.id;
        let saved = persist("lane desired state", || {
            self.store
                .set_lane_selection(id, desired, profiles.clone(), services.clone())
        })
        .await;
        if saved && let Some(updated) = self.store.lane(id).await.ok().flatten() {
            lane.st().record = Some(updated);
        }
    }

    /// The lane's profiles and the services they select, for `why`. `autostart`
    /// expressions use the lane's precomputed layout.
    // TODO: the layout is evaluated with the infra values known when it was built, while a
    // start evaluates `autostart` with current ones. An expression reading an infra value
    // (a port, a container's state) can make `why` report a selection that differs from
    // what a start picks. Evaluate it the way a start does.
    pub(crate) fn profile_services(lane: &LaneRt) -> Option<(Vec<String>, BTreeSet<String>)> {
        let st = lane.st();
        let config = st.config.loaded()?;
        let mut selection = devlanes_resolve::select_profiles(config, &st.profiles, &[]).ok()?;
        devlanes_resolve::retain_autostart(
            &mut selection.services,
            &selection.autostart_pending,
            |name| {
                st.view.as_ref().and_then(|view| {
                    view.services
                        .iter()
                        .filter(|s| s.name == name)
                        .map(|s| s.autostart)
                        .reduce(|any, next| any || next)
                })
            },
        );
        Some((selection.profiles, selection.services.into_iter().collect()))
    }

    /// Runs the `ready` checks of the lane's plain containers concurrently; the
    /// first failure is returned.
    async fn containers_ready(&self, lane: &LaneRt, spec: &InfraSpec) -> Result<(), RpcError> {
        let mut env: EnvMap = lane.st().env.as_deref().cloned().unwrap_or_default();
        env.extend(spec.env.clone());
        // `up` just listed every plain container here under its Docker name.
        let dockers: BTreeMap<String, String> = lane
            .st()
            .infra_containers
            .iter()
            .map(|container| (container.name.clone(), container.docker.clone()))
            .collect();
        let checks = spec.containers.iter().filter_map(|container| {
            let ready = container.ready.as_ref()?;
            let docker = dockers.get(&container.name);
            let ports: Vec<(String, u16)> = container
                .ports
                .iter()
                .filter_map(|(name, _)| {
                    let key = format!("infra.{}/{name}", container.name);
                    spec.ports.get(&key).map(|port| (name.clone(), *port))
                })
                .collect();
            let env = &env;
            Some(async move {
                // A failed read counts as no output yet, so the check polls again.
                let logs = || async {
                    let Some(docker) = docker else {
                        return Vec::new();
                    };
                    let options = crate::integrations::ContainerLogOptions {
                        tail: Some(200),
                        ..Default::default()
                    };
                    let Ok(mut stream) = self.integ.infra.container_logs(docker, options).await
                    else {
                        return Vec::new();
                    };
                    let mut bytes = Vec::new();
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(chunk) => bytes.extend_from_slice(&chunk.bytes),
                            Err(_) => return Vec::new(),
                        }
                    }
                    String::from_utf8_lossy(&bytes)
                        .lines()
                        .map(str::to_owned)
                        .collect()
                };
                #[cfg(unix)]
                let check =
                    crate::ready::container_ready(&container.name, ready, &ports, env, logs);
                // Windows runs `cmd` checks through the shell found at daemon start.
                #[cfg(windows)]
                let check = crate::ready::container_ready(
                    &container.name,
                    ready,
                    &ports,
                    env,
                    self.integ.shell.as_deref(),
                    logs,
                );
                check.await
            })
        });
        join_all(checks).await.into_iter().collect()
    }

    /// Settles a templated profile `infra` by planning the lane against what already
    /// runs. A planning failure keeps infra on; the resolve phase reports it.
    async fn profiles_enable_infra(self: &Arc<Self>, cx: &StartCx, spec: &InfraSpec) -> bool {
        if let Ok(up) = self.integ.infra.namespace(spec).await {
            let mut st = cx.lane.st();
            st.infra_ns = Some(up.namespace);
            st.infra_containers = up.containers;
            st.setup_env = up.setup_env;
        }
        self.resolve_config(&cx.project, &cx.lane, cx.config.clone(), None)
            .await
            .ok()
            .is_none_or(|resolved| resolved.infra_enabled)
    }
}

/// `config_error`, located like `config check`'s, when an `infra.compose` file is
/// missing from `config_dir`: the daemon's load leaves that check out, since a missing
/// file must not unload a running lane's config, and Docker would only report it later
/// and less clearly.
async fn check_compose_files(
    config: &Arc<devlanes_config::ProjectConfig>,
    config_dir: std::path::PathBuf,
) -> Result<(), RpcError> {
    if config.infra.compose.is_none() {
        return Ok(());
    }
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        devlanes_config::check_compose_files(&config, &config_dir)
            .map_err(|error| crate::model::config_error(&error))
    })
    .await
    .map_err(|error| internal("config_error", error))?
}

/// Runs a state.db write, retrying briefly as [`Inner::finish_run`] does; a
/// write that still fails is logged. True once written.
pub(crate) async fn persist<T, F, Fut>(what: &str, write: F) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, devlanes_state::Error>>,
{
    const ATTEMPTS: u32 = 3;
    let mut delay = Duration::from_millis(100);
    for attempt in 1..=ATTEMPTS {
        match write().await {
            Ok(_) => return true,
            // Shutdown closed the store; nothing more can be written.
            Err(devlanes_state::Error::Closed) => return false,
            Err(error) if attempt == ATTEMPTS => {
                tracing::warn!(%error, what, "state.db write failed");
            }
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
    }
    false
}

/// Finishes the row and emits the closing `task` event of a task run whose
/// future was dropped before it got there.
struct TaskRunGuard {
    engine: Arc<Inner>,
    /// `None` once the run closed normally.
    run: Option<devlanes_state::TaskRunId>,
    reference: String,
    task: String,
}

impl Drop for TaskRunGuard {
    /// Drop cannot wait for state.db, so the row is finished on the runtime.
    fn drop(&mut self) {
        let Some(id) = self.run.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let engine = self.engine.clone();
        let reference = std::mem::take(&mut self.reference);
        let task = std::mem::take(&mut self.task);
        runtime.spawn(async move {
            engine
                .close_task_run(
                    id,
                    &reference,
                    &task,
                    None,
                    "failed",
                    Some(json!({"code": "cancelled"})),
                    None,
                )
                .await;
        });
    }
}

/// Waits for a one-shot unit, stopping it after `timeout` or once `cancel`
/// fires; an interrupted unit has no exit code.
async fn finish(
    host: &UnitHost,
    timeout: Option<Duration>,
    cancel: &CancellationToken,
) -> ExitOutcome {
    tokio::select! {
        // An exit that is already there wins over a cancellation.
        biased;
        outcome = finish_within(host, timeout) => outcome,
        () = cancel.cancelled() => {
            if let Err(error) = host.stop(StopPolicy::default()).await {
                tracing::warn!(code = %error.code, unit = %host.info().name, "interrupted unit did not stop");
            }
            ExitOutcome {
                code: None,
                signal: host.exit_outcome().and_then(|o| o.signal),
            }
        }
    }
}

/// Waits for a one-shot unit, stopping it after `timeout`.
async fn finish_within(host: &UnitHost, timeout: Option<Duration>) -> ExitOutcome {
    match timeout {
        Some(limit) => match tokio::time::timeout(limit, host.wait_exit()).await {
            Ok(outcome) => outcome,
            Err(_) => {
                let _ = host.stop(StopPolicy::default()).await;
                ExitOutcome {
                    code: None,
                    signal: host.exit_outcome().and_then(|o| o.signal),
                }
            }
        },
        None => host.wait_exit().await,
    }
}

/// How long after its spawn a unit's exit still counts as a failed start.
const EARLY_EXIT: Duration = Duration::from_millis(500);

/// Waits until each live unit of `hosts` has run for [`EARLY_EXIT`] (or the
/// start's deadline passed), returning early once all of them exited.
async fn settle_early_exits(hosts: &[UnitHost], deadline: Instant) {
    let now = now_ms();
    let youngest = hosts
        .iter()
        .filter(|host| host.is_alive())
        .map(|host| host.info().started_at_ms)
        .max();
    let Some(started) = youngest else {
        return;
    };
    let early = u64::try_from(EARLY_EXIT.as_millis()).unwrap_or(u64::MAX);
    let left = Duration::from_millis((started + early).saturating_sub(now));
    let until = (Instant::now() + left).min(deadline);
    let all_exited = join_all(hosts.iter().map(UnitHost::wait_exit));
    let _ = tokio::time::timeout_at(until, all_exited).await;
}

/// Polls a TCP port until nothing accepts on it.
async fn wait_port_free(port: u16, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if !crate::ready::tcp_open(port).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Sends a unit's log lines as `log` events until it exits or the client leaves.
pub(crate) async fn stream_logs(host: UnitHost, session: Session) {
    let name = host.info().name.clone();
    let disconnected = session.disconnected();
    // Earlier runs of the unit share its log; only this run's lines are streamed.
    let mut cursor: Option<String> = host.run_cursor();
    loop {
        // The exit is published after output drained, so this pass is the last one.
        let alive = host.is_alive();
        let query = LogQuery {
            cursor: cursor.clone(),
            max_bytes: Some(256 * 1024),
            // The client words them; colors stay for one that shows them.
            styled: true,
            ..LogQuery::default()
        };
        if let Ok(page) = host.logs(query).await {
            for line in page.items {
                cursor = Some(format!("{}#{}", line.unit, line.seq));
                let event = devlanes_proto::Event {
                    seq: 0,
                    event: events::LOG.into(),
                    data: json!({"unit": name, "line": line}),
                };
                if session.event(event).await.is_err() {
                    return;
                }
            }
        }
        if !alive || disconnected.is_cancelled() {
            return;
        }
        tokio::select! {
            () = disconnected.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A start's control under a fresh shutdown token.
    fn ctl(start_id: u64) -> Arc<StartCtl> {
        Arc::new(StartCtl::new(&CancellationToken::new(), start_id))
    }

    #[test]
    /// A resume plan names units; replicas map to their service, and units and
    /// profiles the config dropped are left out and reported.
    fn restoring_maps_replicas_and_drops_removed_names() {
        let config = devlanes_config::parse_project(
            "project: demo\nprofiles:\n  front: [web]\nservices:\n  web: { run: serve, replicas: 2 }\n  api: { run: serve }\n",
            None,
        )
        .expect("config");
        let options = StartOptions {
            selectors: vec!["web.1".into(), "web.2".into(), "gone".into()],
            profiles: vec!["front".into(), "old".into()],
            transient: false,
            env: None,
            overrides: EnvMap::new(),
            actor: Actor::default(),
            timeout: Duration::from_secs(1),
            session: None,
            setup_only: false,
            all_at_once: false,
            start_id: None,
            arrival: None,
            restoring: true,
        };
        let (selection, dropped) =
            start_selection(&config, None, &[], &options, true).expect("selection");
        assert_eq!(selection.services, ["web"]);
        assert_eq!(selection.profiles, ["front"]);
        assert_eq!(
            dropped,
            Dropped {
                profiles: vec!["old".into()],
                services: vec!["gone".into()],
            }
        );
        assert_eq!(dropped.warnings("demo@0").len(), 2);
    }

    #[test]
    /// The declaration hash follows what a task runs, not when it runs or how long it
    /// may take, and unset fields do not count.
    fn declaration_hashes_follow_what_runs() {
        let parse = |task: &str| {
            devlanes_config::parse_project(
                &format!("project: demo\ntasks:\n  install: {task}\n"),
                None,
            )
            .expect("config")
        };
        let hash =
            |config: &devlanes_config::ProjectConfig| task_hash(config, "install").expect("hash");
        let one = parse("{ run: pnpm install, when: lane.create }");
        assert_eq!(
            hash(&one),
            hash(&parse(
                "{ run: pnpm install, when: lane.create, timeout: 10m, env: {} }"
            ))
        );
        assert_ne!(
            hash(&one),
            hash(&parse(
                "{ run: pnpm install --frozen-lockfile, when: lane.create }"
            ))
        );
        assert_ne!(
            hash(&one),
            hash(&parse(
                "{ run: pnpm install, when: lane.create, env: { CI: 1 } }"
            ))
        );
        assert!(same_declaration(None, Some(&hash(&one))));
        assert!(!same_declaration(Some("0"), Some(&hash(&one))));
        assert_eq!(
            canonical_json(json!({"a": null, "b": [], "c": {"d": {}}})),
            None
        );
    }

    #[tokio::test(start_paused = true)]
    /// A start that joined `b` goes on once `b` spawned, while the start that
    /// owns it still waits for its other services.
    async fn a_joined_unit_settles_without_the_owners_other_services() {
        let (ours, owner) = (ctl(2), ctl(1));
        owner.plan_spawns(["b".to_owned(), "c".to_owned()]);
        let deadline = Instant::now() + Duration::from_secs(60);
        let joined = tokio::spawn({
            let (ours, owner) = (ours.clone(), owner.clone());
            async move { joined_outcome(&ours, deadline, "b", &owner).await }
        });
        tokio::time::sleep(Duration::from_secs(1)).await;
        owner.record_outcome("b", None);
        let joined = tokio::time::timeout(Duration::from_secs(1), joined)
            .await
            .expect("settled by b alone")
            .expect("join");
        assert!(matches!(joined, Joined::Spawned));
        assert!(!owner.services_done.is_cancelled(), "c still waits");
    }

    #[tokio::test(start_paused = true)]
    /// A failed spawn, a withdrawal and the joiner's deadline each end the wait.
    async fn a_joined_unit_reports_failure_withdrawal_and_timeout() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (ours, owner) = (ctl(2), ctl(1));
        owner.record_outcome("b", Some(RpcError::new("spawn_failed", json!({}), 1)));
        assert!(matches!(
            joined_outcome(&ours, deadline, "b", &owner).await,
            Joined::Failed(error) if error.code == "spawn_failed"
        ));

        let (ours, owner) = (ctl(2), ctl(1));
        owner.withdraw(&["b".to_owned()], "stop");
        assert!(matches!(
            joined_outcome(&ours, deadline, "b", &owner).await,
            Joined::Gone
        ));

        let (ours, owner) = (ctl(2), ctl(1));
        assert!(matches!(
            joined_outcome(&ours, deadline, "b", &owner).await,
            Joined::TimedOut
        ));
    }

    #[tokio::test(start_paused = true)]
    /// A failing write is retried up to three times; a closed store is not retried.
    async fn persist_retries_then_gives_up() {
        let calls = AtomicU32::new(0);
        let flaky = persist("test", || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if call < 2 {
                    Err(devlanes_state::Error::Invalid("busy".into()))
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert!(flaky);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        calls.store(0, Ordering::SeqCst);
        let failing = persist("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(devlanes_state::Error::Invalid("busy".into())) }
        })
        .await;
        assert!(!failing);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        calls.store(0, Ordering::SeqCst);
        let closed = persist("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(devlanes_state::Error::Closed) }
        })
        .await;
        assert!(!closed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
