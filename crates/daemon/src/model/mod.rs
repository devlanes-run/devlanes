//! In-memory registry of projects, lanes and units. SQLite holds the durable part
//! (identities, indices, ports, history); live process state and client
//! environments exist only here.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use devlanes_config::ProjectConfig;
use devlanes_core::{ProjectId, StopPolicy, UnitRunId, WorktreeId, env::Environment};
use devlanes_proto::{Actor, ActorKind, LaneSummary, RpcError, UnitSummary};
use devlanes_resolve::RouteSpec;
use devlanes_state::{Allocation, DesiredState, Lane};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::units::{UnitHost, UnitState};

mod lane_infra;

pub(crate) use lane_infra::{CurrentInfra, EventEffect, InfraRead, LaneInfra};

/// Client environments; values may hold tokens and stay in memory.
pub(crate) type EnvMap = BTreeMap<String, String>;

/// Locks a std mutex, recovering the data if a panicking holder poisoned it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What kind of process a unit is; clients group and filter on the word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Service,
    Task,
    Exec,
}

impl Kind {
    /// Stable word for `UnitSummary.kind`.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Task => "task",
            Self::Exec => "exec",
        }
    }
}

/// Config of one lane's worktree.
#[derive(Clone)]
pub(crate) enum ConfigState {
    /// The worktree has no project file at the project's relative path.
    Missing,
    Loaded(Arc<ProjectConfig>),
    /// Parse or validation failure, already shaped as `config_error` data.
    Invalid(RpcError),
    /// The worktree's project file names another project (a rename, or a branch of
    /// another project checked out). The lane stays until `lane gc` collects it.
    Elsewhere(String),
}

impl ConfigState {
    /// The loaded config, if any.
    pub(crate) fn loaded(&self) -> Option<&Arc<ProjectConfig>> {
        match self {
            Self::Loaded(config) => Some(config),
            _ => None,
        }
    }

    /// The loaded config, else the reason there is none, for a command that needs it.
    pub(crate) fn require(&self, lane: &str) -> Result<Arc<ProjectConfig>, RpcError> {
        match self {
            Self::Loaded(config) => Ok(config.clone()),
            Self::Invalid(error) => Err(error.clone()),
            Self::Elsewhere(other) => Err(elsewhere_error(other, Some(lane))),
            Self::Missing => Err(RpcError::new("no_config", json!({"lane": lane}), 1)),
        }
    }
}

/// One service replica as laid out for this lane (ports, URLs, routes).
#[derive(Debug, Clone)]
pub(crate) struct ServiceInfo {
    pub name: String,
    pub unit: String,
    pub port: u16,
    pub ports: BTreeMap<String, u16>,
    pub url: String,
    pub public_url: Option<String>,
    pub tags: Vec<String>,
    pub hidden: bool,
    pub autostart: bool,
    /// Palette name from the layout.
    pub color: String,
    /// `services.<name>.name`, for `UnitSummary.display_name`.
    pub display_name: Option<String>,
}

/// One `infra.hosts` route to a container's published host port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InfraRoute {
    pub host: String,
    pub container: String,
    pub port: u16,
    pub fallback: bool,
    pub host_header_upstream: bool,
    /// `https://host` through the proxy; `None` for wildcard hosts.
    pub url: Option<String>,
}

/// One `infra.external` entry as this lane sees it: the same port in every
/// lane of the project, hostnames evaluated for this lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalInfo {
    pub name: String,
    /// The declared address, the only one devlanes dials for it.
    pub address: std::net::SocketAddr,
    pub port: u16,
    /// `http://127.0.0.1:<port>` or `http://[::1]:<port>`, as the address names it.
    pub url: String,
    pub hidden: bool,
    pub routes: Vec<ExternalRoute>,
}

/// One hostname of an [`ExternalInfo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalRoute {
    pub host: String,
    pub fallback: bool,
    pub host_header_upstream: bool,
    /// `https://host` through the proxy; `None` for wildcard hosts.
    pub url: Option<String>,
}

impl ExternalInfo {
    /// The URL a browser opens: the first hostname that is not a wildcard.
    pub(crate) fn public_url(&self) -> Option<String> {
        self.routes.iter().find_map(|route| route.url.clone())
    }
}

/// Synchronous layout of a lane, available while it is idle.
#[derive(Debug, Clone)]
pub(crate) struct LaneView {
    pub domain: String,
    /// `domain`'s HTTPS URL through the proxy, built like the services' public
    /// URLs; `None` when the lane routes no hostname.
    pub url: Option<String>,
    /// `lane.port`.
    pub port: u16,
    /// Evaluated `lanes.vars`, for infra expressions.
    pub vars: indexmap::IndexMap<String, devlanes_config::Scalar>,
    pub block: (u16, u16),
    pub services: Vec<ServiceInfo>,
    pub routes: Vec<RouteSpec>,
    /// Routes to containers from `infra.hosts`.
    pub infra_routes: Vec<InfraRoute>,
    /// `infra.external` entries, in declaration order.
    pub externals: Vec<ExternalInfo>,
    /// The lane's `services` expression namespace.
    pub services_namespace: Value,
}

/// What a unit run was launched with. Memory only, never state.db: it holds
/// environment values.
#[derive(Default)]
pub(crate) struct LaunchRecord {
    /// Effective environment with provenance, for `env --explain`.
    pub env: Environment,
    /// Launch notes, for `env --explain`.
    pub notes: Vec<String>,
    /// An exec run's lane variables left out because the container they read
    /// was down, for `env show` to name.
    pub left_out: BTreeMap<String, devlanes_resolve::LeftOut>,
}

impl LaunchRecord {
    /// The record of a service, task or infra command run from its plan.
    pub(crate) fn planned(launch: &devlanes_resolve::Launch) -> Self {
        Self {
            env: crate::lifecycle::copy_env(&launch.env),
            notes: launch.notes.clone(),
            left_out: BTreeMap::new(),
        }
    }
}

/// A spawned unit and its static facts.
pub(crate) struct UnitRec {
    pub host: UnitHost,
    pub kind: Kind,
    pub run_id: Option<UnitRunId>,
    pub ports: BTreeMap<String, u16>,
    pub urls: Vec<String>,
    pub tags: Vec<String>,
    pub hidden: bool,
    pub autostart: bool,
    pub stop: StopPolicy,
    pub literal_port: Option<u16>,
    /// What this run was launched with, for `env show <unit>`.
    pub launch: Arc<LaunchRecord>,
    /// Last log lines captured when the unit failed, for proxy pages and errors.
    pub failure_tail: Vec<String>,
    /// `failure_tail` with colors; empty when it had none.
    pub failure_tail_styled: Vec<String>,
    pub preset: Option<String>,
    /// Contributor-prepared files, kept for cross-lane launch checks.
    pub files: Vec<devlanes_core::contrib::PreparedFile>,
    /// Palette name for `UnitSummary.color`.
    pub color: Option<String>,
    /// The runner's handle, for what the run left behind once it ended.
    pub managed: Arc<dyn crate::integrations::ManagedUnit>,
    /// `services.<name>.sync` entries mirrored while this run lasts.
    pub syncs: Vec<crate::local_sync::LocalSync>,
    /// `services.<name>.name` of a service run.
    pub display_name: Option<String>,
    /// `UnitSummary.role`.
    pub role: &'static str,
    /// `services.<name>.keep_awake`: the lane never goes idle while this runs.
    pub keep_awake: bool,
    /// What a service run was spawned with, to tell when the config changed it.
    /// Memory only, never state.db: it holds environment values.
    pub fingerprint: Option<Arc<crate::drift::Fingerprint>>,
}

impl UnitRec {
    /// Processes the ended run left running or that escaped its final sweep;
    /// empty while it runs.
    pub(crate) fn leftovers(&self, unit: &str) -> devlanes_proto::Leftovers {
        let details = self.managed.exit_details();
        devlanes_proto::Leftovers {
            unit: unit.to_owned(),
            left_running: details
                .as_ref()
                .map(|d| d.left_running.clone())
                .unwrap_or_default(),
            escaped: details.map(|d| d.escaped).unwrap_or_default(),
        }
    }
}

/// A process a previous daemon started that still runs (identity verified at
/// startup). Not supervised: no output, input or exit status, only liveness by
/// identity, until a stop terminates it or it exits on its own.
#[derive(Debug, Clone)]
pub(crate) struct Orphan {
    pub run_id: UnitRunId,
    pub process: devlanes_core::ProcessIdentity,
    /// When the previous daemon started it (Unix ms).
    pub started_at_ms: u64,
    /// When this daemon found it (Unix ms).
    pub since_ms: u64,
    pub actor: Actor,
    /// Descendants that left the leader's session, as the previous daemon recorded
    /// them; signalled with the leader and counted for liveness.
    pub members: Vec<devlanes_core::ProcessIdentity>,
}

/// Longest gap a reading stands for, so a unit that went unread for a while
/// does not let its next reading count for the whole gap. Kept above the
/// sampler's slowest interval (5 s, in `sampler.rs`).
const USAGE_GAP_MS: u64 = 10_000;

/// The sampler's readings of one unit over a lane start: peaks plus running
/// sums for the means, so nothing grows with the number of samples.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnitUsage {
    peak_memory: u64,
    peak_cpu: f64,
    /// Readings weighted by the milliseconds each stands for.
    memory_weighted: u128,
    cpu_weighted: f64,
    weight_ms: u64,
    last_ms: Option<u64>,
}

impl UnitUsage {
    /// Adds one reading taken at `at_ms`. It stands for the time since the
    /// unit's previous reading (at most [`USAGE_GAP_MS`]; 1 ms for the first),
    /// because the sampler reads faster while a client watches and the means
    /// should not lean towards those stretches.
    pub(crate) fn add(&mut self, cpu_percent: f64, memory_bytes: u64, at_ms: u64) {
        let weight = self
            .last_ms
            .map_or(1, |last| at_ms.saturating_sub(last).clamp(1, USAGE_GAP_MS));
        self.last_ms = Some(at_ms);
        self.peak_memory = self.peak_memory.max(memory_bytes);
        self.peak_cpu = self.peak_cpu.max(cpu_percent);
        self.memory_weighted += u128::from(memory_bytes) * u128::from(weight);
        self.cpu_weighted += cpu_percent * weight as f64;
        self.weight_ms += weight;
    }

