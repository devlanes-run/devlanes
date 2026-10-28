//! Projects and worktree lanes (plan 5.1-5.3, decisions 2, 3 and 9): discovery from
//! a cwd, registration of every worktree, lane index assignment with primary-branch
//! moves, port blocks, per-lane config and layout, and live watching.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use devlanes_config::{
    ExpressionContext, NewWorktrees, OpaqueExtensions, ProjectConfig, RemovedWorktrees, Templated,
};
use devlanes_core::{
    LaneId, WorktreeId,
    paths::{name_on_disk, path_starts_with, same_path, strip_path_prefix},
    ports::{PortName, PortPool},
};
use devlanes_proto::{RpcError, events};
use devlanes_resolve::{LaneFacts, ResolveInput, lane_name};
use devlanes_state::{
    AllocationRequest, BlockOwner, DesiredState, LaneInput, ProjectInput, WorktreeInput,
};
use serde_json::{Value, json};

use crate::{
    engine::Inner,
    integrations::{InfraPortNeed, PortScope},
    model::{
        ConfigState, EnvMap, LaneRt, LaneSt, LaneView, ProjectRt, ProjectSt, ServiceInfo,
        config_error, lane_summary, lock,
    },
};

/// How long a lane's worktree folder must stay gone before an idle lane is cleaned.
pub(crate) const MISSING_GRACE: Duration = Duration::from_secs(5);

/// How often a deferred refresh tries for a lane's `op` again.
const DEFERRED_REFRESH_POLL: Duration = Duration::from_millis(50);

/// A lane with a record whose folder is gone and that runs nothing.
fn sweepable(st: &LaneSt) -> bool {
    !st.present && st.record.is_some() && !st.any_running() && st.busy.is_none()
}

/// Worktree as reported by git (or the single non-git root).
#[derive(Debug, Clone)]
pub(crate) struct Tree {
    pub identity: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub present: bool,
}

/// What a caller may do to a lane's port block and layout ([`Inner::lane_access`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneAccess {
    /// Nothing runs: the block may move and the layout follows the config.
    Free,
    /// Units, orphans or spawns are live: the block stays (plan 5.8), and the
    /// layout follows the config only while that needs no other block.
    Running,
    /// Another operation holds the lane's `op`: block and layout stay until it
    /// ends, then the pending refresh runs.
    Held,
}

/// Lanes that appeared or disappeared during one re-list.
#[derive(Default)]
pub(crate) struct SyncReport {
    pub added: Vec<Arc<LaneRt>>,
    pub removed: Vec<Arc<LaneRt>>,
}

/// Internal failures carry their message; the CLI words them.
pub(crate) fn internal(code: &str, error: impl std::fmt::Display) -> RpcError {
    RpcError::new(code, json!({"message": error.to_string()}), 1)
}

/// Maps store failures; pool exhaustion keeps its own code, and the failures a
/// user can act on get a code and a message saying what to do.
pub(crate) fn store_error(error: devlanes_state::Error) -> RpcError {
    use devlanes_state::Error as E;
    match error {
        E::Allocation(error) => allocation_error(&error),
        E::ActiveRuns { units, tasks } => RpcError::new(
            "active_runs",
            json!({
                "units": units,
                "tasks": tasks,
                "message": format!(
                    "{units} unit run(s) and {tasks} task run(s) are still recorded as running; \
                     stop them with `devlanes stop` (orphaned units included) and wait for \
                     running tasks to finish, then retry"
                ),
            }),
            1,
        ),
        E::Conflict { constraint } => RpcError::new(
            "state_conflict",
            json!({
                "constraint": constraint,
                "message": format!(
                    "state.db already has a conflicting record ({constraint}); \
                     `devlanes status` shows what is still running"
                ),
            }),
            1,
        ),
        E::Corrupt(detail) => RpcError::new(
            "state_corrupt",
            json!({
                "detail": detail,
                "message": format!(
                    "state.db holds an unreadable value ({detail}); \
                     `devlanes daemon reset-state --yes` starts over"
                ),
            }),
            1,
        ),
        other => internal("state_error", other),
    }
}

/// Port allocation failures as machine codes.
pub(crate) fn allocation_error(error: &devlanes_core::ports::AllocationError) -> RpcError {
    use devlanes_core::ports::AllocationError as E;
    let (code, data) = match error {
        E::PoolExhausted { size } => ("pool_exhausted", json!({"block_size": size})),
        E::BlockConflict => ("port_block_conflict", json!({})),
        E::OffsetsExhausted { size } => ("block_too_small", json!({"block_size": size})),
        other => (
            "port_allocation_failed",
            json!({"message": other.to_string()}),
        ),
    };
    RpcError::new(code, data, 1)
}

/// Parses a `.env` file: `KEY=VALUE`, optional `export`, quotes and comments.
pub(crate) fn parse_dotenv(text: &str) -> EnvMap {
    let mut map = EnvMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        let value = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            inner.replace("\\n", "\n").replace("\\\"", "\"")
        } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            inner.to_owned()
        } else {
            // Unquoted values end at an inline comment.
            value
                .split_once(" #")
                .map_or(value, |(v, _)| v)
                .trim()
                .to_owned()
        };
        map.insert(key.to_owned(), value);
    }
    map
}

/// Reads the lane's `.env`, falling back to the primary worktree's (plan 5.1).
pub(crate) async fn read_dotenv(lane_dir: PathBuf, primary_dir: PathBuf) -> EnvMap {
    tokio::task::spawn_blocking(move || {
        for dir in [lane_dir, primary_dir] {
            if let Ok(text) = std::fs::read_to_string(dir.join(".env")) {
                return parse_dotenv(&text);
            }
        }
        EnvMap::new()
    })
    .await
    .unwrap_or_default()
}

/// Loads `devlanes.yaml` from a lane's config directory. The local override and
/// `.env` fall back to the primary worktree's when the lane has none.
/// What the composition root registered that config validation needs: preset
/// option parsers and the contributors that name each preset's ports.
#[derive(Clone)]
pub(crate) struct ConfigChecks {
    pub presets: Vec<crate::integrations::PresetEntry>,
    pub contributors: Vec<Arc<dyn devlanes_core::contrib::Contributor>>,
}

impl Inner {
    /// The registered presets and contributors, for [`check_config`].
    pub(crate) fn config_checks(&self) -> ConfigChecks {
        ConfigChecks {
            presets: self.integ.presets.clone(),
            contributors: self.integ.contributors.clone(),
        }
    }
}

/// Validates preset options against the registered presets and counts each
/// preset's required ports against a fixed `lanes.block_size`.
pub(crate) fn check_config(
    config: &ProjectConfig,
    checks: &ConfigChecks,
) -> Result<devlanes_config::CheckReport, devlanes_config::ConfigError> {
    let validator = |name: &str, options: Option<&Value>| -> Result<(), String> {
        match checks.presets.iter().find(|preset| preset.name == name) {
            Some(preset) => (preset.parse)(options),
            None if checks.presets.is_empty() => Ok(()),
            None => Err(format!("unknown preset `{name}`")),
        }
    };
    let ports = |preset: &str| -> Vec<String> {
        let view = devlanes_core::contrib::ServiceView {
            name: "",
            platform: None,
            preset: Some(preset),
            tags: &[],
        };
        let mut names: Vec<String> = checks
            .contributors
            .iter()
            .flat_map(|contributor| contributor.required_ports(&view))
            .collect();
        names.sort();
        names.dedup();
        names
    };
    // With the names, an unknown preset gets the closest registered one suggested.
    let names: Vec<&str> = checks.presets.iter().map(|preset| preset.name).collect();
    devlanes_config::validate_with_registry(config, &names, &validator, &ports)
}

/// Loads the project file `file` from a lane's config directory, with the files it
/// extends and their local overlays; an overlay the lane lacks is read from the primary
/// worktree's config directory. Returns the state and every file read or looked for.
pub(crate) async fn load_config(
    lane_dir: PathBuf,
    primary_dir: PathBuf,
    file: String,
    checks: ConfigChecks,
) -> (ConfigState, Vec<PathBuf>) {
    tokio::task::spawn_blocking(move || {
        let entry = lane_dir.join(&file);
        match entry.try_exists() {
            Ok(true) => {}
            Ok(false) => return (ConfigState::Missing, vec![entry]),
            Err(error) => {
                let error = RpcError::new(
                    "config_error",
                    json!({"message": error.to_string(), "file": entry, "line": 0, "column": 0}),
                    1,
                );
                return (ConfigState::Invalid(error), vec![entry]);
            }
        }
        let fallback = devlanes_config::LocalFallback {
            lane_dir: &lane_dir,
            primary_dir: &primary_dir,
        };
        let load = devlanes_config::load_project_files::<OpaqueExtensions>(&entry, Some(fallback));
        let checked = load
            .result
            .and_then(|(config, _)| check_config(&config, &checks).map(|_| config));
        let state = match checked {
            Ok(config) => ConfigState::Loaded(Arc::new(config)),
            Err(error) => ConfigState::Invalid(config_error(&error)),
        };
        (state, load.files)
    })
    .await
    .unwrap_or_else(|error| {
        (
            ConfigState::Invalid(internal("config_error", error)),
            Vec::new(),
        )
    })
}

/// Modification time and size of each file a lane's config came from (the project
/// file, what it extends, the local overlays devlanes looked for) and of the `.env`
/// files its expressions read; a missing file is `None`.
pub(crate) type ConfigStamp = Vec<(PathBuf, Option<(std::time::SystemTime, u64)>)>;