    /// Peak `(memory, cpu)`; `None` before the first reading.
    pub(crate) fn peak(&self) -> (Option<u64>, Option<f64>) {
        if self.last_ms.is_none() {
            return (None, None);
        }
        (Some(self.peak_memory), Some(self.peak_cpu))
    }

    /// Time-weighted mean `(memory, cpu)`; `None` before the first reading.
    pub(crate) fn average(&self) -> (Option<u64>, Option<f64>) {
        if self.weight_ms == 0 {
            return (None, None);
        }
        let memory = self.memory_weighted / u128::from(self.weight_ms);
        (
            Some(u64::try_from(memory).unwrap_or(u64::MAX)),
            Some(self.cpu_weighted / self.weight_ms as f64),
        )
    }
}

/// Mutable state of one worktree lane.
///
/// The facts a lane's drift is computed from (where the worktree is, its
/// record and allocation, config, layout, client env, overrides, profiles,
/// units, orphans, waiting units and last whole start) are private and change
/// only through the setters below.
pub(crate) struct LaneSt {
    pub identity: String,
    path: PathBuf,
    branch: Option<String>,
    present: bool,
    /// `None` while unassigned (no index, ports or hosts).
    record: Option<Lane>,
    pub unassigned: Option<RpcError>,
    config: ConfigState,
    env: Option<Arc<EnvMap>>,
    overrides: EnvMap,
    /// Profiles of the last start, in the order given; empty means the default.
    profiles: Vec<String>,
    units: BTreeMap<String, UnitRec>,
    pub started_at_ms: Option<u64>,
    /// A stop of the whole lane runs. Starts in their phases are
    /// [`Self::starts_in_phases`].
    pub stopping: bool,
    view: Option<Arc<LaneView>>,
    allocation: Option<Allocation>,
    /// The lane's live infra; written only through the `*_infra` methods.
    infra: LaneInfra,
    pub setup_env: EnvMap,
    /// Why the last layout of the lane (`view`) did not resolve, such as a
    /// `lanes.vars` value with no value here. The next layout that resolves
    /// clears it. A file that does not load is the config state's to report.
    pub view_error: Option<RpcError>,
    pub last_error: Option<RpcError>,
    pub skipped_tasks: Vec<String>,
    /// Per-unit CPU and memory over the current lane start.
    pub usage: BTreeMap<String, UnitUsage>,
    /// Set by the moves check: the index this lane takes at its next start.
    pub moves_to: Option<u32>,
    /// Why a selected unit did not start: a lane phase before services failed.
    /// Shown as the unit's `reason` until a start reaches the services phase.
    pub phase_reasons: BTreeMap<String, RpcError>,
    /// Units left running by a previous daemon, by unit name.
    orphans: BTreeMap<String, Orphan>,
    /// The starts (and restarts) running on this lane, while they run. Several
    /// can run at once: one that brings the lane up and any number that add
    /// units to it. Stops reach them here instead of waiting for `op`. Each
    /// start installs its own, so nothing done to one reaches a start that
    /// begins later.
    pub starts: Vec<Arc<StartCtl>>,
    /// Units a start was asked for and has not handed to the start gate or
    /// spawned yet, so the lane shows them `queued` from the moment the request
    /// arrives instead of when its turn comes.
    waiting: BTreeMap<String, Waiting>,
    /// The start bringing the lane up (infra, setup, lane tasks), from the
    /// moment it decided to until its services phase ended. Starts that arrive
    /// meanwhile add their units to the lane it brings up instead of bringing
    /// it up again.
    pub bringup: Option<Bringup>,
    /// Starts and `infra up`s that passed their checks and have not returned.
    /// The lane counts as running for box lanes that clash with it while
    /// one is, so a start that fails its checks takes nothing from a box.
    pub claiming: usize,
    /// Whether a unit spawned, or containers were stored, since `claiming`
    /// last rose from zero. Until then a claim clears no lane choice, so a
    /// start that fails before anything runs gives the box lane back. A flag
    /// rather than what runs now, since a unit that exits at once still ran.
    pub claim_ran: bool,
    /// Spawns in flight ([`SpawnGuard`]). A stop waits until none is left before
    /// it picks its targets, so no unit appears after it looked.
    pub spawning: usize,
    /// Woken whenever `spawning` drops.
    pub spawned: Arc<Notify>,
    /// The config needs ports the block lacks (or another block size or start)
    /// while the lane runs something; the block moves at the next whole-lane
    /// start (plan 5.8).
    pub ports_pending: bool,
    /// `host_conflict` warnings from the latest route snapshot: hostnames of this
    /// lane another lane routes.
    pub host_conflicts: Vec<devlanes_proto::LaneWarning>,
    /// A block or view refresh waits for the operation holding `op` (normally a
    /// start between taking it and its last spawn); a task holding `op` next
    /// runs it.
    pub refresh_pending: bool,
    /// Held shared by `exec` and `run` (they take no `op`, so a start never waits
    /// for a task) and exclusively by `lane clean` and `daemon reset-state` while
    /// they release the lane, so no ad-hoc unit appears in a lane whose records
    /// are going away.
    pub adhoc_gate: Arc<tokio::sync::RwLock<()>>,
    /// Parent of the tokens of this lane's `run`s; a clean or reset fires it.
    pub adhoc_cancel: Option<CancellationToken>,
    /// How far a clean or reset holding this lane has got.
    pub closing: Closing,
    /// The config files as of the last reload; `None` before the first one.
    pub config_stamp: Option<crate::registry::ConfigStamp>,
    /// Every file the last config load read or looked for, which the stamp covers.
    pub config_files: Vec<PathBuf>,
    /// The last config that loaded. While units run and the file is invalid, missing or
    /// names another project, the lane's layout and routes keep following it, so a
    /// typo or a rebase conflict does not take running services off the proxy.
    pub last_good: Option<Arc<ProjectConfig>>,
    /// How the running units differ from what the current config would start;
    /// written only by the lane's drift pass ([`crate::drift`]).
    drift: crate::drift::Drift,
    /// The infra settings and left-out units of the lane's last whole start,
    /// which `infra_changed` and `added` compare against.
    started_with: Option<crate::drift::StartedWith>,
    /// The last run of each service the config no longer has, once it stopped:
    /// `logs`, `debug why` and `debug errors` still find it.
    past: BTreeMap<String, UnitRec>,
    /// What the expressions of the loaded config read of `lanes.main` and
    /// `projects`, kept with it.
    references: Option<Arc<devlanes_resolve::References>>,
    /// Ticked by every setter of a fact above.
    facts: crate::units::FactsClock,
    /// Settings this worktree's file sets differently from the primary worktree's,
    /// where the primary's applies (`project_setting_ignored`) or the lanes share one
    /// container (`infra_spec_conflict`).
    pub divergent: Vec<devlanes_proto::LaneWarning>,
    /// Set while an explicit daemon shutdown runs this lane's `lane.stop`
    /// tasks: their spawns pass the refusal that starts with the shutdown.
    pub shutdown_hooks: bool,
    /// devlanes ran in this worktree, so it gets a lane. Worktrees that merely
    /// exist get none, except the primary one; an extra lane (`lane new`) gets
    /// one by existing.
    pub wanted: bool,
    /// A clean of this lane, whose worktree is gone, is under way or scheduled.
    pub sweeping: bool,
    /// When a sweep first saw the folder gone; cleared once it is back.
    pub missing_since: Option<std::time::Instant>,
    /// Set while the idle loop has the lane's processes throttled or frozen.
    pub power: Option<LanePowerSt>,
}

/// How far a clean or reset holding a lane has got ([`LaneSt::closing`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Closing {
    /// No clean or reset holds the lane.
    #[default]
    No,
    /// [`LaneRt::close_adhoc`] closed the lane: its stop, `lane.clean` tasks,
    /// teardown and container removal may still run and may need the shared
    /// containers.
    Hooks,
    /// A clean finished all of that; the lane no longer uses the shared
    /// containers, though its rows stay until the clean releases them.
    Released,
}

/// A unit a start asked for and has not queued in the start gate or spawned.
#[derive(Debug, Clone)]
pub(crate) struct Waiting {
    /// The start that asked for it; a later start of the same unit takes over.
    pub start_id: u64,
    /// The requester, so `status --mine` finds the unit while it waits.
    pub actor: Actor,
}

/// The start bringing a lane up, while it does.
#[derive(Debug, Clone)]
pub(crate) struct Bringup {
    pub start_id: u64,
    /// What it runs now, in words a person reads in `status` and the TUI: `containers`,
    /// `setup`, a task name. `None` before its first phase and once it reached services.
    pub step: Option<String>,
}

/// A throttled or frozen lane, for its summary.
#[derive(Debug, Clone)]
pub(crate) struct LanePowerSt {
    pub level: devlanes_core::Power,
    /// When the lane reached `level` (Unix ms).
    pub since_ms: u64,
    /// The last activity, in the idle board's monotonic ms, that the change of
    /// level took into account; anything newer thaws the lane.
    pub seen: u64,
    /// The last activity before the lane went idle: Unix ms and its kind word.
    pub last_activity: Option<(u64, &'static str)>,
    /// Per-lane containers paused with the freeze, unpaused first on thaw.
    pub paused_containers: bool,
}

impl LanePowerSt {
    /// The protocol form for `LaneSummary.power`.
    pub(crate) fn summary(&self) -> devlanes_proto::LanePower {
        devlanes_proto::LanePower {
            level: self.level.word().into(),
            since_ms: self.since_ms,
            last_activity_ms: self.last_activity.map(|(ms, _)| ms),
            last_activity: self.last_activity.map(|(_, kind)| kind.into()),
        }
    }
}

/// Stable words for power levels in summaries and events.
pub(crate) trait PowerWord {
    /// `normal`, `throttled` or `frozen`.
    fn word(self) -> &'static str;
}

impl PowerWord for devlanes_core::Power {
    /// `normal`, `throttled` or `frozen`.
    fn word(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Throttled => "throttled",
            Self::Frozen => "frozen",
        }
    }
}

/// Marks a worktree identity as an extra lane of another worktree's folder:
/// `<base identity>#lane=<name>`.
pub(crate) const ALIAS_MARK: &str = "#lane=";

impl LaneSt {
    /// A lane in its initial, unassigned state, whose facts tick `facts`, the
    /// engine's clock.
    pub(crate) fn new(
        identity: String,
        path: PathBuf,
        branch: Option<String>,
        present: bool,
        facts: crate::units::FactsClock,
    ) -> Self {
        Self {
            identity,
            path,
            branch,
            present,
            record: None,
            unassigned: None,
            config: ConfigState::Missing,
            env: None,
            overrides: EnvMap::new(),
            profiles: Vec::new(),
            units: BTreeMap::new(),
            started_at_ms: None,
            stopping: false,
            view: None,
            allocation: None,
            infra: LaneInfra::default(),
            setup_env: EnvMap::new(),
            view_error: None,
            last_error: None,
            skipped_tasks: Vec::new(),
            usage: BTreeMap::new(),
            moves_to: None,
            phase_reasons: BTreeMap::new(),
            orphans: BTreeMap::new(),
            starts: Vec::new(),
            waiting: BTreeMap::new(),
            bringup: None,
            claiming: 0,
            claim_ran: false,
            spawning: 0,
            spawned: Arc::new(Notify::new()),
            ports_pending: false,
            host_conflicts: Vec::new(),
            refresh_pending: false,
            adhoc_gate: Arc::new(tokio::sync::RwLock::new(())),
            adhoc_cancel: None,
            closing: Closing::No,
            config_stamp: None,
            config_files: Vec::new(),
            last_good: None,
            drift: crate::drift::Drift::default(),
            started_with: None,
            past: BTreeMap::new(),
            references: None,
            facts,
            divergent: Vec::new(),
            shutdown_hooks: false,
            wanted: false,
            sweeping: false,
            missing_since: None,
            power: None,
        }
    }

    /// The worktree's folder.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The worktree's branch; `None` when detached or not a Git worktree.
    pub(crate) fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Whether the worktree's folder exists.
    pub(crate) fn present(&self) -> bool {
        self.present
    }

    /// Records where the worktree is, its branch and whether its folder exists.
    pub(crate) fn set_place(&mut self, path: PathBuf, branch: Option<String>, present: bool) {
        self.path = path;
        self.branch = branch;
        self.present = present;
        self.facts.tick();
    }

    /// Records that the worktree's folder is gone; returns whether it was there.
    pub(crate) fn mark_missing(&mut self) -> bool {
        let was = std::mem::replace(&mut self.present, false);
        self.facts.tick();
        was
    }

    /// The lane's record; `None` while unassigned.
    pub(crate) fn record(&self) -> Option<&Lane> {
        self.record.as_ref()
    }

    /// Replaces the lane's record.
    pub(crate) fn set_record(&mut self, record: Option<Lane>) {
        self.record = record;
        self.facts.tick();
    }

    /// The lane's port block and offsets; `None` while it has none.
    pub(crate) fn allocation(&self) -> Option<&Allocation> {
        self.allocation.as_ref()
    }

    /// Replaces the lane's port block and offsets.
    pub(crate) fn set_allocation(&mut self, allocation: Option<Allocation>) {
        self.allocation = allocation;
        self.facts.tick();
    }

    /// The project file as of the last reload.
    pub(crate) fn config(&self) -> &ConfigState {
        &self.config
    }

    /// Stores the project file a reload read, with what its expressions read.
    pub(crate) fn set_config(&mut self, config: ConfigState) {
        self.references = config
            .loaded()
            .map(|config| Arc::new(devlanes_resolve::References::of(config.as_ref())));
        self.config = config;
        self.facts.tick();
    }

    /// What the expressions of the loaded config read of `lanes.main` and
    /// `projects`; `None` while no config is loaded.
    pub(crate) fn references(&self) -> Option<&Arc<devlanes_resolve::References>> {
        self.references.as_ref()
    }

    /// The lane's layout; `None` before it first resolved or once it no longer does.
    pub(crate) fn view(&self) -> Option<&Arc<LaneView>> {
        self.view.as_ref()
    }

    /// Replaces the lane's layout.
    pub(crate) fn set_view(&mut self, view: Option<Arc<LaneView>>) {
        self.view = view;
        self.facts.tick();
    }

    /// The client environment of the lane's last start; `None` before one.
    pub(crate) fn env(&self) -> Option<&Arc<EnvMap>> {
        self.env.as_ref()
    }

    /// Replaces the lane's client environment.
    pub(crate) fn set_env(&mut self, env: Arc<EnvMap>) {
        self.env = Some(env);
        self.facts.tick();
    }

    /// The lane's `--env` overrides.
    pub(crate) fn overrides(&self) -> &EnvMap {
        &self.overrides
    }

    /// Replaces the lane's `--env` overrides.
    pub(crate) fn set_overrides(&mut self, overrides: EnvMap) {
        self.overrides = overrides;
        self.facts.tick();
    }

    /// Profiles of the last start, in the order given; empty means the default.
    pub(crate) fn profiles(&self) -> &[String] {
        &self.profiles
    }

    /// Replaces the lane's profiles.
    pub(crate) fn set_profiles(&mut self, profiles: Vec<String>) {
        self.profiles = profiles;
        self.facts.tick();
    }

    /// The lane's units that ran under this daemon, by unit name: units of the
    /// current config, and removed ones while they run.
    pub(crate) fn units(&self) -> &BTreeMap<String, UnitRec> {
        &self.units
    }

    /// Records a run of `name`, replacing the unit's previous one. The one
    /// place a run is inserted: the unit is no longer a past one, nor left out.
    pub(crate) fn record_unit(&mut self, name: String, rec: UnitRec) {
        self.past.remove(&name);
        if let Some(started) = &mut self.started_with {
            started.left_out.remove(&name);
        }
        self.units.insert(name, rec);
        self.facts.tick();
    }

    /// The record of `name`, to change in place.
    pub(crate) fn unit_mut(&mut self, name: &str) -> Option<&mut UnitRec> {
        // Under the lane lock the caller holds, so a pass that reads the new
        // count also sees the change.
        self.facts.tick();
        self.units.get_mut(name)
    }

    /// Forgets every unit and orphan, as a lane whose records are released does.
    pub(crate) fn clear_units(&mut self) {
        self.units.clear();
        self.past.clear();
        self.orphans.clear();
        self.facts.tick();
    }

    /// The last runs of services the config no longer has, by unit name.
    pub(crate) fn past(&self) -> &BTreeMap<String, UnitRec> {
        &self.past
    }

    /// The record of `name`, or its last run when the config dropped it.
    pub(crate) fn unit_or_past(&self, name: &str) -> Option<&UnitRec> {
        self.units.get(name).or_else(|| self.past.get(name))
    }

    /// Moves the records of services that no longer run and that the current
    /// layout does not lay out to [`Self::past`], with their usage. Checked
    /// against the lane as it is now, so a run spawned meanwhile stays.
    pub(crate) fn retire_dropped(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let dropped: Vec<String> = self
            .units
            .iter()
            .filter(|(name, rec)| {
                rec.kind == Kind::Service
                    && !rec.host.is_alive()
                    && !view.services.iter().any(|service| service.unit == **name)
            })
            .map(|(name, _)| name.clone())
            .collect();
        if dropped.is_empty() {
            return;
        }
        for name in dropped {
            if let Some(rec) = self.units.remove(&name) {
                self.usage.remove(&name);
                self.past.insert(name, rec);
            }
        }
        self.facts.tick();
    }

    /// The lane's drift as its last pass applied it.
    pub(crate) fn drift(&self) -> &crate::drift::Drift {
        &self.drift
    }

    /// Stores a drift pass's result; returns whether what clients see changed.
    pub(crate) fn apply_drift(&mut self, drift: crate::drift::Drift) -> bool {
        let changed = !self.drift.shows_like(&drift);
        self.drift = drift;
        changed
    }

    /// The project file changed what this lane runs: some unit has drift, the
    /// infra settings changed, the running units could not be compared with the
    /// file, or a start still running planned from a config the file no longer
    /// matches.
    pub(crate) fn config_changed(&self) -> bool {
        !self.drift.units.is_empty()
            || self.drift.infra_changed
            || self.drift.unplanned.is_some()
            || self.starts.iter().any(|start| {
                start
                    .planned_from()
                    .is_some_and(|used| !crate::drift::same_config(self.config.loaded(), used))
            })
    }

    /// Previous daemons' processes still running, by unit name.
    pub(crate) fn orphans(&self) -> &BTreeMap<String, Orphan> {
        &self.orphans
    }

    /// Records a previous daemon's process of `name` unless one is recorded.
    pub(crate) fn insert_orphan(&mut self, name: String, orphan: Orphan) {
        self.orphans.entry(name).or_insert(orphan);
        self.facts.tick();
    }

    /// Forgets the orphan `name`, once it ended or was stopped.
    pub(crate) fn remove_orphan(&mut self, name: &str) -> Option<Orphan> {
        let removed = self.orphans.remove(name);
        self.facts.tick();
        removed
    }

    /// Units a start was asked for and has not handed to the start gate or
    /// spawned yet.
    pub(crate) fn waiting(&self) -> &BTreeMap<String, Waiting> {
        &self.waiting
    }

    /// The waiting units, to add or remove some.
    pub(crate) fn waiting_mut(&mut self) -> &mut BTreeMap<String, Waiting> {
        // Under the lane lock, as in `unit_mut`.
        self.facts.tick();
        &mut self.waiting
    }

    /// What the lane's last whole start brought up, which drift compares against.
    pub(crate) fn started_with(&self) -> Option<&crate::drift::StartedWith> {
        self.started_with.as_ref()
    }

    /// Replaces what the lane's last whole start brought up.
    pub(crate) fn set_started_with(&mut self, started: Option<crate::drift::StartedWith>) {
        self.started_with = started;
        self.facts.tick();
    }

    /// Adds the units among `units` that have no record to the units the
    /// lane's last whole start left out, once its services phase ended.
    pub(crate) fn leave_out_unrecorded(&mut self, units: impl IntoIterator<Item = String>) {
        let Some(started) = &mut self.started_with else {
            return;
        };
        started.left_out.extend(
            units
                .into_iter()
                .filter(|unit| !self.units.contains_key(unit)),
        );
        self.facts.tick();
    }