/// Reads the [`ConfigStamp`] of `files` plus the lane's and the primary worktree's
/// `.env`, off the executor.
pub(crate) async fn config_stamp(
    mut files: Vec<PathBuf>,
    lane_dir: PathBuf,
    primary_dir: PathBuf,
) -> ConfigStamp {
    tokio::task::spawn_blocking(move || {
        files.push(lane_dir.join(".env"));
        files.push(primary_dir.join(".env"));
        files
            .into_iter()
            .map(|file| {
                let stamp = std::fs::metadata(&file)
                    .ok()
                    .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
                (file, stamp)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Settings a lane's file sets differently from the primary worktree's file: the
/// project-wide `lanes.primary` and `lanes.max`, where the primary's applies, and the
/// shared infra (Compose files, the shared project name, shared containers), which
/// every lane starts and the last one to start wins.
fn divergence(
    primary: &ProjectConfig,
    own: &ProjectConfig,
    lane: &str,
) -> Vec<devlanes_proto::LaneWarning> {
    let mut ignored = Vec::new();
    if own.lanes.primary != primary.lanes.primary {
        ignored.push("lanes.primary");
    }
    if as_json(&own.lanes.max) != as_json(&primary.lanes.max) {
        ignored.push("lanes.max");
    }
    let mut warnings = Vec::new();
    if !ignored.is_empty() {
        warnings.push(devlanes_proto::LaneWarning {
            code: "project_setting_ignored".into(),
            data: json!({
                "lane": lane,
                "settings": ignored,
                "message": format!(
                    "{} differs from the primary worktree's project file, whose value applies",
                    ignored.join(" and ")
                ),
            }),
        });
    }
    let shared = |config: &ProjectConfig| -> BTreeMap<String, Value> {
        let mut shared = BTreeMap::new();
        shared.insert("infra.compose".to_owned(), as_json(&config.infra.compose));
        shared.insert(
            "infra.project_name".to_owned(),
            as_json(&config.infra.project_name),
        );
        for (name, container) in &config.infra.containers {
            if !matches!(container.per_lane, Some(Templated::Literal(true))) {
                shared.insert(format!("infra.containers.{name}"), as_json(container));
            }
        }
        shared
    };
    let (theirs, ours) = (shared(primary), shared(own));
    let conflicts: Vec<&String> = ours
        .iter()
        .filter(|(key, value)| theirs.get(*key).is_some_and(|theirs| theirs != *value))
        .map(|(key, _)| key)
        .collect();
    if !conflicts.is_empty() {
        let names: Vec<&str> = conflicts.iter().map(|key| key.as_str()).collect();
        warnings.push(devlanes_proto::LaneWarning {
            code: "infra_spec_conflict".into(),
            data: json!({
                "lane": lane,
                "settings": names,
                "message": format!(
                    "{} differs from the primary worktree's project file; every lane shares it, so each start of a lane with another version changes it for all",
                    names.join(", ")
                ),
            }),
        });
    }
    warnings
}

/// A config value as JSON for comparison, `null` when it cannot be serialized.
fn as_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// `project_name_taken`: another repository, still on disk, uses the name.
fn name_taken(name: &str, other: &Path) -> RpcError {
    RpcError::new(
        "project_name_taken",
        json!({
            "project": name,
            "other": other,
            "message": format!(
                "project `{name}` is already used by {}; give this one another `project:`. \
                 If you need one name in several repositories, ask for it at \
                 https://github.com/devlanes-run/devlanes/issues",
                other.display()
            ),
        }),
        1,
    )
}

/// Evaluates a literal-or-expression without lane context (block size, max).
fn literal<T: Clone + serde::de::DeserializeOwned>(value: &Option<Templated<T>>) -> Option<T> {
    value
        .as_ref()
        .and_then(|value| value.evaluate(&ExpressionContext::default()).ok())
}

impl Inner {
    /// The request's project: the one `--file` names (registering it), else the one
    /// `--project` names, else the cwd's ([`Self::project_for_cwd`]).
    pub(crate) async fn project_for(
        self: &Arc<Self>,
        context: &devlanes_proto::Context,
    ) -> Result<Option<Arc<ProjectRt>>, RpcError> {
        if let Some(file) = &context.file {
            return self.load_project(file).await.map(Some);
        }
        if let Some(name) = &context.project {
            return self.project_by_name(name).map(Some);
        }
        self.project_for_cwd(&context.cwd).await
    }

    /// The registered project named `name`; `not_found` says how a project file other
    /// than `devlanes.yaml` gets registered.
    pub(crate) fn project_by_name(&self, name: &str) -> Result<Arc<ProjectRt>, RpcError> {
        self.all_projects()
            .into_iter()
            .find(|project| project.name() == name)
            .ok_or_else(|| {
                let mut names: Vec<String> = self.all_projects().iter().map(|p| p.name()).collect();
                names.sort();
                RpcError::new(
                    "not_found",
                    json!({
                        "reference": name,
                        "candidates": names,
                        "hint": "a project in a file other than devlanes.yaml is registered \
                                 the first time a command names its file with --file",
                    }),
                    3,
                )
            })
    }

    /// The project of the nearest `devlanes.yaml` above `cwd` (registered on first
    /// use). Without one, the project of the deepest lane holding `cwd`: a project file
    /// below the worktree root seen from above it, or a worktree whose file is gone.
    pub(crate) async fn project_for_cwd(
        self: &Arc<Self>,
        cwd: &Path,
    ) -> Result<Option<Arc<ProjectRt>>, RpcError> {
        if cwd.as_os_str().is_empty() {
            return Ok(None);
        }
        let search = {
            let cwd = cwd.to_path_buf();
            tokio::task::spawn_blocking(move || devlanes_config::search_project_file(&cwd))
                .await
                .map_err(|error| internal("io", error))?
                .map_err(|error| internal("io", error))?
        };
        let file = match search {
            devlanes_config::ConfigSearch::Found(file) => file,
            devlanes_config::ConfigSearch::Missing { .. } => {
                return Ok(self.project_holding(cwd));
            }
        };
        // A lane read from this file answers without parsing it again, once its stamp
        // shows no edit; an invalid file still finds its lane, so `status`, `stop` and
        // `logs` keep working. One that now names another project does not.
        let dir = file.parent().unwrap_or(Path::new(""));
        for (project, lane) in self.all_lanes() {
            let (config_dir, config_file) = project.config_path();
            let matches = {
                let st = lane.st();
                st.present
                    && config_file == devlanes_config::DEFAULT_PROJECT_FILE
                    && !matches!(st.config, ConfigState::Elsewhere(_))
                    && same_path(&st.path.join(&config_dir), dir)
            };
            if !matches {
                continue;
            }
            self.fresh_config(&project, &lane).await;
            if !matches!(lane.st().config, ConfigState::Elsewhere(_)) {
                return Ok(Some(project));
            }
        }
        self.load_project(&file).await.map(Some)
    }

    /// The project of the deepest present lane whose worktree holds `cwd`, preferring a
    /// project loaded from `devlanes.yaml`.
    fn project_holding(&self, cwd: &Path) -> Option<Arc<ProjectRt>> {
        let mut best: Option<(usize, bool, Arc<ProjectRt>)> = None;
        for (project, lane) in self.all_lanes() {
            let default = project.config_path().1 == devlanes_config::DEFAULT_PROJECT_FILE;
            let st = lane.st();
            // Any config state: a worktree whose file was deleted still has its lane.
            if !st.present || !path_starts_with(cwd, &st.path) {
                continue;
            }
            let depth = st.path.components().count();
            if best
                .as_ref()
                .is_none_or(|(d, def, _)| (depth, default) > (*d, *def))
            {
                best = Some((depth, default, project.clone()));
            }
        }
        best.map(|(_, _, project)| project)
    }

    /// Registers the project a project file names, or returns it when known. Projects
    /// are identified by name: another repository that uses the name is refused, and so
    /// is a second file in this repository naming the same project while the first
    /// still does.
    pub(crate) async fn load_project(
        self: &Arc<Self>,
        config_file: &Path,
    ) -> Result<Arc<ProjectRt>, RpcError> {
        if devlanes_config::is_local_overlay(config_file) {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": format!(
                    "{} is a local overlay; name the project file it belongs to",
                    config_file.display()
                )}),
                2,
            ));
        }
        let config_dir = config_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let file_name = config_file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| devlanes_config::DEFAULT_PROJECT_FILE.to_owned());
        let (state, _) = load_config(
            config_dir.clone(),
            config_dir.clone(),
            file_name.clone(),
            self.config_checks(),
        )
        .await;
        let config = match state {
            ConfigState::Loaded(config) => config,
            ConfigState::Invalid(error) => return Err(error),
            ConfigState::Missing | ConfigState::Elsewhere(_) => {
                return Err(RpcError::new(
                    "not_found",
                    json!({"reference": config_file}),
                    3,
                ));
            }
        };
        let repository = devlanes_git::discover(&config_dir, config.lanes.primary.as_deref())
            .await
            .map_err(|error| internal("git_failed", error))?;
        let (root, common_dir, primary_branch, relative, trees) = match &repository {
            Some(repo) => {
                let relative = strip_path_prefix(&config_dir, &repo.root)
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let trees = repo.worktrees.iter().map(tree).collect();
                (
                    repo.root.clone(),
                    Some(repo.common_dir.clone()),
                    repo.primary_branch.clone(),
                    relative,
                    trees,
                )
            }
            None => (
                config_dir.clone(),
                None,
                config
                    .lanes
                    .primary
                    .clone()
                    .unwrap_or_else(|| "main".into()),
                PathBuf::new(),
                vec![Tree {
                    identity: "main".into(),
                    path: config_dir.clone(),
                    branch: None,
                    present: true,
                }],
            ),
        };
        let identity = common_dir.clone().unwrap_or_else(|| root.clone());
        let name = config.project.clone();
        // Serializes registration so two first requests cannot both create the project.
        let _registering = self.registering.lock().await;
        if let Some(project) = self.all_projects().into_iter().find(|p| p.name() == name) {
            let (known_root, known_identity) = {
                let st = project.st();
                (
                    st.root.clone(),
                    st.common_dir.clone().unwrap_or_else(|| st.root.clone()),
                )
            };
            if !same_path(&known_identity, &identity) {
                let gone = !tokio::fs::try_exists(&known_root).await.unwrap_or(true);
                if !gone {
                    return Err(name_taken(&name, &known_root));
                }
                // The repository moved: the name follows it.
                self.adopt_repository(&project, &root, common_dir.clone(), &primary_branch)
                    .await?;
            }
            let (known_dir, known_file) = project.config_path();
            if known_dir != relative || known_file != file_name {
                self.move_config(&project, config_file, &root, &relative, &file_name)
                    .await?;
            }
            drop(_registering);
            self.sync_worktrees(&project, trees).await?;
            return Ok(project);
        }
        let record = self
            .store
            .upsert_project(ProjectInput {
                name: name.clone(),
                common_dir: common_dir.clone(),
                root_path: root.clone(),
                primary_branch: primary_branch.clone(),
            })
            .await
            .map_err(store_error)?;
        self.store
            .set_project_config(record.id, relative.clone(), file_name.clone())
            .await
            .map_err(store_error)?;
        let project = self
            .register_project(
                record.id,
                name,
                root,
                common_dir,
                primary_branch,
                (relative, file_name),
            )
            .await?;
        drop(_registering);
        self.sync_worktrees(&project, trees).await?;
        self.watch_project(&project, repository).await;
        Ok(project)
    }

    /// Points a project at the repository it now lives in, after the one it was
    /// registered from is gone.
    async fn adopt_repository(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        root: &Path,
        common_dir: Option<PathBuf>,
        primary_branch: &str,
    ) -> Result<(), RpcError> {
        self.store
            .upsert_project(ProjectInput {
                name: project.name(),
                common_dir: common_dir.clone(),
                root_path: root.to_owned(),
                primary_branch: primary_branch.to_owned(),
            })
            .await
            .map_err(store_error)?;
        let mut st = project.st();
        st.root = root.to_owned();
        st.common_dir = common_dir;
        st.primary_branch = primary_branch.to_owned();
        Ok(())
    }

    /// Records that a project's file is now `file_name` in `relative`, unless the file
    /// it was registered from still names the project, which makes two files claim one
    /// name.
    async fn move_config(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        config_file: &Path,
        worktree: &Path,
        relative: &Path,
        file_name: &str,
    ) -> Result<(), RpcError> {
        let (known_dir, known_file) = project.config_path();
        {
            // The old file would sit in the same worktree as the new one.
            let old_dir = worktree.join(&known_dir);
            let (state, _) = load_config(
                old_dir.clone(),
                old_dir.clone(),
                known_file.clone(),
                self.config_checks(),
            )
            .await;
            if state
                .loaded()
                .is_some_and(|config| config.project == project.name())
            {
                return Err(RpcError::new(
                    "duplicate_project_name",
                    json!({
                        "project": project.name(),
                        "file": config_file,
                        "other": old_dir.join(&known_file),
                        "message": format!(
                            "{} and {} both name project `{}`; give one of them another `project:`",
                            config_file.display(),
                            old_dir.join(&known_file).display(),
                            project.name()
                        ),
                    }),
                    1,
                ));
            }
        }
        self.store
            .set_project_config(project.id, relative.to_owned(), file_name.to_owned())
            .await
            .map_err(store_error)?;
        project.set_config_path(relative.to_owned(), file_name.to_owned());
        for lane in project.lanes() {
            if lane.st().present {
                self.reload_config(project, &lane).await;
            }
        }
        Ok(())
    }

    /// Creates the in-memory project and loads its persisted worktrees and lanes.
    pub(crate) async fn register_project(
        self: &Arc<Self>,
        id: devlanes_core::ProjectId,
        name: String,
        root: PathBuf,
        common_dir: Option<PathBuf>,
        primary_branch: String,
        (config_dir, config_file): (PathBuf, String),
    ) -> Result<Arc<ProjectRt>, RpcError> {
        if let Some(project) = self.project(id) {
            return Ok(project);
        }
        let worktrees = self.store.worktrees(id).await.map_err(store_error)?;
        let lanes = self.store.lanes(Some(id)).await.map_err(store_error)?;
        let mut map = BTreeMap::new();
        for worktree in worktrees {
            let mut st = LaneSt::new(
                worktree.identity.clone(),
                worktree.path.clone(),
                worktree.branch.clone(),
                worktree.present,
            );
            st.record = lanes
                .iter()
                .find(|lane| lane.worktree_id == worktree.id && lane.cleaned_at.is_none())
                .cloned();
            if let Some(record) = &st.record {
                st.allocation = self
                    .store
                    .allocation(BlockOwner::Lane(record.id))
                    .await
                    .map_err(store_error)?;
                st.profiles = record.profiles.clone();
            }
            map.insert(worktree.id, new_lane(worktree.id, st));
        }
        let project = Arc::new(ProjectRt {
            id,
            sync: tokio::sync::Mutex::new(()),
            st: std::sync::Mutex::new(ProjectSt {
                name,
                root,
                common_dir,
                primary_branch,
                last_env: None,
                lanes: map,
                config_dir,
                config_file,
            }),
            watching: std::sync::atomic::AtomicBool::new(false),
            watch: self.shutdown.child_token(),
        });
        lock(&self.projects).insert(id, project.clone());
        Ok(project)
    }

    /// The primary worktree: lane 0, else the worktree on the primary branch, else
    /// the main checkout, else the project root.
    pub(crate) fn primary_path(&self, project: &ProjectRt) -> PathBuf {
        let st = project.st();
        let lanes: Vec<_> = st.lanes.values().cloned().collect();
        let primary = st.primary_branch.clone();
        let root = st.root.clone();
        drop(st);
        let mut fallback = None;
        for lane in &lanes {
            let lane = lane.st();
            if lane.index() == Some(0) {
                return lane.path.clone();
            }
            if lane.present
                && lane.alias().is_none()
                && lane.branch.as_deref() == Some(primary.as_str())
            {
                fallback = Some(lane.path.clone());
            }
        }
        if let Some(path) = fallback {
            return path;
        }
        lanes
            .iter()
            .find(|lane| lane.st().identity == "main")
            .map_or(root, |lane| lane.st().path.clone())
    }

    /// The loaded config of the primary worktree's lane, whose project-wide settings
    /// (`lanes.primary`, `lanes.max`) apply to every lane.
    pub(crate) fn primary_config(&self, project: &ProjectRt) -> Option<Arc<ProjectConfig>> {
        let primary = self.primary_path(project);
        project.lanes().into_iter().find_map(|lane| {
            let st = lane.st();
            (st.alias().is_none() && same_path(&st.path, &primary))
                .then(|| st.config.loaded().cloned())
                .flatten()
        })
    }

    /// Re-lists worktrees from git and applies the diff, then cleans idle lanes
    /// whose folder is gone, in the background.
    pub(crate) async fn relist(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
    ) -> Result<SyncReport, RpcError> {
        let report = self.relist_only(project).await?;
        self.sweep_missing(project);
        Ok(report)
    }

    /// [`Self::relist`] without the clean, for `lane gc`, which collects those
    /// lanes itself and asks first.
    pub(crate) async fn relist_only(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
    ) -> Result<SyncReport, RpcError> {
        let (root, common_dir) = {
            let st = project.st();
            (st.root.clone(), st.common_dir.clone())
        };
        let Some(common_dir) = common_dir else {
            return Ok(SyncReport::default());
        };
        let trees = match devlanes_git::list(&root, &common_dir).await {
            Ok(list) => list.iter().map(tree).collect(),
            // The registered root may itself be a removed worktree; ask another one.
            Err(_) => {
                let alternative = project
                    .lanes()
                    .into_iter()
                    .map(|lane| lane.st().path.clone())
                    .find(|path| path.exists() && !same_path(path, &root));
                match alternative {
                    Some(path) => devlanes_git::list(&path, &common_dir)
                        .await
                        .map_err(|error| internal("git_failed", error))?
                        .iter()
                        .map(tree)
                        .collect(),
                    None => return Ok(SyncReport::default()),
                }
            }
        };
        self.sync_worktrees(project, trees).await
    }

    /// Applies a worktree list: registers new worktrees, marks missing ones, assigns
    /// lanes and reloads configs. Never stops anything by itself.
    pub(crate) async fn sync_worktrees(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        trees: Vec<Tree>,
    ) -> Result<SyncReport, RpcError> {
        let _sync = project.sync.lock().await;
        let mut report = SyncReport::default();
        let mut seen = BTreeSet::new();
        let mut changed: Vec<Arc<LaneRt>> = Vec::new();
        for tree in trees.into_iter().filter(|t| !t.identity.is_empty()) {
            let record = self
                .store
                .upsert_worktree(WorktreeInput {
                    project_id: project.id,
                    identity: tree.identity.clone(),
                    path: tree.path.clone(),
                    branch: tree.branch.clone(),
                    present: tree.present,
                })
                .await
                .map_err(store_error)?;
            seen.insert(record.id);
            let existing = project.st().lanes.get(&record.id).cloned();
            let lane = match existing {
                Some(lane) => lane,
                None => {
                    let lane = new_lane(
                        record.id,
                        LaneSt::new(
                            tree.identity.clone(),
                            tree.path.clone(),
                            tree.branch.clone(),
                            false,
                        ),
                    );
                    project.st().lanes.insert(record.id, lane.clone());
                    lane
                }
            };
            let (was_present, moved, rebranched) = {
                let mut st = lane.st();
                let was_present = st.present;
                let moved = !same_path(&st.path, &tree.path);
                let rebranched = st.branch != tree.branch;
                st.path = tree.path.clone();
                st.branch = tree.branch.clone();
                st.present = tree.present;
                if tree.present {
                    st.missing_since = None;
                }
                (was_present, moved, rebranched)
            };
            if tree.present
                && (!was_present
                    || moved
                    || matches!(
                        lane.st().config,
                        ConfigState::Missing | ConfigState::Elsewhere(_)
                    ))
            {
                self.reload_config(project, &lane).await;
            }
            if tree.present && !was_present {
                report.added.push(lane.clone());
            }
            if !tree.present && was_present {
                report.removed.push(lane.clone());
            }
            if moved || rebranched || tree.present != was_present {
                changed.push(lane.clone());
            }
        }
        // Extra lanes of a folder are not git worktrees: they follow their base.
        let lanes = project.lanes();
        for alias in &lanes {
            let Some(base_identity) = alias.st().alias().map(|(base, _)| base.to_owned()) else {
                continue;
            };
            let base = lanes
                .iter()
                .find(|lane| lane.st().identity == base_identity)
                .map(|base| {
                    let st = base.st();
                    (st.path.clone(), st.branch.clone(), st.present)
                });
            let (path, branch, present) = {
                let st = alias.st();
                base.unwrap_or_else(|| (st.path.clone(), st.branch.clone(), false))
            };
            let identity = alias.st().identity.clone();
            let (was_present, moved) = {
                let mut st = alias.st();
                let was_present = st.present;
                let moved = !same_path(&st.path, &path) || st.branch != branch;
                st.path = path.clone();
                st.branch = branch.clone();
                st.present = present;
                if present {
                    st.missing_since = None;
                }
                (was_present, moved)
            };
            seen.insert(alias.worktree);
            if !moved && present == was_present {
                continue;
            }
            self.store
                .upsert_worktree(WorktreeInput {
                    project_id: project.id,
                    identity,
                    path,
                    branch,
                    present,
                })
                .await
                .map_err(store_error)?;
            if present {
                self.reload_config(project, alias).await;
            } else if was_present {
                report.removed.push(alias.clone());
            }
            changed.push(alias.clone());
        }
        for lane in project.lanes() {
            if seen.contains(&lane.worktree) {
                continue;
            }
            let was_present = {
                let mut st = lane.st();
                std::mem::replace(&mut st.present, false)
            };
            if was_present {
                let _ = self.store.mark_worktree_missing(lane.worktree).await;
                report.removed.push(lane.clone());
                changed.push(lane.clone());
            }
        }
        let assigned = self.assign_lanes(project).await?;
        // Only newly assigned lanes count as "added" for new_worktrees policies.
        report.added.retain(|lane| {
            assigned.iter().any(|a| Arc::ptr_eq(a, lane)) || lane.st().record.is_some()
        });
        changed.extend(assigned);
        self.update_moves(project);
        self.refresh_views(project).await;
        for lane in changed {
            self.emit_lane(project, &lane);
        }
        Ok(report)
    }

    /// Assigns lanes to worktrees whose config appeared, without re-listing:
    /// unlike [`Self::sync_worktrees`] it never marks a worktree missing.
    async fn assign_pending(self: &Arc<Self>, project: &Arc<ProjectRt>) -> Result<(), RpcError> {
        let _sync = project.sync.lock().await;
        let assigned = self.assign_lanes(project).await?;
        self.update_moves(project);
        for lane in assigned {
            self.emit_lane(project, &lane);
        }
        Ok(())
    }

    /// Gives present worktrees with a config a lane index and port block
    /// (decision 2: lane 0 for the primary branch, or any branch under
    /// [`crate::EngineOptions::ci`], else lowest free index >= 1).
    async fn assign_lanes(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
    ) -> Result<Vec<Arc<LaneRt>>, RpcError> {
        let primary = project.st().primary_branch.clone();
        // `lanes.max` is project-wide: the primary worktree's file sets it.
        let primary_max = self
            .primary_config(project)
            .and_then(|config| literal(&config.lanes.max));
        let mut lanes = project.lanes();
        lanes.sort_by_key(|lane| lane.st().path.clone());
        let mut assigned = Vec::new();
        for lane in lanes {
            let (config, branch, alias) = {
                let st = lane.st();
                if !st.present || st.record.is_some() {
                    continue;
                }
                // Only the primary worktree gets a lane by existing; any other
                // gets one once devlanes runs in it.
                let primary_tree = st.alias().is_none()
                    && (st.identity == "main" || st.branch.as_deref() == Some(primary.as_str()));
                if !primary_tree && !st.wanted {
                    continue;
                }
                let Some(config) = st.config.loaded().cloned() else {
                    continue;
                };
                (config, st.branch.clone(), st.alias().is_some())
            };
            let used: BTreeSet<u32> = project
                .lanes()
                .iter()
                .filter_map(|l| l.st().index())
                .collect();
            if let Some(max) = primary_max.or_else(|| literal(&config.lanes.max))
                && used.len() >= usize::try_from(max).unwrap_or(usize::MAX)
            {
                lane.st().unassigned = Some(RpcError::new(
                    "unassigned",
                    json!({"reason": "lanes_max", "max": max}),
                    1,
                ));
                continue;
            }
            // An extra lane of a folder never takes lane 0 from its first lane.
            let index = if !alias
                && (self.opts.ci || branch.as_deref() == Some(primary.as_str()))
                && !used.contains(&0)
            {
                0
            } else {
                (1..).find(|i| !used.contains(i)).unwrap_or(1)
            };
            let record = self
                .store
                .upsert_lane(LaneInput {
                    project_id: project.id,
                    worktree_id: lane.worktree,
                    index,
                    name: lane_name(index),
                    desired_state: DesiredState::Stopped,
                    profile: None,
                    profiles: Vec::new(),
                })
                .await
                .map_err(store_error)?;
            lane.st().record = Some(record.clone());
            match self.allocate_lane(project, &lane, &[]).await {
                Ok(_) => {
                    lane.st().unassigned = None;
                    // A failure here comes back as an error at the lane's first start.
                    if let Err(error) = self.ensure_lane_data(project, &lane).await {
                        tracing::warn!(code = %error.code, data = %error.data, "lane data folder");
                    }
                    assigned.push(lane);
                }
                Err(error) if error.code == "pool_exhausted" => {
                    let _ = self.store.delete_lane(record.id).await;
                    let mut st = lane.st();
                    st.record = None;
                    st.allocation = None;
                    st.unassigned = Some(RpcError::new(
                        "unassigned",
                        json!({"reason": "pool_exhausted"}),
                        1,
                    ));
                }
                Err(error) => {
                    lane.st().last_error = Some(error);
                    assigned.push(lane);
                }
            }
        }
        Ok(assigned)
    }

    /// Computes `moves_to` for every assigned lane (applied at its next start).
    /// Under CI a lane that holds lane 0 keeps it on any branch, and no lane on
    /// the primary branch moves onto it.
    pub(crate) fn update_moves(&self, project: &ProjectRt) {
        let primary = project.st().primary_branch.clone();
        let lanes = project.lanes();
        let used: BTreeSet<u32> = lanes.iter().filter_map(|l| l.st().index()).collect();
        let free = (1..).find(|i| !used.contains(i)).unwrap_or(1);
        for lane in lanes {
            let mut st = lane.st();
            let Some(index) = st.index() else {
                st.moves_to = None;
                continue;
            };
            if st.alias().is_some() {
                st.moves_to = None;
                continue;
            }
            let on_primary = st.branch.as_deref() == Some(primary.as_str());
            st.moves_to = match (on_primary, index) {
                (true, 0) | (false, 1..) => None,
                (true, _) if self.opts.ci && used.contains(&0) => None,
                (true, _) => Some(0),
                (false, 0) if self.opts.ci => None,
                (false, 0) => Some(free),
            };
        }
    }

    /// Applies a pending move before a start. Lane 0's current holder is moved out
    /// only while it runs nothing; otherwise the move waits and the start proceeds.
    pub(crate) async fn apply_move(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) -> Result<(), RpcError> {
        let _sync = project.sync.lock().await;
        self.update_moves(project);
        let target = {
            let st = lane.st();
            // A lane that runs something keeps its index, domain and ports.
            match st.moves_to {
                // The calling start counts in `starting`; another one may have
                // planned on the current index.
                Some(target) if st.record.is_some() && !st.any_running() && st.starting <= 1 => {
                    target
                }
                _ => return Ok(()),
            }
        };
        if target == 0
            && let Some(holder) = project.lane_by_index(0)
            && !Arc::ptr_eq(&holder, lane)
        {
            // A start, stop or clean in flight holds the holder's `op`; holding it
            // here keeps one from beginning while the holder is renumbered.
            let Ok(_holder_op) = holder.op.try_write() else {
                return Ok(());
            };
            if holder.st().any_running() || holder.st().busy.is_some() {
                return Ok(());
            }
            let used: BTreeSet<u32> = project
                .lanes()
                .iter()
                .filter_map(|l| l.st().index())
                .collect();
            let free = (1..).find(|i| !used.contains(i)).unwrap_or(1);
            self.set_index(project, &holder, free).await?;
        }
        if target != 0
            && project
                .lane_by_index(target)
                .is_some_and(|other| !Arc::ptr_eq(&other, lane))
        {
            return Ok(());
        }
        self.set_index(project, lane, target).await?;
        self.update_moves(project);
        Ok(())
    }

    /// Persists a new index for a lane, keeping its block, desired state and profile.
    async fn set_index(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        index: u32,
    ) -> Result<(), RpcError> {
        let Some(record) = lane.st().record.clone() else {
            return Ok(());
        };
        let updated = self
            .store
            .upsert_lane(LaneInput {
                project_id: project.id,
                worktree_id: lane.worktree,
                index,
                name: lane_name(index),
                desired_state: record.desired_state,
                profile: record.profile.clone(),
                profiles: record.profiles.clone(),
            })
            .await
            .map_err(store_error)?;
        lane.st().record = Some(updated);
        self.emit_lane(project, lane);
        Ok(())
    }

    /// How far the caller may change `lane`'s block and layout right now; the one
    /// check behind [`Self::allocate_lane`] and [`Self::refresh_views`] and their
    /// `_held` forms. `held` says the caller holds the lane's `op` itself (a start
    /// on its own lane, `infra up`). Otherwise the lane's `op` is tried: while
    /// another operation holds it, normally a start between taking it and its
    /// last spawn, the lane is [`LaneAccess::Held`] and a refresh is scheduled for
    /// when it ends. The returned guard keeps a start from beginning until the
    /// caller is done.
    pub(crate) fn lane_access<'a>(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &'a Arc<LaneRt>,
        held: bool,
    ) -> (LaneAccess, Option<tokio::sync::RwLockWriteGuard<'a, ()>>) {
        let guard = if held {
            None
        } else {
            match lane.op.try_write() {
                Ok(guard) => Some(guard),
                Err(_) => {
                    self.defer_refresh(project, lane);
                    return (LaneAccess::Held, None);
                }
            }
        };
        let st = lane.st();
        // Another start holding the lane planned on the current block, even
        // while its units still wait for a start slot. A `held` caller that is
        // a start counts itself in `starting`.
        let others = st.starting.saturating_sub(usize::from(held));
        let access = if st.any_running() || st.spawning > 0 || others > 0 {
            LaneAccess::Running
        } else {
            LaneAccess::Free
        };
        (access, guard)
    }

    /// Marks a refresh of `lane` pending and, unless one is already queued,
    /// queues a task that runs it once it gets the lane's `op`: right after the
    /// operation holding it ends, before or after the next one in line.
    fn defer_refresh(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        if std::mem::replace(&mut lane.st().refresh_pending, true) {
            return;
        }
        let (engine, project, lane) = (self.clone(), project.clone(), lane.clone());
        tokio::spawn(async move {
            let _op = loop {
                // Tried, never queued for: a writer in `op`'s queue would hold up
                // every start that adds units until the start holding it ends,
                // and that one may wait minutes for a start slot.
                if let Ok(op) = lane.op.try_write() {
                    break op;
                }
                tokio::select! {
                    () = engine.shutdown.cancelled() => return,
                    // The project was dropped (`daemon reset-state`) or the lane removed.
                    () = project.watch.cancelled() => return,
                    () = lane.watch.cancelled() => return,
                    () = tokio::time::sleep(DEFERRED_REFRESH_POLL) => {}
                }
            };
            // A start on this lane may have refreshed it meanwhile.
            if !lane.st().refresh_pending {
                return;
            }
            let refresh: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(engine.refresh_with(&project, Some(&lane)));
            refresh.await;
            engine.emit_lane(&project, &lane);
        });
    }

    /// Allocates (or resizes) the lane's block for the config's ports plus `infra`
    /// needs, and the project's shared block when shared infra needs ports. A lane
    /// that runs something, or that another operation holds, keeps its block
    /// ([`Self::lane_access`]).
    pub(crate) async fn allocate_lane(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        infra: &[InfraPortNeed],
    ) -> Result<devlanes_state::Allocation, RpcError> {
        let (access, _guard) = self.lane_access(project, lane, false);
        self.allocate_with(project, lane, infra, access).await
    }

    /// [`Self::allocate_lane`] for a caller that holds the lane's `op`.
    pub(crate) async fn allocate_lane_held(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        infra: &[InfraPortNeed],
    ) -> Result<devlanes_state::Allocation, RpcError> {
        let (access, _guard) = self.lane_access(project, lane, true);
        self.allocate_with(project, lane, infra, access).await
    }

    /// The allocation itself, under an access the caller already checked.
    async fn allocate_with(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        infra: &[InfraPortNeed],
        access: LaneAccess,
    ) -> Result<devlanes_state::Allocation, RpcError> {
        let (config, record, facts_env) = {
            let st = lane.st();
            let config = st
                .config
                .loaded()
                .cloned()
                .ok_or_else(|| RpcError::new("no_config", json!({"path": st.path}), 1))?;
            let record = st
                .record
                .clone()
                .ok_or_else(|| RpcError::new("unassigned", json!({"reason": "unassigned"}), 1))?;
            (config, record, st.env.clone())
        };
        let needs = devlanes_resolve::port_needs(&config, &self.integ.contributors)
            .map_err(|error| crate::resolve::resolve_error(&error))?;
        let mut names = needs.names.clone();
        for need in infra.iter().filter(|n| n.scope == PortScope::Lane) {
            names.insert(PortName::new(need.unit.clone(), need.name.clone()));
        }
        let pool = self.pool()?;
        let facts = self.facts(project, lane)?;
        let (lane_dir, primary_dir) = self.config_dirs(project, lane);
        let dotenv = read_dotenv(lane_dir, primary_dir).await;
        let client_env = facts_env.map(|env| (*env).clone()).unwrap_or_default();
        let fixed_start =
            devlanes_resolve::fixed_block_start(&config, &facts, &client_env, &dotenv)
                .map_err(|error| crate::resolve::resolve_error(&error))?;
        let fixed_size = literal(&config.lanes.block_size);
        // Running units keep the block they were spawned with (plan 5.8): a
        // config that needs another block waits for the next whole-lane start.
        // A held lane's start has resolved (or is resolving) against the current
        // block; the refresh after it decides. A lane without a block yet has
        // nothing to keep.
        let kept = {
            let mut st = lane.st();
            if access == LaneAccess::Held
                && let Some(current) = st.allocation.clone()
            {
                return Ok(current);
            }
            match st.allocation.clone() {
                Some(current) if access == LaneAccess::Running => {
                    let moves = needs
                        .reserved
                        .iter()
                        .any(|offset| !current.reserved.contains(offset))
                        || fixed_size.is_some_and(|size| size != current.block.size)
                        || fixed_start.is_some_and(|start| start != current.block.start);
                    let missing = names.iter().any(|name| !current.offsets.contains_key(name));
                    if moves || !missing {
                        st.ports_pending = moves;
                        return Ok(current);
                    }
                    Some(current)
                }
                _ => None,
            }
        };
        if let Some(current) = kept {
            // Only new port names: they take free offsets of the kept block, so
            // a service added to a running lane can start without moving it.
            let grown = self
                .store
                .allocate(AllocationRequest {
                    owner: BlockOwner::Lane(record.id),
                    pool,
                    fixed_size: Some(current.block.size),
                    fixed_start: Some(current.block.start),
                    names,
                    reserved: needs.reserved.clone(),
                    avoid: self.remote.advertised_ports(),
                    never: self.external_ports(),
                })
                .await;
            let mut st = lane.st();
            return Ok(match grown {
                Ok(allocation) if allocation.block == current.block => {
                    st.allocation = Some(allocation.clone());
                    st.ports_pending = false;
                    allocation
                }
                // The block has no room for them: they wait for the next
                // whole-lane start, which moves it.
                _ => {
                    st.ports_pending = true;
                    current
                }
            });
        }
        let allocation = self
            .store
            .allocate(AllocationRequest {
                owner: BlockOwner::Lane(record.id),
                pool,
                fixed_size,
                fixed_start,
                names,
                reserved: needs.reserved.clone(),
                avoid: self.avoided_ports(pool).await,
                never: self.external_ports(),
            })
            .await
            .map_err(store_error)?;
        tracing::debug!(
            lane = record.index,
            block_start = allocation.block.start,
            block_size = allocation.block.size,
            ports = allocation.offsets.len(),
            "ports allocated"
        );
        let shared: BTreeSet<PortName> = infra
            .iter()
            .filter(|n| n.scope == PortScope::Shared)
            .map(|n| PortName::new(n.unit.clone(), n.name.clone()))
            .collect();
        if !shared.is_empty() {
            self.store
                .allocate(AllocationRequest {
                    owner: BlockOwner::Project(project.id),
                    pool,
                    fixed_size: None,
                    fixed_start: None,
                    names: shared,
                    reserved: BTreeSet::new(),
                    avoid: self.avoided_ports(pool).await,
                    never: self.external_ports(),
                })
                .await
                .map_err(store_error)?;
        }
        {
            let mut st = lane.st();
            st.allocation = Some(allocation.clone());
            st.ports_pending = false;
        }
        Ok(allocation)
    }

    /// Ports a new pool block keeps clear of: a connected box's mirrored ports.
    /// `infra.external` ports go in the request's `never` instead.
    #[cfg(unix)]
    async fn avoided_ports(&self, _pool: PortPool) -> BTreeSet<u16> {
        self.remote.advertised_ports()
    }

    /// Ports a new pool block keeps clear of: a connected box's mirrored ports
    /// and the Windows reserved port ranges inside `pool`, where no service
    /// could bind. Ranges `netsh` could not list are not avoided; `doctor`
    /// reports why. `infra.external` ports go in the request's `never` instead.
    #[cfg(windows)]
    async fn avoided_ports(&self, pool: PortPool) -> BTreeSet<u16> {
        let mut avoid = self.remote.advertised_ports();
        let reserved = crate::reserved_ports::RESERVED.get().await;
        avoid.extend(reserved.ports_within(pool.start, pool.end));
        avoid
    }

    /// The machine-wide pool from the user config.
    pub(crate) fn pool(&self) -> Result<PortPool, RpcError> {
        let (start, end) = self
            .opts
            .user
            .ports
            .bounds()
            .map_err(|error| config_error(&error))?;
        Ok(PortPool { start, end })
    }

    /// Lane facts for resolution.
    pub(crate) fn facts(&self, project: &ProjectRt, lane: &LaneRt) -> Result<LaneFacts, RpcError> {
        let primary = self.primary_path(project);
        let name = project.name();
        let (config_dir, _) = project.config_path();
        let st = lane.st();
        let record = st
            .record
            .as_ref()
            .ok_or_else(|| RpcError::new("unassigned", json!({"reason": "unassigned"}), 1))?;
        let data_path = self.lane_data_path(&name, record.id);
        Ok(LaneFacts::new(
            name,
            primary,
            record.index,
            st.path.clone(),
            data_path,
            st.branch.clone(),
        )
        .with_project_dir(&config_dir))
    }

    /// `<data root>/<project>`, holding the data folders of the project's lanes.
    /// The folder name is the project name (encoded on Windows, where `con`
    /// or `nul.x` would name a device).
    pub(crate) fn project_data_path(&self, project: &str) -> PathBuf {
        self.opts
            .user
            .lanes
            .data_root(&self.opts.home)
            .join(name_on_disk(project).as_ref())
    }

    /// `<data root>/<project>/<lane id>` (`lane.data_path`). Keyed by the lane's record
    /// id rather than its index: indexes move between lanes (a lane whose branch becomes
    /// the primary one takes lane 0), and the data must stay with its lane. Lanes that
    /// share one worktree have different records, so each gets its own folder.
    pub(crate) fn lane_data_path(&self, project: &str, lane: LaneId) -> PathBuf {
        self.project_data_path(project).join(lane.0.to_string())
    }

    /// Creates the lane's data folder. Called when the lane is assigned and again before
    /// every start, so a folder deleted by hand comes back.
    pub(crate) async fn ensure_lane_data(
        &self,
        project: &ProjectRt,
        lane: &LaneRt,
    ) -> Result<(), RpcError> {
        let Some(id) = lane.st().record.as_ref().map(|record| record.id) else {
            return Ok(());
        };
        let path = self.lane_data_path(&project.name(), id);
        let result = tokio::task::spawn_blocking({
            let path = path.clone();
            move || std::fs::create_dir_all(&path)
        })
        .await
        .map_err(std::io::Error::other)
        .and_then(|created| created);
        result.map_err(|error| {
            RpcError::new(
                "lane_data_unavailable",
                json!({
                    "path": path,
                    "message": format!("cannot create the lane data folder: {error}"),
                }),
                1,
            )
        })
    }

    /// Deletes the lane's data folder (`lane clean`), then the project's folder once no
    /// lane folder is left in it. A missing folder is not an error.
    pub(crate) async fn remove_lane_data(&self, project: &str, lane: LaneId) {
        let path = self.lane_data_path(project, lane);
        let parent = self.project_data_path(project);
        let result = tokio::task::spawn_blocking(move || {
            match crate::files::remove_dir_all(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            // Fails while other lanes' folders remain, which is the point.
            let _ = std::fs::remove_dir(&parent);
            Ok(())
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            #[cfg(windows)]
            Ok(Err(error)) if devlanes_sys_windows::file::is_held(&error) => {
                tracing::warn!(
                    project,
                    lane = lane.0,
                    %error,
                    "cannot delete the lane data folder: a program still has a file in it open; close that program and delete the folder by hand"
                );
            }
            Ok(Err(error)) => {
                tracing::warn!(project, lane = lane.0, %error, "cannot delete the lane data folder");
            }
            Err(error) => tracing::warn!(%error, "lane data folder task failed"),
        }
    }

    /// `(lane config dir, primary config dir)`.
    pub(crate) fn config_dirs(&self, project: &ProjectRt, lane: &LaneRt) -> (PathBuf, PathBuf) {
        let (config_dir, _) = project.config_path();
        let primary = self.primary_path(project).join(&config_dir);
        (lane.st().path.join(&config_dir), primary)
    }

    /// The files a lane's config came from at its last load, or its project file
    /// before the first one.
    fn lane_config_files(&self, project: &ProjectRt, lane: &LaneRt) -> Vec<PathBuf> {
        let files = lane.st().config_files.clone();
        if !files.is_empty() {
            return files;
        }
        let (config_dir, config_file) = project.config_path();
        vec![lane.st().path.join(config_dir).join(config_file)]
    }

    /// Reloads one lane's config. A change while units run, or while a start or
    /// another operation holds the lane, sets `config_changed`; running units are
    /// never touched (plan 5.8). A file that names another project leaves the lane
    /// with [`ConfigState::Elsewhere`].
    pub(crate) async fn reload_config(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        let (lane_dir, primary_dir) = self.config_dirs(project, lane);
        let known = self.lane_config_files(project, lane);
        // Taken before reading, so an edit during the load shows as a new stamp.
        let stamp = config_stamp(known.clone(), lane_dir.clone(), primary_dir.clone()).await;
        let (_, config_file) = project.config_path();
        let (config, files) = load_config(
            lane_dir.clone(),
            primary_dir.clone(),
            config_file,
            self.config_checks(),
        )
        .await;
        let config = match config {
            ConfigState::Loaded(config) if config.project != project.name() => {
                ConfigState::Elsewhere(config.project.clone())
            }
            other => self.with_resolver_domain(other),
        };
        // A load that read other files (an `extends` added or removed) stamps those.
        let stamp = if files == known {
            stamp
        } else {
            config_stamp(files.clone(), lane_dir, primary_dir).await
        };
        tracing::debug!(
            path = %lane.st().path.display(),
            state = match &config {
                ConfigState::Loaded(_) => "loaded",
                ConfigState::Missing => "missing",
                ConfigState::Invalid(_) => "invalid",
                ConfigState::Elsewhere(_) => "another project",
            },
            "config read"
        );
        let in_flight = lane.op.try_write().is_err();
        {
            let mut st = lane.st();
            st.config_stamp = Some(stamp);
            st.config_files = files;
            let before = match &st.config {
                ConfigState::Loaded(config) => serde_json::to_value(config.as_ref()).ok(),
                _ => None,
            };
            let after = match &config {
                ConfigState::Loaded(config) => serde_json::to_value(config.as_ref()).ok(),
                _ => None,
            };
            let differs = before != after || before.is_none();
            // A start in flight spawns with the config it began with; the drift
            // computed once it ends says what this edit changed.
            if differs && (in_flight || !st.starts.is_empty()) {
                st.config_changed = true;
            }
            if let ConfigState::Loaded(loaded) = &config {
                st.last_good = Some(loaded.clone());
            }
            st.config = config;
        }
    }

    /// Reloads the lane's config right away when one of its files changed since the
    /// last load, so `start`, `restart`, `run`, `wait` and `config check` sent right
    /// after an edit see it instead of waiting for the poller, which calls this too.
    pub(crate) async fn fresh_config(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        let present = lane.st().present;
        if !present {
            return;
        }
        let (lane_dir, primary_dir) = self.config_dirs(project, lane);
        let files = self.lane_config_files(project, lane);
        let current = config_stamp(files, lane_dir, primary_dir).await;
        if lane.st().config_stamp.as_ref() == Some(&current) {
            return;
        }
        self.reload_config(project, lane).await;
        if lane.st().record.is_none()
            && let Err(error) = self.assign_pending(project).await
        {
            tracing::warn!(code = %error.code, "lane assignment failed");
        }
        self.refresh_views(project).await;
        // After the refresh, so new ports have offsets and the layout lists new
        // services. A `.env` edit leaves the config as it was, so plans decide.
        let running = lane.st().any_running();
        if running {
            self.update_drift(project, lane).await;
        }
        self.emit_lane(project, lane);
    }

    /// Recomputes layouts (ports, URLs, routes) for every assigned lane, lane 0
    /// first so the others see `lanes.main.services`. A lane another operation holds
    /// keeps its layout until that operation ends ([`Self::lane_access`]).
    pub(crate) async fn refresh_views(self: &Arc<Self>, project: &Arc<ProjectRt>) {
        self.refresh_with(project, None).await;
    }

    /// [`Self::refresh_views`] for a caller that holds `lane`'s `op`.
    pub(crate) async fn refresh_views_held(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        self.refresh_with(project, Some(lane)).await;
    }

    /// The refresh, with the lane whose `op` the caller holds, if any.
    async fn refresh_with(self: &Arc<Self>, project: &Arc<ProjectRt>, held: Option<&Arc<LaneRt>>) {
        use tracing::Instrument as _;
        let name = project.name();
        let primary_config = self.primary_config(project);
        for lane in project.lanes() {
            let mut st = lane.st();
            let divergent = match (&primary_config, st.config.loaded()) {
                (Some(primary), Some(own)) if !Arc::ptr_eq(primary, own) => {
                    divergence(primary, own, &st.reference(&name))
                }
                _ => Vec::new(),
            };
            st.divergent = divergent;
        }
        self.refresh_lanes(project, held)
            .instrument(tracing::debug_span!("refresh views"))
            .await;
    }

    /// Body of [`Self::refresh_with`], in its span.
    async fn refresh_lanes(self: &Arc<Self>, project: &Arc<ProjectRt>, held: Option<&Arc<LaneRt>>) {
        let mut lanes = project.lanes();
        lanes.sort_by_key(|lane| {
            lane.st()
                .index()
                .map_or(u32::MAX, |i| if i == 0 { 0 } else { i })
        });
        let mut main_services: Option<Value> = None;
        for lane in lanes {
            let index = lane.st().index();
            if index.is_none() {
                lane.st().view = None;
                continue;
            }
            let owned = held.is_some_and(|held| Arc::ptr_eq(held, &lane));
            let (access, _guard) = self.lane_access(project, &lane, owned);
            // A held lane's start resolves against the layout it has; the
            // deferred refresh replaces it once the start ended.
            if access == LaneAccess::Held {
                if let Some(view) = lane.st().view.clone().filter(|_| index == Some(0)) {
                    main_services = Some(view.services_namespace.clone());
                }
                continue;
            }
            // This refresh is the one a deferral waited for.
            lane.st().refresh_pending = false;
            // A config that gained ports needs offsets before its layout resolves.
            let keep = if self.needs_allocation(&lane) {
                let _ = self.allocate_with(project, &lane, &[], access).await;
                access == LaneAccess::Running && lane.st().ports_pending
            } else {
                lane.st().ports_pending = false;
                false
            };
            // While the block waits for the next whole-lane start, the running
            // units' layout stays; the new config cannot resolve without its block.
            let kept = lane.st().view.clone().filter(|_| keep);
            if let Some(view) = kept {
                if index == Some(0) {
                    main_services = Some(view.services_namespace.clone());
                }
                continue;
            }
            let view = tracing::Instrument::instrument(
                self.compute_view(project, &lane, main_services.as_ref()),
                tracing::debug_span!("lane view", name = index.unwrap_or_default()),
            )
            .await;
            match view {
                Ok(view) => {
                    if index == Some(0) {
                        main_services = Some(view.services_namespace.clone());
                    }
                    let mut st = lane.st();
                    st.view = Some(Arc::new(view));
                    if st
                        .last_error
                        .as_ref()
                        .is_some_and(|e| e.code == "config_error")
                    {
                        st.last_error = None;
                    }
                }
                Err(error) => {
                    let mut st = lane.st();
                    st.view = None;
                    if !matches!(
                        st.config,
                        ConfigState::Missing | ConfigState::Invalid(_) | ConfigState::Elsewhere(_)
                    ) {
                        st.last_error = Some(error);
                    }
                }
            }
        }
        let _span = tracing::debug_span!("publish routes").entered();
        self.publish_routes();
    }

    /// True when the lane's config names a port without a persisted offset or
    /// fixes a block size other than the lane's.
    fn needs_allocation(&self, lane: &LaneRt) -> bool {
        let _span = tracing::debug_span!("port needs").entered();
        let st = lane.st();
        let (Some(config), Some(allocation)) = (st.config.loaded(), &st.allocation) else {
            return false;
        };
        if literal(&config.lanes.block_size).is_some_and(|size| size != allocation.block.size) {
            return true;
        }
        devlanes_resolve::port_needs(config, &self.integ.contributors).is_ok_and(|needs| {
            needs
                .names
                .iter()
                .any(|name| !allocation.offsets.contains_key(name))
                || needs
                    .reserved
                    .iter()
                    .any(|offset| !allocation.reserved.contains(offset))
        })
    }

    /// Layout of one lane without contributors or secrets.
    async fn compute_view(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        lanes_main: Option<&Value>,
    ) -> Result<LaneView, RpcError> {
        let (config, allocation, env, infra_ns) = {
            let st = lane.st();
            // Running units keep the layout they started with while the file is broken.
            let kept = st.last_good.clone().filter(|_| st.any_running());
            let config = match (&st.config, kept) {
                (ConfigState::Loaded(config), _) => config.clone(),
                (_, Some(config)) => config,
                (ConfigState::Invalid(error), None) => return Err(error.clone()),
                (ConfigState::Missing, None) => {
                    return Err(RpcError::new("no_config", json!({"path": st.path}), 1));
                }
                (ConfigState::Elsewhere(other), None) => {
                    return Err(crate::model::elsewhere_error(other));
                }
            };
            let allocation = st
                .allocation
                .clone()
                .ok_or_else(|| RpcError::new("unassigned", json!({"reason": "no_ports"}), 1))?;
            (config, allocation, st.env.clone(), st.infra_ns.clone())
        };
        let facts = self.facts(project, lane)?;
        let (lane_dir, primary_dir) = self.config_dirs(project, lane);
        let dotenv = read_dotenv(lane_dir, primary_dir).await;
        let client_env = env.map(|env| (*env).clone()).unwrap_or_default();
        let infra = infra_ns.unwrap_or(Value::Null);
        let mut input = ResolveInput::new(&config, &facts, allocation.block, &allocation.offsets);
        input.client_env = &client_env;
        input.dotenv = &dotenv;
        input.infra = &infra;
        input.lanes_main = lanes_main;
        input.contributors = &self.integ.contributors;
        input.ca_env = &self.integ.ca_env;
        input.public_https_port = self.integ.routes.https_port();
        let layout = devlanes_resolve::lane_layout(&input)
            .map_err(|error| crate::resolve::resolve_error(&error))?;
        let services = layout
            .services
            .values()
            .map(|service| ServiceInfo {
                name: service.name.clone(),
                unit: service.unit.clone(),
                port: service.port,
                ports: service.ports.clone(),
                url: service.url.clone(),
                public_url: service.public_url.clone(),
                tags: service.tags.clone(),
                hidden: service.hidden,
                autostart: service.autostart,
                color: service.color.clone(),
                display_name: service.display_name.clone(),
            })
            .collect();
        let infra_routes = infra_routes(
            &config,
            &facts,
            &layout,
            &infra,
            &client_env,
            &dotenv,
            self.integ.routes.https_port(),
        );
        let reference = lane.st().reference(&project.name());
        let own = crate::external::lane_ports(
            &reference,
            layout
                .services
                .values()
                .map(|service| (service.name.as_str(), service.port, &service.ports)),
            &infra,
        );
        if let Some(error) = self.external_conflict(&config, lane, own) {
            return Err(error);
        }
        let externals = crate::external::external_infos(
            &config,
            &lane_context(
                &facts,
                &layout.domain,
                layout.port,
                &layout.vars,
                &infra,
                &client_env,
                &dotenv,
            ),
            self.integ.routes.https_port(),
        );
        Ok(LaneView {
            domain: layout.domain.clone(),
            port: layout.port,
            vars: layout.vars.clone(),
            block: (allocation.block.start, allocation.block.size),
            services,
            routes: layout.routes.clone(),
            infra_routes,
            externals,
            lane_env: layout.lane_env().clone(),
            services_namespace: layout.services_namespace(),
        })
    }

    /// Emits `lane.changed` with a fresh summary and republishes routes.
    pub(crate) fn emit_lane(&self, project: &ProjectRt, lane: &LaneRt) {
        self.emit_lane_event(project, lane);
        self.publish_routes();
    }

    /// Emits `lane.changed` with a summary taken now. The lane's publish lock
    /// spans taking and sending it, so concurrent callers cannot deliver an older
    /// summary after a newer one.
    pub(crate) fn emit_lane_event(&self, project: &ProjectRt, lane: &LaneRt) {
        let name = project.name();
        let _publish = lock(&lane.publish);
        // A worktree without a lane is nobody's business; a cleaned lane already
        // sent `lane.removed`.
        if !lane.st().listed() {
            return;
        }
        let mut summary = lane_summary(&name, &lane.st());
        if lane.st().record.is_some() {
            crate::model::add_shared(&mut summary, &crate::model::shared_containers(project));
        }
        self.fill_queued(&mut summary);
        self.bus
            .emit(events::LANE_CHANGED, json!({"lane": summary}));
    }

    /// Starts git and config watchers for a project once. Returns once the git
    /// watcher is live, so a worktree added right after the request that
    /// registered the project is seen.
    pub(crate) async fn watch_project(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        repository: Option<devlanes_git::Repository>,
    ) {
        if !self.opts.watch || !project.claim_watch() {
            return;
        }
        if let Some(repository) = repository {
            // Registering file-system watches can take seconds on a busy machine,
            // so it runs on the blocking pool; the request waits for it, because
            // until the watches exist a change produces no event at all.
            let watched =
                tokio::task::spawn_blocking(move || devlanes_git::watch(repository)).await;
            let updates = match watched {
                Ok(Ok(updates)) => Some(updates),
                Ok(Err(error)) => {
                    tracing::warn!(%error, "worktree watch unavailable");
                    None
                }
                Err(error) => {
                    tracing::warn!(%error, "worktree watch unavailable");
                    None
                }
            };
            if let Some(mut updates) = updates {
                // Worktrees that changed between the caller's listing and the
                // watches going live left no event behind.
                match self.relist(project).await {
                    Ok(report) => self.apply_policies(project, report).await,
                    Err(error) => tracing::warn!(code = %error.code, "worktree sync failed"),
                }
                let engine = self.clone();
                let project = project.clone();
                tokio::spawn(async move {
                    engine.follow_worktrees(&project, &mut updates).await;
                });
            }
        }
        let engine = self.clone();
        let project = project.clone();
        tokio::spawn(async move { engine.watch_configs(project).await });
    }

    /// Applies each worktree list the git watcher reports until the project goes away.
    async fn follow_worktrees(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        updates: &mut tokio::sync::mpsc::Receiver<
            Result<Vec<devlanes_git::Worktree>, devlanes_git::Error>,
        >,
    ) {
        loop {
            let update = tokio::select! {
                () = project.watch.cancelled() => break,
                update = updates.recv() => update,
            };
            let Some(update) = update else { break };
            let result = match update {
                Ok(list) => {
                    self.sync_worktrees(project, list.iter().map(tree).collect())
                        .await
                }
                // The watcher lists from the worktree the project was found in,
                // which fails once that worktree is removed; `relist` asks another.
                Err(_) => self.relist(project).await,
            };
            match result {
                Ok(report) => self.apply_policies(project, report).await,
                Err(error) => tracing::warn!(code = %error.code, "worktree sync failed"),
            }
        }
    }

    /// Polls each lane's config files and reloads on change. Polling stays correct
    /// across editors that replace files and needs no per-lane OS watch handles.
    async fn watch_configs(self: Arc<Self>, project: Arc<ProjectRt>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                () = project.watch.cancelled() => return,
                _ = tick.tick() => {}
            }
            // Every file of the last load counts, the primary worktree's local overlay
            // and `.env` included. A start in flight keeps the lane's block and layout:
            // the new config is recorded now (`config_changed`), its ports and views
            // once the start ended (`lane_access`).
            for lane in project.lanes() {
                self.fresh_config(&project, &lane).await;
            }
        }
    }

    /// The present worktree of `project` holding `cwd`, deepest first; a
    /// folder's first lane wins over its extra lanes.
    pub(crate) fn lane_at(project: &ProjectRt, cwd: &Path) -> Option<Arc<LaneRt>> {
        let mut best: Option<(usize, bool, Arc<LaneRt>)> = None;
        for lane in project.lanes() {
            let st = lane.st();
            if st.present && path_starts_with(cwd, &st.path) {
                let rank = (st.path.components().count(), st.alias().is_none());
                if best
                    .as_ref()
                    .is_none_or(|(depth, base, _)| rank > (*depth, *base))
                {
                    best = Some((rank.0, rank.1, lane.clone()));
                }
            }
        }
        best.map(|(_, _, lane)| lane)
    }

    /// devlanes runs in `lane`'s worktree: it gets a lane once its config loads,
    /// and `lanes.new_worktrees` applies to it. A lane that has a record already,
    /// or a folder that is gone, is left alone.
    pub(crate) async fn claim_lane(self: &Arc<Self>, project: &Arc<ProjectRt>, lane: &Arc<LaneRt>) {
        {
            let mut st = lane.st();
            if st.wanted || st.record.is_some() || !st.present {
                return;
            }
            st.wanted = true;
        }
        let assigned = {
            let _sync = project.sync.lock().await;
            let assigned = match self.assign_lanes(project).await {
                Ok(assigned) => assigned,
                Err(error) => {
                    tracing::warn!(code = %error.code, "lane assignment failed");
                    Vec::new()
                }
            };
            self.update_moves(project);
            assigned
        };
        self.refresh_views(project).await;
        if !assigned.iter().any(|other| Arc::ptr_eq(other, lane)) {
            // Still waiting (lanes.max, config error): it shows with its reason.
            self.emit_lane(project, lane);
        }
        for other in &assigned {
            self.emit_lane(project, other);
        }
        self.new_lane_policies(project, assigned);
    }

    /// Runs `lanes.new_worktrees` for lanes that just got their record.
    fn new_lane_policies(self: &Arc<Self>, project: &Arc<ProjectRt>, lanes: Vec<Arc<LaneRt>>) {
        for lane in lanes {
            let Some(config) = lane.st().config.loaded().cloned() else {
                continue;
            };
            if lane.st().record.is_none() {
                continue;
            }
            let policy = config.lanes.new_worktrees.unwrap_or(NewWorktrees::Register);
            if matches!(policy, NewWorktrees::Register) {
                continue;
            }
            let env = project.st().last_env.clone();
            let Some(env) = env else {
                let mut st = lane.st();
                st.last_error = Some(RpcError::new(
                    "needs_env",
                    json!({"lane": st.reference(&project.name())}),
                    1,
                ));
                drop(st);
                self.emit_lane(project, &lane);
                continue;
            };
            lane.st().env = Some(env);
            let engine = self.clone();
            let project = project.clone();
            tokio::spawn(async move {
                let setup_only = matches!(policy, NewWorktrees::Setup);
                if let Err(error) = engine.auto_start(&project, &lane, setup_only).await {
                    lane.st().last_error = Some(error);
                    engine.emit_lane(&project, &lane);
                }
            });
        }
    }

    /// Cleans, in the background, the lanes of `project` whose worktree folder has
    /// been gone for [`MISSING_GRACE`] and that run nothing; their logs stay until
    /// retention removes them. Lanes that still run something keep showing as
    /// missing. The grace covers `git worktree move`, which removes the old
    /// folder before git's record names the new one.
    pub(crate) fn sweep_missing(self: &Arc<Self>, project: &Arc<ProjectRt>) {
        let idle: Vec<(Arc<LaneRt>, Duration)> = project
            .lanes()
            .into_iter()
            .filter_map(|lane| {
                let mut st = lane.st();
                if !sweepable(&st) || st.sweeping {
                    return None;
                }
                st.sweeping = true;
                let since = *st.missing_since.get_or_insert_with(std::time::Instant::now);
                drop(st);
                Some((lane, MISSING_GRACE.saturating_sub(since.elapsed())))
            })
            .collect();
        for (lane, wait) in idle {
            let engine = self.clone();
            let project = project.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = engine.shutdown.cancelled() => return,
                }
                if sweepable(&lane.st()) {
                    let reference = lane.st().reference(&project.name());
                    match engine.clean_lane(&project, &lane, None).await {
                        Ok(_) => {
                            tracing::info!(lane = %reference, "cleaned a lane whose worktree is gone")
                        }
                        Err(error) => {
                            tracing::warn!(lane = %reference, code = %error.code, "cleaning a lane whose worktree is gone failed")
                        }
                    }
                }
                lane.st().sweeping = false;
            });
        }
    }

    /// Runs `removed_worktrees` for lanes of one re-list whose folder went away
    /// while they ran something, then cleans the idle ones.
    pub(crate) async fn apply_policies(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        report: SyncReport,
    ) {
        self.new_lane_policies(project, report.added);
        for lane in report.removed {
            // Idle lanes are cleaned by the sweep below.
            if !lane.st().any_running() {
                continue;
            }
            // The config poller can read the removed worktree's config as missing
            // before this runs; its running units still stop, but only a config
            // that says so cleans.
            let policy = {
                let st = lane.st();
                match st.config.loaded() {
                    Some(config) => config
                        .lanes
                        .removed_worktrees
                        .unwrap_or(RemovedWorktrees::Stop),
                    None if st.any_running() => RemovedWorktrees::Stop,
                    None => continue,
                }
            };
            let engine = self.clone();
            let project = project.clone();
            tokio::spawn(async move {
                let result = match policy {
                    RemovedWorktrees::Stop => engine
                        .stop_lane(&project, &lane, None, None, false)
                        .await
                        .map(drop),
                    RemovedWorktrees::Clean => {
                        engine.clean_lane(&project, &lane, None).await.map(drop)
                    }
                };
                if let Err(error) = result {
                    tracing::warn!(code = %error.code, "removed worktree policy failed");
                }
            });
        }
        self.sweep_missing(project);
    }
}

/// Creates a lane runtime around its state.
pub(crate) fn new_lane(worktree: WorktreeId, st: LaneSt) -> Arc<LaneRt> {
    Arc::new(LaneRt {
        worktree,
        op: tokio::sync::RwLock::new(()),
        st: std::sync::Mutex::new(st),
        publish: std::sync::Mutex::new(()),
        watch: tokio_util::sync::CancellationToken::new(),
        power: tokio::sync::Mutex::new(()),
        desired: tokio::sync::Mutex::new(()),
        decide: tokio::sync::Mutex::new(()),
        setup: tokio::sync::Mutex::new(()),
    })
}

/// Converts git's view of a worktree.
fn tree(worktree: &devlanes_git::Worktree) -> Tree {
    Tree {
        identity: worktree.identity.clone(),
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        present: worktree.present,
    }
}

/// Expression context with `project`, `lane` (incl. vars and port) and `env`.
pub(crate) fn lane_context(
    facts: &LaneFacts,
    domain: &str,
    port: u16,
    vars: &indexmap::IndexMap<String, devlanes_config::Scalar>,
    infra: &Value,
    env: &EnvMap,
    dotenv: &EnvMap,
) -> ExpressionContext {
    let mut lane = json!({
        "index": facts.index,
        "name": facts.name,
        "label": facts.label,
        "path": facts.path.display().to_string(),
        "data_path": devlanes_core::paths::forward_slashes(&facts.data_path),
        "domain": domain,
        "port": port,
    });
    if let Some(map) = lane.as_object_mut() {
        for (key, value) in vars {
            if let Ok(value) = serde_json::to_value(value) {
                map.insert(key.clone(), value);
            }
        }
    }
    let mut env_ns = serde_json::Map::new();
    for source in [dotenv, env] {
        for (key, value) in source {
            env_ns.insert(key.clone(), json!(value));
        }
    }
    ExpressionContext {
        project: json!({"name": facts.project, "primary_path": facts.primary_path.display().to_string()}),
        lane,
        infra: if infra.is_null() {
            json!({})
        } else {
            infra.clone()
        },
        env: Value::Object(env_ns),
        ..ExpressionContext::default()
    }
}