    /// For an extra lane of another worktree's folder (`lane new --name`): the
    /// base worktree's identity and this lane's name.
    pub(crate) fn alias(&self) -> Option<(&str, &str)> {
        self.identity.rsplit_once(ALIAS_MARK)
    }

    /// The units a selector can name in this lane, with their tags: units that
    /// ran, the task units of starts in flight (one may run before the lane
    /// records it), configured services and orphans. `stop`, `restart`, `reload`
    /// and the lane check of a lone selector all decide by this list.
    pub(crate) fn unit_candidates(&self) -> Vec<(String, Vec<String>)> {
        let mut candidates: Vec<(String, Vec<String>)> = self
            .units
            .iter()
            .map(|(name, rec)| (name.clone(), rec.tags.clone()))
            .collect();
        let tasks: BTreeSet<String> = self
            .starts
            .iter()
            .flat_map(|start| start.task_units())
            .collect();
        let services = self
            .view
            .iter()
            .flat_map(|view| view.services.iter())
            .map(|service| (service.unit.clone(), service.tags.clone()));
        let others = tasks
            .into_iter()
            .map(|name| (name, Vec::new()))
            .chain(services)
            .chain(self.orphans.keys().map(|name| (name.clone(), Vec::new())));
        for (name, tags) in others {
            if !candidates.iter().any(|(known, _)| *known == name) {
                candidates.push((name, tags));
            }
        }
        candidates
    }

    /// The units of this lane `selectors` match, with a service name standing for
    /// its replicas, including one the file no longer has that still ran (`api.3`
    /// after `replicas: 2`); `not_found` names the first selector that matches none
    /// and lists the candidates as `<reference>:<unit>`.
    pub(crate) fn match_units(
        &self,
        reference: &str,
        selectors: &[String],
    ) -> Result<Vec<String>, RpcError> {
        let configured = self.view.iter().flat_map(|view| {
            view.services
                .iter()
                .map(|s| (s.name.as_str(), s.unit.as_str()))
        });
        let leftover = self.units.keys().filter_map(|unit| {
            let (base, _) = unit.rsplit_once('.')?;
            let service = self
                .view
                .as_ref()?
                .services
                .iter()
                .find(|s| s.name == base && s.unit != s.name)?;
            Some((service.name.as_str(), unit.as_str()))
        });
        let services = configured.chain(leftover);
        let selectors = crate::engine::expand_replicas(selectors, services);
        crate::engine::match_units(&selectors, &self.unit_candidates(), reference)
    }

    /// Whether status, lane lists and events show this lane: it has a lane
    /// record, or it waits for one (`lanes.max`, a full port pool, a config
    /// error) in its present folder because devlanes ran there or `lane new`
    /// added it. A worktree that merely exists, or whose folder is gone without
    /// a lane, is not a lane.
    pub(crate) fn listed(&self) -> bool {
        self.record.is_some() || (self.present && (self.wanted || self.alias().is_some()))
    }

    /// Lane index while assigned.
    pub(crate) fn index(&self) -> Option<u32> {
        self.record.as_ref().map(|lane| lane.index)
    }

    /// True while any unit's process has not exited.
    pub(crate) fn any_alive(&self) -> bool {
        self.units.values().any(|unit| unit.host.is_alive())
    }

    /// True while a supervised unit or a previous daemon's orphan runs here.
    pub(crate) fn any_running(&self) -> bool {
        self.any_alive() || !self.orphans.is_empty()
    }

    /// The config the lane's layout and commands outside a unit use: the loaded
    /// one, else, while something runs, the last good one, so running units keep
    /// what they started with while the file is broken. `reference` names the
    /// lane in an `elsewhere` error.
    pub(crate) fn effective_config(
        &self,
        reference: Option<&str>,
    ) -> Result<Arc<ProjectConfig>, RpcError> {
        let kept = self.last_good.clone().filter(|_| self.any_running());
        match (&self.config, kept) {
            (ConfigState::Loaded(config), _) => Ok(config.clone()),
            (_, Some(config)) => Ok(config),
            (ConfigState::Invalid(error), None) => Err(error.clone()),
            (ConfigState::Missing, None) => {
                Err(RpcError::new("no_config", json!({"path": self.path}), 1))
            }
            (ConfigState::Elsewhere(other), None) => Err(elsewhere_error(other, reference)),
        }
    }

    /// The lane's live infra.
    pub(crate) fn infra(&self) -> &LaneInfra {
        &self.infra
    }

    /// Stores a full read ([`LaneInfra::replace`]). Returns whether the
    /// namespace changed.
    pub(crate) fn replace_infra(&mut self, read: InfraRead) -> bool {
        let changed = self.infra.replace(read);
        self.claim_containers();
        if changed {
            self.facts.tick();
        }
        changed
    }

    /// Stores `keys` of a read ([`LaneInfra::merge`]). Returns whether the
    /// namespace changed.
    pub(crate) fn merge_infra(&mut self, read: &InfraRead, keys: &BTreeSet<String>) -> bool {
        let changed = self.infra.merge(read, keys);
        self.claim_containers();
        // Only the namespace reaches a plan.
        if changed {
            self.facts.tick();
        }
        changed
    }

    /// Keeps the declared names of a lane whose infra never ran.
    pub(crate) fn note_infra_declared(&mut self, declared: BTreeSet<String>) {
        self.infra.note_declared(declared);
    }

    /// Records observations ([`LaneInfra::apply_observed`]). Returns the keys
    /// that run with other runs than stored.
    pub(crate) fn apply_infra_observed(
        &mut self,
        observed: &BTreeMap<String, crate::integrations::Observation>,
        began: tokio::time::Instant,
    ) -> BTreeSet<String> {
        self.infra.apply_observed(observed, began)
    }

    /// `keys` were asked about and got no answer.
    pub(crate) fn infra_unanswered(
        &mut self,
        keys: &BTreeSet<String>,
        began: tokio::time::Instant,
    ) {
        self.infra.mark_unknown(keys, began);
    }

    /// Stores the lane's infra containers alone, such as the ones a failed
    /// `up` left running.
    pub(crate) fn store_infra_containers(
        &mut self,
        containers: Vec<crate::integrations::InfraContainer>,
    ) {
        self.infra.store_containers(containers);
        self.claim_containers();
    }

    /// Applies a container event ([`LaneInfra::apply_event`]).
    pub(crate) fn apply_infra_event(
        &mut self,
        event: &crate::integrations::InfraEvent,
        now: tokio::time::Instant,
    ) -> EventEffect {
        self.infra.apply_event(event, now)
    }

    /// Events may have been lost: every key of a lane that stores its infra is
    /// checked again. Returns whether the lane stores its infra.
    pub(crate) fn suspect_all_infra(&mut self, now: tokio::time::Instant) -> bool {
        self.infra.suspect_all(now)
    }

    /// Confirms `runs` of `key` healthy.
    pub(crate) fn mark_infra_settled(
        &mut self,
        key: &str,
        runs: &[crate::integrations::ContainerRun],
    ) {
        self.infra.mark_settled(key, runs);
    }

    /// Schedules the background infra refresh unless one is scheduled.
    pub(crate) fn queue_infra_refresh(&mut self) -> bool {
        self.infra.queue_refresh()
    }

    /// The scheduled infra refresh started.
    pub(crate) fn infra_refresh_taken(&mut self) {
        self.infra.refresh_taken();
    }

    /// A claim counts as having run something once one of the lane's
    /// containers holds a port of the lane.
    fn claim_containers(&mut self) {
        if self.claiming > 0
            && self
                .infra
                .containers()
                .iter()
                .any(|container| container.holds_port())
        {
            self.claim_ran = true;
        }
    }

    /// `ports_pending` and `host_conflict` warnings for the lane's summary.
    pub(crate) fn warnings(&self, reference: &str) -> Vec<devlanes_proto::LaneWarning> {
        let mut warnings = Vec::new();
        if self.ports_pending {
            warnings.push(devlanes_proto::LaneWarning {
                code: "ports_pending".into(),
                data: json!({
                    "lane": reference,
                    "message": format!(
                        "the edited config needs a different port block; running units keep their ports, and the lane gets the new block when it starts again after `devlanes stop --lane {reference}`"
                    ),
                }),
            });
        }
        warnings.extend(self.host_conflicts.iter().cloned());
        warnings.extend(self.divergent.iter().cloned());
        warnings
    }

    /// `unit_orphaned` when `name` is a previous daemon's process still running:
    /// it names the pid and the command that stops it.
    pub(crate) fn orphan_error(&self, project: &str, name: &str) -> Option<RpcError> {
        let orphan = self.orphans.get(name)?;
        let lane = self.reference(project);
        Some(RpcError::new(
            "unit_orphaned",
            json!({
                "unit": name,
                "lane": lane,
                "pid": orphan.process.pid,
                "started_at_ms": orphan.started_at_ms,
                "orphaned_since_ms": orphan.since_ms,
                "command": format!("devlanes stop {name} --lane {lane}"),
            }),
            1,
        ))
    }

    /// True while any service unit is alive.
    pub(crate) fn services_alive(&self) -> bool {
        self.units
            .values()
            .any(|unit| unit.kind == Kind::Service && unit.host.is_alive())
    }

    /// Canonical reference: `project@index`; while unassigned, an extra lane's
    /// `project@<name>`, else `project@<dir>`.
    pub(crate) fn reference(&self, project: &str) -> String {
        match (self.index(), self.alias()) {
            (Some(index), _) => format!("{project}@{index}"),
            (None, Some((_, name))) => format!("{project}@{name}"),
            (None, None) => format!("{project}@{}", dir_name(&self.path)),
        }
    }