/// Container routes from `infra.hosts`, using known host ports. `https_port`
/// is the proxy's, for the URLs status shows.
fn infra_routes(
    config: &ProjectConfig,
    facts: &LaneFacts,
    layout: &devlanes_resolve::LaneLayout,
    infra: &Value,
    env: &EnvMap,
    dotenv: &EnvMap,
    https_port: Option<u16>,
) -> Vec<crate::model::InfraRoute> {
    let context = lane_context(
        facts,
        &layout.domain,
        layout.port,
        &layout.vars,
        infra,
        env,
        dotenv,
    )
    // Prepared once: every host template below is evaluated against it.
    .prepare();
    let as_port = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
    };
    let mut routes = Vec::new();
    for (container, hosts) in &config.infra.hosts {
        // Ports are known once the container's infra ran; until then it has no route.
        let Some(entry) = infra.get(container) else {
            continue;
        };
        for host in hosts {
            let (template, port, fallback, upstream) = match host {
                devlanes_config::HostConfig::Name(name) => (name, None, None, false),
                devlanes_config::HostConfig::Route(route) => (
                    &route.host,
                    route.port.as_deref(),
                    route.fallback.as_ref(),
                    matches!(
                        route.host_header,
                        Some(devlanes_config::HostHeader::Upstream)
                    ),
                ),
            };
            // A named port is a key of `infra.<container>.ports`: the container
            // port for Compose services, the port's name for plain containers.
            let port = match port {
                Some(name) => as_port(entry.get("ports").and_then(|ports| ports.get(name))),
                None => as_port(entry.get("port")),
            };
            let Some(port) = port else {
                tracing::debug!(%container, "infra host route has no published port yet");
                continue;
            };
            let fallback = match fallback {
                Some(value) => match value.evaluate(&context) {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::debug!(%container, message = %error.message, "infra host fallback unresolved");
                        continue;
                    }
                },
                None => false,
            };
            if let Ok(host) = template.evaluate(&context) {
                let url =
                    (!host.contains('*')).then(|| devlanes_resolve::https_url(&host, https_port));
                routes.push(crate::model::InfraRoute {
                    host,
                    container: container.clone(),
                    port,
                    fallback,
                    host_header_upstream: upstream,
                    url,
                });
            }
        }
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// An unregistered preset is refused with the closest registered name.
    fn unknown_preset_names_the_closest() {
        let config = devlanes_config::parse_project_with::<OpaqueExtensions>(
            "project: demo\nservices:\n  web: { run_script: { file: package.json, name: dev }, preset: vtie }\n",
            None,
        )
        .expect("parses");
        let parse: Arc<crate::integrations::PresetParser> = Arc::new(|_| Ok(()));
        let checks = ConfigChecks {
            presets: ["vite", "next"]
                .map(|name| crate::integrations::PresetEntry {
                    name,
                    parse: parse.clone(),
                })
                .to_vec(),
            contributors: Vec::new(),
        };
        let error = check_config(&config, &checks).expect_err("unknown preset");
        assert!(
            error.message.contains("did you mean `vite`"),
            "{}",
            error.message
        );
    }

    #[test]
    /// `.env` parsing handles export, quotes and inline comments.
    fn dotenv_forms() {
        let env = parse_dotenv("# c\nexport A=1\nB=\"two words\"\nC='x#y'\nD=plain # note\n=bad\n");
        assert_eq!(env["A"], "1");
        assert_eq!(env["B"], "two words");
        assert_eq!(env["C"], "x#y");
        assert_eq!(env["D"], "plain");
        assert_eq!(env.len(), 4);
    }
}