    /// Lifecycle word for `LaneSummary.state`: `starting` while a start runs
    /// its phases ([`Self::starts_in_phases`]) or a service is still starting. Queued services make it
    /// `starting` too, in [`crate::engine::Inner::fill_queued`], since the lane does not
    /// see the gate.
    pub(crate) fn state_word(&self) -> &'static str {
        if self.record.is_none()
            && matches!(
                self.config,
                ConfigState::Missing | ConfigState::Elsewhere(_)
            )
        {
            return "no-config";
        }
        if self.record.is_none() {
            return "unassigned";
        }
        if self.stopping {
            return "stopping";
        }
        if self.starts_in_phases() > 0 {
            return "starting";
        }
        let failed = self.units.values().any(|unit| {
            unit.kind == Kind::Service && matches!(unit.host.state(), UnitState::Failed(_))
        });
        if failed {
            return "failed";
        }
        // `--all-at-once`, the end of a gated start and a start that returned on
        // its timeout leave no start running while services still get ready.
        let starting = self.units.values().any(|unit| {
            unit.kind == Kind::Service && matches!(unit.host.state(), UnitState::Starting)
        });
        if starting {
            return "starting";
        }
        if self.any_running() {
            "running"
        } else {
            "idle"
        }
    }

    /// Starts that hold the lane's lease and have not ended their phases. A
    /// start counts itself while it runs.
    pub(crate) fn starts_in_phases(&self) -> usize {
        self.starts.iter().filter(|start| start.in_phases()).count()
    }

    /// Whether a stop of the whole lane or a start in its phases is under way.
    pub(crate) fn is_busy(&self) -> bool {
        self.stopping || self.starts_in_phases() > 0
    }

    /// Whether `start`/`resume` from a shell is needed before services can run.
    pub(crate) fn needs_env(&self) -> bool {
        self.env.is_none()
            && !self.any_running()
            && self
                .record
                .as_ref()
                .is_some_and(|lane| lane.desired_state == DesiredState::Running)
    }
}

/// Color of a unit without config tags of its own (exec units, containers,
/// infra commands): from its name and the project's `tags:` colors.
pub(crate) fn derived_color(lane: &LaneSt, name: &str) -> String {
    let overrides = lane
        .config
        .loaded()
        .map(|config| devlanes_resolve::tag_colors(config))
        .unwrap_or_default();
    devlanes_core::color::unit_color(&[], name, None, &overrides).to_owned()
}

/// Directory name used in references of unassigned lanes.
pub(crate) fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One worktree's lane plus the lock that orders lifecycle operations on it.
pub(crate) struct LaneRt {
    pub worktree: WorktreeId,
    /// Starts that add units to a lane hold it shared, so they never wait for
    /// each other. Bringing the lane up (until its services phase), whole-lane
    /// stops, clean, reset, `infra up` and block moves hold it exclusively.
    pub op: tokio::sync::RwLock<()>,
    pub st: Mutex<LaneSt>,
    /// Held while a `lane.changed` summary is built and emitted, so summaries
    /// reach the bus in the order they were taken.
    pub publish: Mutex<()>,
    /// Stops the config watcher when the lane goes away.
    pub watch: tokio_util::sync::CancellationToken,
    /// Serializes freezing, throttling and thawing of the lane. Taken after `op`
    /// when both are needed, never before.
    pub power: tokio::sync::Mutex<()>,
    /// Serializes read-modify-write updates of the lane's saved services, which
    /// starts sharing `op` would otherwise interleave.
    pub desired: tokio::sync::Mutex<()>,
    /// Held by a start while it decides whether it brings the lane up, through
    /// its wait for `op`. A second start waits here instead of in `op`'s queue,
    /// where an exclusive request would hold up every shared one behind it.
    pub decide: tokio::sync::Mutex<()>,
    /// Held while a start checks and runs setup entries, so two starts never
    /// run the same entry at once.
    pub setup: tokio::sync::Mutex<()>,
    /// The lane's single-flight drift job.
    pub drift_job: crate::drift::DriftJob,
}

impl LaneRt {
    /// Locks the lane state for a short critical section.
    pub(crate) fn st(&self) -> MutexGuard<'_, LaneSt> {
        lock(&self.st)
    }

    /// Interrupts every start running on this lane. Called before waiting for
    /// `op`, which those starts hold.
    pub(crate) fn cancel_start(&self) {
        for start in &self.st().starts {
            start.cancel.cancel();
        }
    }

    /// The starts running on this lane, oldest first.
    pub(crate) fn running_starts(&self) -> Vec<Arc<StartCtl>> {
        self.st().starts.clone()
    }

    /// Installs a start's control for as long as the returned slot lives. Called
    /// before the start waits for `op`, so stops reach a start that waits too.
    pub(crate) fn begin_start(
        self: &Arc<Self>,
        shutdown: &CancellationToken,
        start_id: u64,
    ) -> StartSlot {
        let ctl = Arc::new(StartCtl::new(shutdown, start_id));
        self.st().starts.push(ctl.clone());
        StartSlot {
            lane: self.clone(),
            ctl,
        }
    }

    /// Counts a spawn in flight until the guard drops.
    pub(crate) fn spawn_guard(self: &Arc<Self>) -> SpawnGuard {
        self.st().spawning += 1;
        SpawnGuard(self.clone())
    }

    /// For `exec` and `run`: waits out a clean or reset of this lane, then holds
    /// the ad-hoc gate shared. The token fires when a clean or reset of the lane
    /// begins, or with `shutdown`.
    pub(crate) async fn enter_adhoc(
        &self,
        shutdown: &CancellationToken,
    ) -> (tokio::sync::OwnedRwLockReadGuard<()>, CancellationToken) {
        loop {
            let gate = self.st().adhoc_gate.clone();
            let guard = gate.read_owned().await;
            {
                let mut st = self.st();
                if st.closing == Closing::No {
                    let parent = st
                        .adhoc_cancel
                        .get_or_insert_with(|| shutdown.child_token());
                    return (guard, parent.child_token());
                }
            }
            // A closer marked the lane but has not queued for the gate yet; once it
            // has, the next read waits for it to finish.
            drop(guard);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// For `lane clean` and `daemon reset-state`, which hold `op`: interrupts the
    /// lane's runs, then waits until no `exec` or `run` holds the ad-hoc gate.
    /// The lane opens again when the returned value drops.
    pub(crate) async fn close_adhoc(self: &Arc<Self>) -> AdhocClosed {
        let gate = {
            let mut st = self.st();
            st.closing = Closing::Hooks;
            if let Some(token) = st.adhoc_cancel.take() {
                token.cancel();
            }
            st.adhoc_gate.clone()
        };
        AdhocClosed {
            lane: self.clone(),
            _guard: gate.write_owned().await,
        }
    }

    /// Waits until no spawn is in flight on this lane.
    pub(crate) async fn spawns_settled(&self) {
        loop {
            let spawned = self.st().spawned.clone();
            let notified = spawned.notified();
            tokio::pin!(notified);
            // Registered before the check, so a drop in between still wakes us.
            notified.as_mut().enable();
            if self.st().spawning == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// A lane closed to `exec` and `run` ([`LaneRt::close_adhoc`]).
pub(crate) struct AdhocClosed {
    lane: Arc<LaneRt>,
    _guard: tokio::sync::OwnedRwLockWriteGuard<()>,
}

impl Drop for AdhocClosed {
    /// Lets `exec` and `run` in again.
    fn drop(&mut self) {
        self.lane.st().closing = Closing::No;
    }
}

/// One spawn in flight on a lane, from just before its last cancellation check
/// until its unit is recorded (or the spawn gave up).
pub(crate) struct SpawnGuard(Arc<LaneRt>);

impl Drop for SpawnGuard {
    /// Uncounts the spawn and wakes waiting stops.
    fn drop(&mut self) {
        let spawned = {
            let mut st = self.0.st();
            st.spawning = st.spawning.saturating_sub(1);
            st.spawned.clone()
        };
        spawned.notify_waiters();
    }
}

/// What a start brought up, which its answer and its error report.
#[derive(Debug, Clone, Default)]
pub(crate) struct Brought {
    /// Service units it spawned, and units still queued at its deadline.
    pub started: Vec<String>,
    /// Setup entries (`setup.<name>`) and tasks it ran to success.
    pub ran: Vec<String>,
    /// It brought the lane up instead of adding units to it.
    pub whole_lane: bool,
}

/// The one record of a start (or restart), installed on its lane from the
/// request until [`crate::engine::Inner::end_start`]: its id, how to cancel
/// it, how far it got and what it brought up. A `stop` or `restart` of single
/// units reaches it here without waiting for its `op`.
pub(crate) struct StartCtl {
    /// The `start_id` of the start's events and result.
    start_id: u64,
    /// What the start brought up so far.
    brought: Mutex<Brought>,
    /// The start holds the lane's lease, so other starts and refreshes leave
    /// the block and index it plans on alone.
    holds_lease: AtomicBool,
    /// The start finished its phases; the lane no longer reads `starting`
    /// for it.
    phases_ended: AtomicBool,
    /// Ends the start as `start_cancelled`: a whole-lane stop, clean, reset or
    /// shutdown fires it, and so does a stop of the task the start runs.
    pub cancel: CancellationToken,
    /// Services units the start is about to spawn. A later start of the same
    /// unit waits for this one instead of queueing it a second time.
    spawning: Mutex<BTreeSet<String>>,
    /// How long each unit waited for a start slot, for the start's log line.
    gate_waits: Mutex<Vec<(String, u64)>>,
    /// How each planned spawn ended: `None` spawned, `Some` failed with the
    /// error. Starts that joined a unit report what this one got.
    outcomes: Mutex<BTreeMap<String, Option<RpcError>>>,
    /// Woken when a spawn's outcome is recorded or a unit is withdrawn, so a
    /// start that joined one unit goes on as soon as that unit is settled.
    pub changed: Notify,
    /// Fires once the start spawns no more services: its services phase is over
    /// or the start ended.
    pub services_done: CancellationToken,
    /// Units a stop (`"stop"`) or restart (`"restart"`) took out of the start:
    /// it neither spawns nor waits for them and reports them as stopped.
    withdrawn: Mutex<BTreeMap<String, &'static str>>,
    /// Task units the start runs right now (hook tasks, their steps, setup
    /// entries), counted.
    tasks: Mutex<BTreeMap<String, usize>>,
    /// Task units the start may still run, known once it resolved the lane. A
    /// stop can name them before (or while) they spawn.
    planned: Mutex<BTreeSet<String>>,
    /// The config the start plans from, once it took it; a reload in the
    /// middle of the start leaves it as it was.
    config: std::sync::OnceLock<Arc<ProjectConfig>>,
}

impl StartCtl {
    /// A control whose token fires with `parent`.
    pub(crate) fn new(parent: &CancellationToken, start_id: u64) -> Self {
        Self {
            start_id,
            brought: Mutex::new(Brought::default()),
            holds_lease: AtomicBool::new(false),
            phases_ended: AtomicBool::new(false),
            cancel: parent.child_token(),
            spawning: Mutex::new(BTreeSet::new()),
            gate_waits: Mutex::new(Vec::new()),
            outcomes: Mutex::new(BTreeMap::new()),
            changed: Notify::new(),
            services_done: CancellationToken::new(),
            withdrawn: Mutex::new(BTreeMap::new()),
            tasks: Mutex::new(BTreeMap::new()),
            planned: Mutex::new(BTreeSet::new()),
            config: std::sync::OnceLock::new(),
        }
    }

    /// Records the config the start plans from. The first call wins.
    pub(crate) fn plans_from(&self, config: Arc<ProjectConfig>) {
        let _ = self.config.set(config);
    }

    /// The config the start plans from, once it took one.
    pub(crate) fn planned_from(&self) -> Option<&Arc<ProjectConfig>> {
        self.config.get()
    }

    /// The `start_id` of this start.
    pub(crate) fn start_id(&self) -> u64 {
        self.start_id
    }

    /// Records a service unit the start spawned (or left queued at its deadline).
    pub(crate) fn push_started(&self, unit: String) {
        lock(&self.brought).started.push(unit);
    }

    /// Records a setup entry (`setup.<name>`) or task the start ran.
    pub(crate) fn push_ran(&self, unit: String) {
        lock(&self.brought).ran.push(unit);
    }

    /// The service units the start spawned so far.
    pub(crate) fn started(&self) -> Vec<String> {
        lock(&self.brought).started.clone()
    }

    /// Everything the start brought up so far.
    pub(crate) fn brought(&self) -> Brought {
        lock(&self.brought).clone()
    }

    /// Records that the start took the lane's lease, bringing the lane up when
    /// `whole_lane`. The lane reads `starting` for it until the returned mark
    /// drops, however the start returns.
    #[must_use = "the start's phases end when the mark drops"]
    pub(crate) fn mark_lease(self: &Arc<Self>, whole_lane: bool) -> PhasesMark {
        lock(&self.brought).whole_lane = whole_lane;
        self.holds_lease.store(true, Ordering::Release);
        PhasesMark(self.clone())
    }

    /// Records that the start finished its phases.
    fn end_phases(&self) {
        self.phases_ended.store(true, Ordering::Release);
    }

    /// Between [`Self::mark_lease`] and [`Self::end_phases`].
    fn in_phases(&self) -> bool {
        self.holds_lease.load(Ordering::Acquire) && !self.phases_ended.load(Ordering::Acquire)
    }

    /// Records how long `unit` waited for its start slot.
    pub(crate) fn record_gate_wait(&self, unit: &str, waited_ms: u64) {
        lock(&self.gate_waits).push((unit.to_owned(), waited_ms));
    }

    /// Every recorded start-slot wait, in the order the slots came.
    pub(crate) fn gate_waits(&self) -> Vec<(String, u64)> {
        lock(&self.gate_waits).clone()
    }

    /// Records how the spawn of `unit` ended: `None` when it spawned.
    pub(crate) fn record_outcome(&self, unit: &str, error: Option<RpcError>) {
        lock(&self.outcomes).insert(unit.to_owned(), error);
        self.changed.notify_waiters();
    }

    /// How the spawn of `unit` ended, once it did: `Some(None)` spawned,
    /// `Some(Some(error))` failed, `None` never tried (withdrawn or cancelled).
    pub(crate) fn outcome(&self, unit: &str) -> Option<Option<RpcError>> {
        lock(&self.outcomes).get(unit).cloned()
    }

    /// Records the service units the start is about to spawn.
    pub(crate) fn plan_spawns(&self, units: impl IntoIterator<Item = String>) {
        lock(&self.spawning).extend(units);
    }

    /// Whether the start still means to spawn `unit`: planned, not withdrawn
    /// and its services phase not over.
    pub(crate) fn will_spawn(&self, unit: &str) -> bool {
        !self.services_done.is_cancelled()
            && self.withdrawn_by(unit).is_none()
            && lock(&self.spawning).contains(unit)
    }

    /// Records task units the start may run.
    pub(crate) fn plan_tasks(&self, units: impl IntoIterator<Item = String>) {
        lock(&self.planned).extend(units);
    }

    /// Task units a stop may name: those the start runs or still plans to run,
    /// including one whose process exists but is not recorded on the lane yet.
    pub(crate) fn task_units(&self) -> BTreeSet<String> {
        let mut units = lock(&self.planned).clone();
        units.extend(lock(&self.tasks).keys().cloned());
        units
    }

    /// Takes `units` out of the start on behalf of `by`.
    pub(crate) fn withdraw(&self, units: &[String], by: &'static str) {
        {
            let mut withdrawn = lock(&self.withdrawn);
            for unit in units {
                withdrawn.entry(unit.clone()).or_insert(by);
            }
        }
        self.changed.notify_waiters();
    }

    /// Who took `unit` out of the start, if anybody did.
    pub(crate) fn withdrawn_by(&self, unit: &str) -> Option<&'static str> {
        lock(&self.withdrawn).get(unit).copied()
    }

    /// Every withdrawn unit and who withdrew it.
    pub(crate) fn withdrawn(&self) -> BTreeMap<String, &'static str> {
        lock(&self.withdrawn).clone()
    }

    /// Marks task units as running in this start until the guard drops.
    pub(crate) fn running_tasks(self: &Arc<Self>, units: Vec<String>) -> TaskGuard {
        let mut tasks = lock(&self.tasks);
        for unit in &units {
            *tasks.entry(unit.clone()).or_default() += 1;
        }
        TaskGuard {
            ctl: self.clone(),
            units,
        }
    }

    /// Whether the start runs a task unit named `unit` right now.
    pub(crate) fn runs_task(&self, unit: &str) -> bool {
        lock(&self.tasks).contains_key(unit)
    }
}

/// A start in its phases, until dropped ([`StartCtl::mark_lease`]).
pub(crate) struct PhasesMark(Arc<StartCtl>);

impl Drop for PhasesMark {
    /// Ends the start's phases.
    fn drop(&mut self) {
        self.0.end_phases();
    }
}

/// Task units a start runs, until dropped ([`StartCtl::running_tasks`]).
pub(crate) struct TaskGuard {
    ctl: Arc<StartCtl>,
    units: Vec<String>,
}

impl Drop for TaskGuard {
    /// Uncounts the units.
    fn drop(&mut self) {
        let mut tasks = lock(&self.ctl.tasks);
        for unit in &self.units {
            if let Some(count) = tasks.get_mut(unit) {
                *count -= 1;
                if *count == 0 {
                    tasks.remove(unit);
                }
            }
        }
    }
}

/// A start's control installed on its lane ([`LaneRt::begin_start`]). A start
/// ends by handing it to [`crate::engine::Inner::end_start`].
pub(crate) struct StartSlot {
    lane: Arc<LaneRt>,
    pub ctl: Arc<StartCtl>,
}

impl Drop for StartSlot {
    /// Uninstalls the control and releases restarts waiting for the services
    /// phase. Emits nothing: `end_start` tells clients, and a slot dropped
    /// anywhere else is a start unwinding.
    fn drop(&mut self) {
        self.ctl.services_done.cancel();
        self.lane
            .st()
            .starts
            .retain(|current| !Arc::ptr_eq(current, &self.ctl));
    }
}

/// Mutable state of a project.
pub(crate) struct ProjectSt {
    pub name: String,
    pub root: PathBuf,
    pub common_dir: Option<PathBuf>,
    pub primary_branch: String,
    /// Environment of the last request that carried one, for auto-started worktrees.
    pub last_env: Option<Arc<EnvMap>>,
    /// Changed only through [`ProjectRt::insert_lane`] and
    /// [`ProjectRt::remove_lane`], since other lanes' plans read it.
    lanes: BTreeMap<WorktreeId, Arc<LaneRt>>,
    /// The project file's directory relative to each worktree root.
    pub config_dir: PathBuf,
    /// The project file's name in that directory, `devlanes.yaml` unless `--file`
    /// registered another one.
    pub config_file: String,
}

/// A project: the worktrees of one repository (or one non-git root) whose project
/// file names it. Its name is its identity.
pub(crate) struct ProjectRt {
    pub id: ProjectId,
    /// Serializes changes to the project's lanes: re-listing, registration and
    /// assignment, `lane new`'s name check and insert, and a clean's release of
    /// its lane. Taken after a lane's `op`, never before.
    pub sync: tokio::sync::Mutex<()>,
    /// Orders the project's infra coming up against its shared containers going
    /// down, for Compose and plain containers alike. Every `InfraProvider::up`
    /// holds it shared, so starts of different lanes stay parallel; every
    /// `InfraProvider::down`, and the "no lane uses them" decision before it,
    /// hold it exclusively ([`crate::shared_infra`]). Taken after a lane's `op`,
    /// never before; never held while taking `sync`, and `sync` is never held
    /// while taking it.
    pub shared_infra: tokio::sync::RwLock<()>,
    pub st: Mutex<ProjectSt>,
    pub watching: AtomicBool,
    pub watch: tokio_util::sync::CancellationToken,
    /// The engine's clock, ticked by changes to the lane map and config path.
    pub facts: crate::units::FactsClock,
}

impl ProjectSt {
    /// A project's state with `lanes`.
    pub(crate) fn new(
        name: String,
        root: PathBuf,
        common_dir: Option<PathBuf>,
        primary_branch: String,
        lanes: BTreeMap<WorktreeId, Arc<LaneRt>>,
        config_dir: PathBuf,
        config_file: String,
    ) -> Self {
        Self {
            name,
            root,
            common_dir,
            primary_branch,
            last_env: None,
            lanes,
            config_dir,
            config_file,
        }
    }

    /// The lane runtime of `worktree`.
    pub(crate) fn lane(&self, worktree: &WorktreeId) -> Option<&Arc<LaneRt>> {
        self.lanes.get(worktree)
    }
}

impl ProjectRt {
    /// Puts `lane` in the project's map as the runtime of `worktree`.
    pub(crate) fn insert_lane(&self, worktree: WorktreeId, lane: Arc<LaneRt>) {
        self.st().lanes.insert(worktree, lane);
        self.facts.tick();
    }

    /// Takes the runtime of `worktree` out of the project's map.
    pub(crate) fn remove_lane(&self, worktree: &WorktreeId) {
        self.st().lanes.remove(worktree);
        self.facts.tick();
    }

    /// Locks the project state for a short critical section.
    pub(crate) fn st(&self) -> MutexGuard<'_, ProjectSt> {
        lock(&self.st)
    }

    /// Display name from the config.
    pub(crate) fn name(&self) -> String {
        self.st().name.clone()
    }

    /// The project file's directory relative to each worktree root, and its name.
    pub(crate) fn config_path(&self) -> (PathBuf, String) {
        let st = self.st();
        (st.config_dir.clone(), st.config_file.clone())
    }

    /// Records where the project file now is.
    pub(crate) fn set_config_path(&self, dir: PathBuf, file: String) {
        {
            let mut st = self.st();
            st.config_dir = dir;
            st.config_file = file;
        }
        self.facts.tick();
    }

    /// Lanes in worktree order.
    pub(crate) fn lanes(&self) -> Vec<Arc<LaneRt>> {
        self.st().lanes.values().cloned().collect()
    }

    /// Claims the watcher slot; true for the first caller only.
    pub(crate) fn claim_watch(&self) -> bool {
        !self.watching.swap(true, Ordering::SeqCst)
    }

    /// The lane holding `index`, if any.
    pub(crate) fn lane_by_index(&self, index: u32) -> Option<Arc<LaneRt>> {
        self.lanes()
            .into_iter()
            .find(|lane| lane.st().index() == Some(index))
    }
}

/// Converts proto attribution to the core type stored in SQLite.
pub(crate) fn core_actor(actor: &Actor) -> devlanes_core::Actor {
    devlanes_core::Actor {
        kind: match actor.kind {
            ActorKind::Human => devlanes_core::ActorKind::Human,
            ActorKind::Agent => devlanes_core::ActorKind::Agent,
        },
        agent_id: actor.agent_id.clone(),
        session_id: actor.session_id.clone(),
    }
}

/// Converts a stored actor back to the proto type.
pub(crate) fn proto_actor(actor: &devlanes_core::Actor) -> Actor {
    Actor {
        kind: match actor.kind {
            devlanes_core::ActorKind::Human => ActorKind::Human,
            devlanes_core::ActorKind::Agent => ActorKind::Agent,
        },
        agent_id: actor.agent_id.clone(),
        session_id: actor.session_id.clone(),
    }
}

/// True when `unit` was started by the caller's agent session (`--mine`). An agent
/// that exposes no session id counts all its sessions as one.
pub(crate) fn is_mine(owner: &Actor, caller: &Actor) -> bool {
    match (&owner.session_id, &caller.session_id) {
        (Some(owner), Some(caller)) => owner == caller,
        (None, None) => owner.kind == caller.kind && owner.agent_id == caller.agent_id,
        _ => false,
    }
}

/// Gives a unit that is not running the reason its last start stopped short, so
/// clients can say why it stayed idle without looking at the lane.
pub(crate) fn apply_phase_reason(lane: &LaneSt, summary: &mut UnitSummary) {
    let running = lane
        .units
        .get(&summary.name)
        .is_some_and(|unit| unit.host.is_alive());
    if !running && let Some(reason) = lane.phase_reasons.get(&summary.name) {
        summary.reason = Some(reason.clone());
    }
}

/// Sets `UnitSummary.order`: where the unit is declared in its list of the
/// lane's config (`services`, `tasks`, `infra.setup`, `infra.teardown` or
/// `infra.external`). A replica `name.N` takes its service's place.
pub(crate) fn apply_order(lane: &LaneSt, summary: &mut UnitSummary) {
    let Some(config) = lane.config.loaded() else {
        return;
    };
    let name = summary.name.as_str();
    let service = config.services.get_index_of(name).or_else(|| {
        let (base, replica) = name.rsplit_once('.')?;
        replica.parse::<u16>().ok()?;
        config.services.get_index_of(base)
    });
    let index = match summary.role.as_deref() {
        Some("setup") => name
            .strip_prefix("setup.")
            .and_then(|entry| config.infra.setup.get_index_of(entry)),
        Some("teardown") => name
            .strip_prefix("teardown.")
            .and_then(|entry| config.infra.teardown.get_index_of(entry)),
        _ if summary.kind == "task" => config.tasks.get_index_of(name),
        _ if summary.kind == "service" => service,
        _ if summary.kind == devlanes_proto::EXTERNAL_KIND => {
            config.infra.external.get_index_of(name)
        }
        _ => None,
    };
    summary.order = index.and_then(|index| u32::try_from(index).ok());
}

/// Builds the summary of one live or finished unit.
pub(crate) fn unit_summary(lane: &str, name: &str, unit: &UnitRec) -> UnitSummary {
    let state = unit.host.state();
    let exit = unit.host.exit_outcome();
    let reason = match &state {
        UnitState::Failed(reason) => reason.clone(),
        _ => None,
    };
    let info = unit.host.info();
    let leftovers = unit.leftovers(name);
    UnitSummary {
        name: name.to_owned(),
        lane: lane.to_owned(),
        state: state.word().into(),
        pid: unit.host.identity().map(|id| id.pid),
        exit_code: exit.and_then(|outcome| outcome.code),
        ports: unit.ports.clone(),
        urls: unit.urls.clone(),
        actor: info.actor.clone(),
        reason,
        kind: unit.kind.word().into(),
        autostart: unit.autostart,
        hidden: unit.hidden,
        tags: unit.tags.clone(),
        started_at_ms: Some(info.started_at_ms),
        finished_at_ms: unit.host.ended_at_ms(),
        cpu_percent: None,
        memory_bytes: None,
        color: unit.color.clone(),
        orphaned_since_ms: None,
        left_running: leftovers.left_running,
        escaped: leftovers.escaped,
        display_name: unit.display_name.clone(),
        ready_check: info.goal.check().map(str::to_owned),
        pty: info.pty,
        role: Some(unit.role.to_owned()),
        order: None,
        public_url: unit
            .urls
            .iter()
            .find(|url| url.starts_with("https://"))
            .cloned(),
        power: None,
        drift: None,
    }
}

/// Summary of a previous daemon's process: `orphaned`, with the configured
/// service's ports and URLs when it is one.
pub(crate) fn orphan_summary(
    lane: &LaneSt,
    reference: &str,
    name: &str,
    orphan: &Orphan,
) -> UnitSummary {
    let service = lane
        .view
        .as_ref()
        .and_then(|view| view.services.iter().find(|s| s.unit == name));
    let mut summary = match service {
        Some(service) => idle_summary(reference, service),
        None => {
            let task = lane
                .config
                .loaded()
                .is_some_and(|config| config.tasks.contains_key(name));
            let kind = if task { "task" } else { "exec" };
            // A previous daemon's setup or teardown entry is known only by its name.
            let role = match name.split_once('.') {
                Some(("setup", _)) => "setup",
                Some(("teardown", _)) => "teardown",
                _ => kind,
            };
            UnitSummary {
                name: name.to_owned(),
                lane: reference.to_owned(),
                role: Some(role.into()),
                kind: kind.into(),
                color: Some(derived_color(lane, name)),
                ..UnitSummary::default()
            }
        }
    };
    summary.state = devlanes_proto::ORPHANED.into();
    summary.pid = Some(orphan.process.pid);
    summary.actor = orphan.actor.clone();
    summary.started_at_ms = Some(orphan.started_at_ms);
    summary.orphaned_since_ms = Some(orphan.since_ms);
    summary
}

/// Summary of a configured service that has not run since the daemon started.
pub(crate) fn idle_summary(lane: &str, service: &ServiceInfo) -> UnitSummary {
    let mut ports = service.ports.clone();
    ports.insert("default".into(), service.port);
    let mut urls = vec![service.url.clone()];
    urls.extend(service.public_url.clone());
    UnitSummary {
        name: service.unit.clone(),
        lane: lane.to_owned(),
        state: "idle".into(),
        ports,
        urls,
        kind: "service".into(),
        autostart: service.autostart,
        hidden: service.hidden,
        tags: service.tags.clone(),
        color: Some(service.color.clone()),
        display_name: service.display_name.clone(),
        role: Some("service".into()),
        public_url: service.public_url.clone(),
        ..UnitSummary::default()
    }
}

/// Summary of an `infra.external` entry: kind, role and state `external`
/// whether or not anything listens on its port, which devlanes never checks.
pub(crate) fn external_summary(
    lane: &LaneSt,
    reference: &str,
    external: &ExternalInfo,
) -> UnitSummary {
    let public_url = external.public_url();
    let mut urls = vec![external.url.clone()];
    urls.extend(public_url.clone());
    UnitSummary {
        color: Some(derived_color(lane, &external.name)),
        name: external.name.clone(),
        lane: reference.to_owned(),
        state: devlanes_proto::EXTERNAL_KIND.into(),
        ports: BTreeMap::from([("default".to_owned(), external.port)]),
        urls,
        kind: devlanes_proto::EXTERNAL_KIND.into(),
        role: Some(devlanes_proto::EXTERNAL_KIND.into()),
        hidden: external.hidden,
        public_url,
        // No process, so no terminal to size.
        pty: false,
        ..UnitSummary::default()
    }
}

/// Shows a unit a start asked for, and has not queued in the start gate or
/// spawned yet, as `queued` with what it waits for (`waiting_for`): the step
/// the lane's bring-up runs, or `lane` while the start waits for its turn.
/// The gate's own `queued` data replaces this once the unit is in its line.
fn apply_waiting(lane: &LaneSt, summary: &mut UnitSummary) {
    let Some(waiting) = lane.waiting.get(&summary.name) else {
        return;
    };
    // A live process (an earlier run still stopping, or one that just spawned)
    // keeps its own state.
    if lane
        .units
        .get(&summary.name)
        .is_some_and(|rec| rec.host.is_alive())
    {
        return;
    }
    let step = lane
        .bringup
        .as_ref()
        .and_then(|bringup| bringup.step.clone())
        .unwrap_or_else(|| "lane".to_owned());
    summary.state = "queued".into();
    summary.reason = Some(RpcError::new("queued", json!({"waiting_for": step}), 0));
    summary.actor = waiting.actor.clone();
}

/// Builds a lane summary; units list configured services (idle or live),
/// one-shot units, then `infra.external` entries.
pub(crate) fn lane_summary(project: &str, lane: &LaneSt) -> LaneSummary {
    let reference = lane.reference(project);
    let mut units = Vec::new();
    if let Some(view) = &lane.view {
        for service in &view.services {
            units.push(
                match (
                    lane.orphans.get(&service.unit),
                    lane.units.get(&service.unit),
                ) {
                    (Some(orphan), _) => orphan_summary(lane, &reference, &service.unit, orphan),
                    (None, Some(unit)) => unit_summary(&reference, &service.unit, unit),
                    (None, None) => idle_summary(&reference, service),
                },
            );
        }
    }
    for (name, orphan) in &lane.orphans {
        if !units.iter().any(|summary| &summary.name == name) {
            units.push(orphan_summary(lane, &reference, name, orphan));
        }
    }
    for (name, unit) in &lane.units {
        if !units.iter().any(|summary| &summary.name == name) {
            units.push(unit_summary(&reference, name, unit));
        }
    }
    for external in lane.view.iter().flat_map(|view| &view.externals) {
        units.push(external_summary(lane, &reference, external));
    }
    let power = lane.power.as_ref().map(|power| power.level.word());
    for summary in &mut units {
        apply_phase_reason(lane, summary);
        apply_waiting(lane, summary);
        apply_order(lane, summary);
        summary.drift = lane
            .drift
            .units
            .get(&summary.name)
            .map(crate::drift::UnitDrift::wire);
        // Only supervised processes are throttled or frozen; orphans and finished
        // units keep their own state word.
        if summary.state == "ready" && lane.units.contains_key(&summary.name) {
            summary.power = power.map(str::to_owned);
        }
    }
    let missing =
        (!lane.present).then(|| RpcError::new("worktree_missing", json!({"path": lane.path}), 3));
    let reason = missing
        .or_else(|| lane.unassigned.clone())
        .or_else(|| match &lane.config {
            ConfigState::Invalid(error) => Some(error.clone()),
            ConfigState::Elsewhere(other) => Some(elsewhere_error(other, Some(&reference))),
            _ => None,
        })
        .or_else(|| lane.view_error.clone())
        .or_else(|| lane.last_error.clone());
    let profiles = if lane.profiles.is_empty() {
        lane.record
            .as_ref()
            .map(|r| r.profiles.clone())
            .unwrap_or_default()
    } else {
        lane.profiles.clone()
    };
    LaneSummary {
        warnings: lane.warnings(&reference),
        reference,
        project: project.to_owned(),
        index: lane.index(),
        branch: lane.branch.clone(),
        name: lane.alias().map(|(_, name)| name.to_owned()),
        path: lane.path.clone(),
        state: lane.state_word().into(),
        config_changed: lane.config_changed(),
        infra_changed: lane.drift.infra_changed,
        units,
        profile: profiles.first().cloned(),
        profiles,
        moves_to: lane.moves_to,
        domain: lane.view.as_ref().map(|view| view.domain.clone()),
        url: lane.view.as_ref().and_then(|view| view.url.clone()),
        port_block: lane.view.as_ref().map(|view| {
            (
                view.block.0,
                view.block.0.saturating_add(view.block.1.saturating_sub(1)),
            )
        }),
        reason,
        needs_env: lane.needs_env(),
        containers: container_summaries(lane),
        power: lane.power.as_ref().map(LanePowerSt::summary),
    }
}

/// Containers from the lane's infra namespace (`{name: {port, ports}}`), set while
/// its infra is up, with `infra.hidden` applied.
pub(crate) fn container_summaries(lane: &LaneSt) -> Vec<devlanes_proto::ContainerSummary> {
    let Some(namespace) = lane.infra().namespace() else {
        return Vec::new();
    };
    let hidden: &[String] = lane
        .config
        .loaded()
        .and_then(|config| config.infra.hidden.as_deref())
        .unwrap_or_default();
    namespace
        .iter()
        .map(|(name, entry)| {
            let mut ports: BTreeMap<String, u16> = entry
                .get("ports")
                .and_then(Value::as_object)
                .map(|ports| {
                    ports
                        .iter()
                        .filter_map(|(key, port)| {
                            let port = u16::try_from(port.as_u64()?).ok()?;
                            Some((key.clone(), port))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if ports.is_empty()
                && let Some(port) = entry
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|port| u16::try_from(port).ok())
            {
                ports.insert("default".into(), port);
            }
            devlanes_proto::ContainerSummary {
                name: name.clone(),
                ports,
                hidden: hidden.contains(name),
                color: Some(derived_color(lane, name)),
                shared: lane
                    .infra()
                    .containers()
                    .iter()
                    .any(|container| &container.name == name && container.shared),
                cpu_percent: None,
                memory_bytes: None,
                state: lane
                    .infra()
                    .containers()
                    .iter()
                    .filter(|container| &container.name == name)
                    .filter_map(|container| container.state.clone())
                    .max_by_key(|state| state_badness(state)),
                urls: lane
                    .view
                    .iter()
                    .flat_map(|view| &view.infra_routes)
                    .filter(|route| &route.container == name)
                    .filter_map(|route| route.url.clone())
                    .collect(),
            }
        })
        .collect()
}

/// The project's shared containers as some lane whose infra is up sees them.
/// Shared infra serves every lane, so lanes that are idle list them too.
pub(crate) fn shared_containers(project: &ProjectRt) -> Vec<devlanes_proto::ContainerSummary> {
    let mut shared: Vec<devlanes_proto::ContainerSummary> = Vec::new();
    for lane in project.lanes() {
        for container in container_summaries(&lane.st()) {
            if container.shared && !shared.iter().any(|known| known.name == container.name) {
                shared.push(container);
            }
        }
    }
    shared
}

/// Adds the shared containers a lane summary does not list yet. Their URLs
/// are left out: they carry the other lane's hostnames, and this lane has no
/// route of its own until its infra namespace is set.
pub(crate) fn add_shared(summary: &mut LaneSummary, shared: &[devlanes_proto::ContainerSummary]) {
    for container in shared {
        if !summary
            .containers
            .iter()
            .any(|known| known.name == container.name)
        {
            summary.containers.push(devlanes_proto::ContainerSummary {
                urls: Vec::new(),
                ..container.clone()
            });
        }
    }
}

/// How bad a container state is, so a service's replicas report the worst.
fn state_badness(state: &str) -> u8 {
    match state.split(' ').next().unwrap_or_default() {
        "running" | "healthy" => 0,
        "starting" | "created" | "restarting" | "paused" => 1,
        _ => 2,
    }
}

/// Why a lane has no index or no port block, for [`unassigned`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Unassigned {
    /// The project has `lanes.max` lanes.
    LanesMax(u32),
    /// No free port block is left.
    PoolExhausted,
    /// The lane has an index but no port block.
    NoPorts,
    /// The lane has no index yet, or no longer has one.
    NoRecord,
}

/// `unassigned` for the lane `lane` (its reference), which every client names
/// in the message.
pub(crate) fn unassigned(lane: &str, reason: Unassigned) -> RpcError {
    let data = match reason {
        Unassigned::LanesMax(max) => json!({"lane": lane, "reason": "lanes_max", "max": max}),
        Unassigned::PoolExhausted => json!({"lane": lane, "reason": "pool_exhausted"}),
        Unassigned::NoPorts => json!({"lane": lane, "reason": "no_ports"}),
        Unassigned::NoRecord => json!({"lane": lane}),
    };
    RpcError::new("unassigned", data, 1)
}

/// `config_names_other_project`: the lane's worktree now holds another project's file.
pub(crate) fn elsewhere_error(other: &str, lane: Option<&str>) -> RpcError {
    let mut error = RpcError::new(
        "config_names_other_project",
        json!({
            "project": other,
            "message": format!(
                "the project file now names project `{other}`; the lane keeps its ports \
                 and data until `devlanes lane gc --yes` collects it"
            ),
        }),
        1,
    );
    if let Some(lane) = lane {
        error.data["lane"] = json!(lane);
    }
    error
}

/// Shapes a config failure for clients, keeping its source location.
pub(crate) fn config_error(error: &devlanes_config::ConfigError) -> RpcError {
    RpcError::new(
        "config_error",
        json!({
            "message": error.message,
            "file": error.location.file,
            "line": error.location.line,
            "column": error.location.column,
        }),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each reading counts for the time since the previous one, so a fast
    /// stretch of sampling does not outweigh a slow one; long gaps are capped.
    #[test]
    fn usage_means_weigh_readings_by_time() {
        let mut usage = UnitUsage::default();
        assert_eq!(usage.average(), (None, None));
        usage.add(50.0, 300, 0);
        assert_eq!(usage.average(), (Some(300), Some(50.0)));
        // Five seconds at 10%, then five one-second readings at 30%.
        usage.add(10.0, 200, 5_000);
        for at in [6_000, 7_000, 8_000, 9_000, 10_000] {
            usage.add(30.0, 400, at);
        }
        let (memory, cpu) = usage.average();
        let cpu = cpu.expect("cpu");
        assert!((cpu - 20.0).abs() < 0.01, "{cpu}");
        assert_eq!(memory, Some(300));
        assert_eq!(usage.peak(), (Some(400), Some(50.0)));
        // A minute without readings counts as USAGE_GAP_MS.
        let mut gap = UnitUsage::default();
        gap.add(0.0, 0, 0);
        gap.add(0.0, 0, 1_000);
        gap.add(100.0, 0, 61_000);
        let cpu = gap.average().1.expect("cpu");
        let expected = 100.0 * USAGE_GAP_MS as f64 / (1 + 1_000 + USAGE_GAP_MS) as f64;
        assert!((cpu - expected).abs() < 0.01, "{cpu}");
    }
}
