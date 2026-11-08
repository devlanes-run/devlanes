//! The request's projects and lane. This module is the only code that turns a
//! request (its context, units and method) into projects, a lane or a project
//! file. Finding them changes nothing; registering a project and claiming a lane
//! are explicit steps the method's [`Policy`] allows.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use devlanes_config::ProjectConfig;
use devlanes_core::{
    WorktreeId,
    paths::same_path,
    ports::{PortBlock, allocate_offsets, block_size},
    refs::{LaneCandidate, LaneRef, ReferenceError, ResolveContext, UnitRef},
};
use devlanes_proto::{Context, Method, RpcError};
use devlanes_resolve::LaneFacts;
use serde_json::{Value, json};

use crate::{
    engine::{Inner, LanePair, reference_error},
    model::{EnvMap, LaneRt, LaneView, ProjectRt},
    registry::{internal, literal, project_ambiguous, read_dotenv, tree},
};

/// What a method may change while its project and lane are found. [`Policy::of`]
/// is the only place that decides it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    /// Registers the project a `--file`, or the cwd's project file, names when the
    /// daemon does not know it.
    pub register: bool,
    /// Which worktree the request makes a lane.
    pub claim: Claim,
    /// With `-p` or `--file` and the cwd outside the project's repository: its main
    /// lane; otherwise `lane_required`.
    pub main_lane: bool,
    /// Lists the target project's worktrees from git before resolving.
    pub relist: bool,
    /// A request whose project nobody registered is nothing to do: lane lookups give
    /// `None` and [`Inner::read_scope`] an empty scope, so `stop` (with or without
    /// units, `--mine` too) answers an empty page and `lane gc` relists nothing.
    /// Without it, such a request is `project_not_started`.
    pub absent_is_empty: bool,
}

/// Which worktree a request makes a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// None.
    No,
    /// Only a lane found from the cwd, with no `-p` and no lane reference, in a cwd
    /// that resolved to one project: running devlanes in a worktree to look at it
    /// makes it a lane.
    Cwd,
    /// The lane the request targets, however it was named.
    Target,
}

impl Policy {
    /// A command registers the project and claims the lane it targets when it acts
    /// on that lane (lifecycle commands, infra), when its answer is the lane's
    /// allocated values (ports, URLs, environment: `env`, `url`), or when it lists
    /// the cwd's lanes for a person (`status`, `lane ls`, which claim the cwd's lane
    /// only). Every other command reads what exists: in a project nobody registered
    /// it answers `project_not_started`, and `stop`, `stop --mine` and `lane gc`
    /// treat that as nothing to do. Infra commands register because shared infra
    /// runs with ports allocated to the project.
    pub(crate) fn of(method: Method) -> Self {
        let read = Self {
            register: false,
            claim: Claim::No,
            main_lane: true,
            relist: false,
            absent_is_empty: false,
        };
        let acts = Self {
            register: true,
            claim: Claim::Target,
            ..read
        };
        let lists = Self {
            register: true,
            claim: Claim::Cwd,
            ..read
        };
        match method {
            Method::Start => Self {
                relist: true,
                ..acts
            },
            Method::Restart
            | Method::Reload
            | Method::InfraUp
            | Method::InfraDown
            | Method::InfraStatus
            | Method::InfraLogs
            | Method::Env
            | Method::Url => acts,
            Method::Run | Method::Exec | Method::LaneNew => Self {
                main_lane: false,
                ..acts
            },
            Method::Status => lists,
            Method::LaneList => Self {
                relist: true,
                ..lists
            },
            Method::Stop => Self {
                absent_is_empty: true,
                ..read
            },
            Method::LaneGc => Self {
                main_lane: false,
                absent_is_empty: true,
                ..read
            },
            Method::LaneClean | Method::LaneCleanPlan => Self {
                main_lane: false,
                ..read
            },
            _ => read,
        }
    }
}

/// The request's projects, found without registering or claiming anything.
pub(crate) enum Found {
    /// Registered projects: the one named, the cwd's, or several sharing the folder.
    /// `in_repo`: the cwd is inside their repository (a listed worktree, or one git
    /// reports and [`Inner::find`] just listed), so the main lane never stands in.
    Projects {
        projects: Vec<Arc<ProjectRt>>,
        in_repo: bool,
    },
    /// A project file (`--file`, or the nearest `devlanes.yaml`) that loads and
    /// names a project the daemon does not know.
    Unregistered {
        file: PathBuf,
        config: Arc<ProjectConfig>,
    },
    /// No project file and no lane holds the cwd. `searched_to` is the last
    /// directory the search looked in.
    Nothing { searched_to: Option<PathBuf> },
}

/// A lane [`Inner::locate`] found, and whether the cwd named it (rather than `-p`'s
/// main lane or the `--file`'s folder).
struct Located {
    lane: LanePair,
    by_cwd: bool,
}

/// Where a lane reference resolves ([`Inner::reference_projects`]).
pub(crate) enum RefProjects {
    /// Among these projects; every project when empty.
    In(Vec<Arc<ProjectRt>>),
    /// In the project `file` names, which nobody registered.
    Unregistered {
        file: PathBuf,
        config: Arc<ProjectConfig>,
    },
}

/// A lane-targeting read that can answer from the project file alone.
pub(crate) enum LaneOrFile {
    Lane(LanePair),
    File {
        file: PathBuf,
        config: Arc<ProjectConfig>,
    },
}

/// What a list request covers ([`Inner::read_scope`]).
pub(crate) enum ReadScope {
    /// Every project; its lanes as of the request.
    All(Vec<LanePair>),
    /// The `-p` or `--file` project, the cwd's, or every project sharing the cwd's
    /// folder, with lanes they gain later.
    Projects(Vec<Arc<ProjectRt>>),
    /// The cwd's file names a project nobody registered: no lanes.
    Unregistered(String),
    /// The `--lane`.
    Lane(LanePair),
}

impl ReadScope {
    /// The lanes covered now.
    pub(crate) fn lanes(&self) -> Vec<LanePair> {
        match self {
            Self::All(lanes) => lanes.clone(),
            Self::Projects(projects) => projects
                .iter()
                .flat_map(|project| {
                    project
                        .lanes()
                        .into_iter()
                        .map(move |lane| (project.clone(), lane))
                })
                .collect(),
            Self::Unregistered(_) => Vec::new(),
            Self::Lane(pair) => vec![pair.clone()],
        }
    }

    /// Whether the lane `reference` of `project` is covered, including lanes created
    /// after the scope was built (a `status --watch` that outlives a `lane new`).
    pub(crate) fn covers(&self, project: &str, reference: &str) -> bool {
        match self {
            Self::All(_) => true,
            Self::Projects(projects) => projects.iter().any(|p| p.name() == project),
            Self::Unregistered(name) => name == project,
            Self::Lane((p, lane)) => reference == lane.st().reference(&p.name()),
        }
    }
}

impl Inner {
    /// Candidates for lane references: every lane `status` shows (with a record, or
    /// waiting for one) and, matched only when no lane does, worktrees that are no
    /// lane. Matches map back by worktree.
    pub(crate) fn candidates(&self) -> (Vec<LaneCandidate>, BTreeMap<WorktreeId, LanePair>) {
        let mut candidates = Vec::new();
        let mut map = BTreeMap::new();
        for (project, lane) in self.all_lanes() {
            let name = project.name();
            let st = lane.st();
            let listed = st.listed();
            let alias = st.alias().map(|(_, alias)| alias.to_owned());
            // A cleaned extra lane that nobody uses again is nothing to name.
            if alias.is_some() && !listed {
                continue;
            }
            let record = st.record.as_ref();
            // An extra lane answers to its index and its own name; the branch and
            // the folder keep naming the folder's first lane.
            let (branch, path) = match alias {
                Some(_) => (None, PathBuf::new()),
                None => (st.branch.clone(), st.path.clone()),
            };
            let candidate = LaneCandidate {
                id: record.map(|record| record.id),
                worktree: lane.worktree,
                project_id: project.id,
                project: name,
                index: record.map(|record| record.index),
                branch,
                name: alias
                    .or_else(|| record.map(|record| record.name.clone()))
                    .unwrap_or_default(),
                path,
                lane: listed,
            };
            drop(st);
            map.insert(lane.worktree, (project.clone(), lane.clone()));
            candidates.push(candidate);
        }
        (candidates, map)
    }

    /// The request's projects: the `--file`'s, `-p`'s, else the cwd's (the lane
    /// project's when the cwd is in one of its lane folders), registering nothing
    /// and claiming nothing. A known project is brought up to date with the file
    /// found and, when the cwd is in none of its listed worktrees, with the worktree
    /// list git gives for the cwd.
    pub(crate) async fn find(self: &Arc<Self>, context: &Context) -> Result<Found, RpcError> {
        match self.find_projects(context).await? {
            Found::Projects { projects, .. } => {
                let in_repo = self.in_repo(&projects, &context.cwd).await?;
                Ok(Found::Projects { projects, in_repo })
            }
            found => Ok(found),
        }
    }

    /// [`Self::find`] before the repository check.
    async fn find_projects(self: &Arc<Self>, context: &Context) -> Result<Found, RpcError> {
        if let Some(file) = &context.file {
            return self.found_file(file).await;
        }
        if let Some(name) = &context.project {
            return Ok(Found::Projects {
                projects: vec![self.project_by_name(name)?],
                in_repo: false,
            });
        }
        if context.cwd.as_os_str().is_empty() {
            return Ok(Found::Nothing { searched_to: None });
        }
        let search = {
            let cwd = context.cwd.clone();
            tokio::task::spawn_blocking(move || devlanes_config::search_project_file(&cwd))
                .await
                .map_err(|error| internal("io", error))?
                .map_err(|error| internal("io", error))?
        };
        let (file, searched_to) = match search {
            devlanes_config::ConfigSearch::Found(file) => (Some(file), None),
            devlanes_config::ConfigSearch::Missing { searched_to } => (None, Some(searched_to)),
        };
        // A `devlanes.prod.yaml` project's services run where `devlanes.yaml` names
        // another project, so there the cwd alone would pick the wrong one.
        if let Some(name) = &context.lane_project
            && let Some(project) = self.lane_folder_project(name, &context.cwd, file.as_deref())
        {
            return Ok(Found::Projects {
                projects: vec![project],
                in_repo: false,
            });
        }
        if let Some(file) = file {
            if let Some(project) = self.project_reading(&file).await {
                return Ok(Found::Projects {
                    projects: vec![project],
                    in_repo: false,
                });
            }
            return self.found_file(&file).await;
        }
        // A project file below the worktree root seen from above it, or a worktree
        // whose file is gone.
        let holding = self.projects_holding(&context.cwd);
        if !holding.is_empty() {
            return Ok(Found::Projects {
                projects: holding,
                in_repo: false,
            });
        }
        if let Some(name) = &context.lane_project {
            return Ok(Found::Projects {
                projects: vec![self.project_by_name(name)?],
                in_repo: false,
            });
        }
        Ok(Found::Nothing { searched_to })
    }

    /// The project `file` names: known (and brought up to date with the file) or not.
    async fn found_file(self: &Arc<Self>, file: &Path) -> Result<Found, RpcError> {
        let config = self.parse_project_file(file).await?;
        match self.known_project(&config.project) {
            Some(project) => {
                self.sync_known(&project, file, &config).await?;
                Ok(Found::Projects {
                    projects: vec![project],
                    in_repo: false,
                })
            }
            None => Ok(Found::Unregistered {
                file: file.to_owned(),
                config,
            }),
        }
    }

    /// Whether `cwd` is inside the repository of one of `projects`. A listed
    /// worktree holding it answers without git. Otherwise git is asked once, and a
    /// project whose repository it is gets the worktree list git reports, so a
    /// worktree added since the last listing resolves for every caller alike.
    async fn in_repo(
        self: &Arc<Self>,
        projects: &[Arc<ProjectRt>],
        cwd: &Path,
    ) -> Result<bool, RpcError> {
        if cwd.as_os_str().is_empty() {
            return Ok(false);
        }
        if projects
            .iter()
            .any(|project| Self::lane_at(project, cwd).is_some())
        {
            return Ok(true);
        }
        let mut repository = None;
        for project in projects {
            let (common_dir, primary) = {
                let st = project.st();
                (st.common_dir.clone(), st.primary_branch.clone())
            };
            let Some(common_dir) = common_dir else {
                continue;
            };
            if repository.is_none() {
                let found = devlanes_git::discover(cwd, Some(&primary))
                    .await
                    .map_err(|error| internal("git_failed", error))?;
                repository = Some(found);
            }
            if let Some(Some(repo)) = &repository
                && same_path(&repo.common_dir, &common_dir)
            {
                self.sync_worktrees(project, repo.worktrees.iter().map(tree).collect())
                    .await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// [`Self::find`], registering an unregistered project when `policy` does.
    pub(crate) async fn projects(
        self: &Arc<Self>,
        context: &Context,
        policy: Policy,
    ) -> Result<Found, RpcError> {
        match self.find(context).await? {
            Found::Unregistered { file, config } if policy.register => {
                let projects = vec![self.register_file(&file, &config).await?];
                let in_repo = self.in_repo(&projects, &context.cwd).await?;
                Ok(Found::Projects { projects, in_repo })
            }
            found => Ok(found),
        }
    }

    /// The one project of `found`: among several sharing the folder, the request's
    /// `lane_project`, else `project_ambiguous`.
    fn pick_project(&self, found: &Found, context: &Context) -> Result<Arc<ProjectRt>, RpcError> {
        match found {
            Found::Projects { projects, .. } => match projects.as_slice() {
                [project] => Ok(project.clone()),
                _ => context
                    .lane_project
                    .as_ref()
                    .and_then(|name| projects.iter().find(|project| project.name() == *name))
                    .cloned()
                    .ok_or_else(|| project_ambiguous(&context.cwd, projects)),
            },
            Found::Unregistered { file, config } => Err(project_not_started(&config.project, file)),
            Found::Nothing { searched_to } if !context.cwd.as_os_str().is_empty() => {
                Err(no_config(&context.cwd, searched_to.as_deref()))
            }
            Found::Nothing { .. } => {
                let (candidates, _) = self.candidates();
                let mut names: Vec<String> = candidates
                    .iter()
                    .filter(|candidate| candidate.lane)
                    .map(LaneCandidate::label)
                    .collect();
                names.sort();
                Err(RpcError::new(
                    "not_found",
                    json!({"reference": context.cwd, "candidates": names}),
                    3,
                ))
            }
        }
    }

    /// The lane of `found` for a request without a lane reference, from memory only:
    /// the worktree holding the cwd; outside the project's repository the `--file`'s
    /// folder, then for `-p` or `--file` the main lane when `policy` allows it, else
    /// `lane_required`. Inside the repository without a lane there, `not_found`.
    fn locate(
        &self,
        found: &Found,
        context: &Context,
        policy: Policy,
    ) -> Result<Located, RpcError> {
        let project = self.pick_project(found, context)?;
        if let Some(lane) = Self::lane_at(&project, &context.cwd) {
            return Ok(Located {
                lane: (project, lane),
                by_cwd: true,
            });
        }
        let in_repo = matches!(found, Found::Projects { in_repo: true, .. });
        if !in_repo {
            if let Some(lane) = context
                .file
                .as_deref()
                .and_then(Path::parent)
                .and_then(|dir| Self::lane_at(&project, dir))
            {
                return Ok(Located {
                    lane: (project, lane),
                    by_cwd: false,
                });
            }
            // `DEVLANES_LANE_PROJECT` alone gets no fallback: a lane 2 process running
            // devlanes from `/tmp` must not land on lane 0.
            if context.project.is_some() || context.file.is_some() {
                if !policy.main_lane {
                    return Err(lane_required(&project));
                }
                return main_lane(&project).map(|lane| Located {
                    lane,
                    by_cwd: false,
                });
            }
        }
        Err(RpcError::new(
            "not_found",
            json!({"reference": context.cwd, "candidates": lane_refs(&project)}),
            3,
        ))
    }

    /// [`Self::locate`] after the relist and before the claim `policy` asks for.
    async fn lane_in(
        self: &Arc<Self>,
        found: Found,
        context: &Context,
        policy: Policy,
    ) -> Result<LanePair, RpcError> {
        if policy.relist
            && let Ok(project) = self.pick_project(&found, context)
        {
            let _ = self.relist(&project).await;
        }
        let located = self.locate(&found, context, policy)?;
        let one_project = matches!(&found, Found::Projects { projects, .. } if projects.len() == 1);
        let claim = match policy.claim {
            Claim::Target => true,
            Claim::Cwd => located.by_cwd && context.project.is_none() && one_project,
            Claim::No => false,
        };
        let (project, lane) = located.lane;
        if claim {
            self.claim_lane(&project, &lane).await;
        }
        Ok((project, lane))
    }

    /// The request's lane: the `--lane` reference, else the cwd's (or `-p`'s main
    /// lane), with the effects `policy` allows. `None` only when `policy` treats a
    /// project nobody registered as nothing to do.
    pub(crate) async fn lane_or_nothing(
        self: &Arc<Self>,
        context: &Context,
        policy: Policy,
    ) -> Result<Option<LanePair>, RpcError> {
        if let Some(reference) = &context.lane {
            return self.resolve_reference(reference, context, policy).await;
        }
        let found = match self.projects(context, policy).await? {
            Found::Unregistered { .. } if policy.absent_is_empty => return Ok(None),
            found => found,
        };
        self.lane_in(found, context, policy).await.map(Some)
    }

    /// [`Self::lane_or_nothing`] for methods that always need a lane.
    pub(crate) async fn lane(
        self: &Arc<Self>,
        context: &Context,
        policy: Policy,
    ) -> Result<LanePair, RpcError> {
        self.lane_or_nothing(context, policy)
            .await?
            .ok_or_else(|| absent(context))
    }

    /// The lane of a read that can answer from the project file alone (`config show`,
    /// `task ls`, `profile ls`): the file when it names a project nobody registered.
    pub(crate) async fn lane_or_file(
        self: &Arc<Self>,
        context: &Context,
        policy: Policy,
    ) -> Result<LaneOrFile, RpcError> {
        if context.lane.is_some() {
            return self.lane(context, policy).await.map(LaneOrFile::Lane);
        }
        match self.find(context).await? {
            Found::Unregistered { file, config } => Ok(LaneOrFile::File { file, config }),
            found => self
                .lane_in(found, context, policy)
                .await
                .map(LaneOrFile::Lane),
        }
    }

    /// Resolves the `[project@]lane` `reference` with the effects `policy` allows.
    /// A reference naming its project never resolves through the cwd; one naming a
    /// project the daemon does not know reaches it only when the request's `--file`
    /// or the cwd's file names it, so a mistyped name cannot bring back a project
    /// `lane clean` forgot. A reference never counts as found from the cwd, so only
    /// [`Claim::Target`] claims its lane.
    pub(crate) async fn resolve_reference(
        self: &Arc<Self>,
        reference: &str,
        context: &Context,
        policy: Policy,
    ) -> Result<Option<LanePair>, RpcError> {
        let parsed: LaneRef = reference.parse().map_err(reference_error)?;
        let scope = match self
            .reference_projects(&parsed, context, Some(policy))
            .await?
        {
            RefProjects::In(scope) => scope,
            RefProjects::Unregistered { file, config } if policy.register => {
                vec![self.register_file(&file, &config).await?]
            }
            RefProjects::Unregistered { .. } if policy.absent_is_empty => return Ok(None),
            RefProjects::Unregistered { file, config } => {
                return Err(project_not_started(&config.project, &file));
            }
        };
        if policy.relist {
            for project in &scope {
                let _ = self.relist(project).await;
            }
        }
        let (project, lane) = self.match_reference(&parsed, &scope, context)?;
        if policy.claim == Claim::Target {
            self.claim_lane(&project, &lane).await;
        }
        Ok(Some((project, lane)))
    }

    /// The lane `parsed` names among the candidates of `scope` (every project when
    /// empty), from memory only.
    pub(crate) fn match_reference(
        &self,
        parsed: &LaneRef,
        scope: &[Arc<ProjectRt>],
        context: &Context,
    ) -> Result<LanePair, RpcError> {
        let (candidates, map) = self.candidates();
        let resolve = ResolveContext {
            projects: scope.iter().map(|project| project.id).collect(),
            lane: None,
            cwd: context.cwd.clone(),
            home: context
                .env
                .as_ref()
                .and_then(|env| env.get("HOME"))
                .map(PathBuf::from),
        };
        let found = devlanes_core::refs::resolve_lane(parsed, &candidates, &resolve)
            .map_err(reference_error)?;
        map.get(&found.worktree).cloned().ok_or_else(|| {
            reference_error(ReferenceError::NotFound {
                reference: parsed.selector.clone(),
                candidates: Vec::new(),
            })
        })
    }

    /// The projects `parsed` resolves in. A reference naming its project never
    /// resolves through the cwd; one naming a project the daemon does not know
    /// reaches it only when the request's `--file` or the cwd's file names it, so a
    /// mistyped name cannot bring back a project `lane clean` forgot. A short one
    /// resolves among the request's projects, found under `policy` (which may
    /// register them), or with no policy without registering anything.
    pub(crate) async fn reference_projects(
        self: &Arc<Self>,
        parsed: &LaneRef,
        context: &Context,
        policy: Option<Policy>,
    ) -> Result<RefProjects, RpcError> {
        let found = match &parsed.project {
            Some(name) => {
                if let Some(project) = self.known_project(name) {
                    return Ok(RefProjects::In(vec![project]));
                }
                match self.find(context).await {
                    Ok(Found::Unregistered { file, config }) if config.project == *name => {
                        return Ok(RefProjects::Unregistered { file, config });
                    }
                    // Whatever else is at the cwd, a broken file included, says
                    // nothing about a project the reference names.
                    _ => return Err(self.unknown_project(name)),
                }
            }
            None => match policy {
                Some(policy) => self.projects(context, policy).await?,
                None => self.find(context).await?,
            },
        };
        Ok(match found {
            Found::Projects { projects, .. } => RefProjects::In(narrow(projects, context)),
            Found::Nothing { .. } => RefProjects::In(Vec::new()),
            Found::Unregistered { file, config } => RefProjects::Unregistered { file, config },
        })
    }

    /// Splits `[lane:]unit` selectors and resolves their lane. All selectors that
    /// name a lane must name the same one; unqualified ones use the request's lane.
    /// `None` as [`Self::lane_or_nothing`] gives it.
    pub(crate) async fn units_or_nothing(
        self: &Arc<Self>,
        units: &[String],
        context: &Context,
        policy: Policy,
    ) -> Result<Option<(LanePair, Vec<String>)>, RpcError> {
        let mut lane_ref: Option<String> = None;
        let mut selectors = Vec::new();
        for unit in units {
            let parsed: UnitRef = unit.parse().map_err(reference_error)?;
            if let Some(lane) = &parsed.lane {
                let text = match &lane.project {
                    Some(project) => format!("{project}@{}", lane.selector),
                    None => lane.selector.clone(),
                };
                if lane_ref.as_ref().is_some_and(|known| *known != text) {
                    return Err(RpcError::new("mixed_lanes", json!({"units": units}), 2));
                }
                lane_ref = Some(text);
            }
            selectors.push(parsed.selector);
        }
        let by = if lane_ref.is_some() {
            "unit reference"
        } else if context.lane.is_some() {
            "--lane"
        } else {
            "working directory"
        };
        let pair = match lane_ref {
            Some(reference) => self.resolve_reference(&reference, context, policy).await?,
            None => self.lane_or_nothing(context, policy).await?,
        };
        let Some(pair) = pair else {
            return Ok(None);
        };
        tracing::debug!(
            lane = %pair.1.st().reference(&pair.0.name()),
            by,
            cwd = %context.cwd.display(),
            selectors = %selectors.join(","),
            "lane resolved"
        );
        Ok(Some((pair, selectors)))
    }

    /// [`Self::units_or_nothing`] for methods that always need a lane.
    pub(crate) async fn lane_for_units(
        self: &Arc<Self>,
        units: &[String],
        context: &Context,
        policy: Policy,
    ) -> Result<(LanePair, Vec<String>), RpcError> {
        self.units_or_nothing(units, context, policy)
            .await?
            .ok_or_else(|| absent(context))
    }

    /// What a list request covers: every project for `all`, the `--lane`, the `--file`
    /// or `-p` project, else the cwd's project or every project sharing the cwd's
    /// folder (a repository root holding several), else every project. Under
    /// [`Claim::Cwd`] or [`Claim::Target`] the cwd's lane is claimed when the scope is
    /// the cwd's one project.
    pub(crate) async fn read_scope(
        self: &Arc<Self>,
        context: &Context,
        all: bool,
        policy: Policy,
    ) -> Result<ReadScope, RpcError> {
        // Never registers a project: completion and the TUI's monitor ask `all`.
        if all {
            return Ok(ReadScope::All(self.all_lanes()));
        }
        if context.lane.is_some() {
            return Ok(ReadScope::Lane(self.lane(context, policy).await?));
        }
        match self.projects(context, policy).await? {
            Found::Projects { projects, .. } => {
                if policy.relist {
                    for project in &projects {
                        let _ = self.relist(project).await;
                    }
                }
                // `-p` from elsewhere and a folder shared by several projects say
                // nothing about which lane.
                if policy.claim != Claim::No
                    && context.project.is_none()
                    && let [project] = projects.as_slice()
                    && let Some(lane) = Self::lane_at(project, &context.cwd)
                {
                    self.claim_lane(project, &lane).await;
                }
                Ok(ReadScope::Projects(projects))
            }
            Found::Unregistered { config, .. } => {
                Ok(ReadScope::Unregistered(config.project.clone()))
            }
            Found::Nothing { .. } => {
                if policy.relist {
                    for project in self.all_projects() {
                        let _ = self.relist(&project).await;
                    }
                }
                Ok(ReadScope::All(self.all_lanes()))
            }
        }
    }

    /// The reference of the request's own lane as `method` would resolve it, for a
    /// client that completes `method` (`status`'s `here`). It registers, relists
    /// and claims nothing, whatever `method` would, so a project nobody started
    /// (or one `lane clean` forgot) answers `project_not_started` and stays as it
    /// is.
    pub(crate) async fn here(
        self: &Arc<Self>,
        context: &Context,
        method: Method,
    ) -> Result<String, RpcError> {
        let (project, lane) = match &context.lane {
            Some(reference) => {
                let parsed: LaneRef = reference.parse().map_err(reference_error)?;
                match self.reference_projects(&parsed, context, None).await? {
                    RefProjects::In(scope) => self.match_reference(&parsed, &scope, context)?,
                    RefProjects::Unregistered { file, config } => {
                        return Err(project_not_started(&config.project, &file));
                    }
                }
            }
            None => {
                let found = self.find(context).await?;
                self.locate(&found, context, Policy::of(method))?.lane
            }
        };
        let reference = lane.st().reference(&project.name());
        Ok(reference)
    }

    /// Facts of lane 0 of the project `config` describes, laid out without storing
    /// or reserving anything: the primary worktree `root` on `branch`, with the
    /// project file in `project_dir` below it, and a data path that is never created.
    pub(crate) fn provisional_facts(
        &self,
        config: &ProjectConfig,
        root: &Path,
        branch: Option<&str>,
        project_dir: &Path,
    ) -> LaneFacts {
        let data_path = self.project_data_path(&config.project).join("provisional");
        LaneFacts::new(
            config.project.clone(),
            root,
            0,
            root,
            data_path,
            branch.map(str::to_owned),
        )
        .with_project_dir(project_dir)
        .with_domain_suffix(self.domain_suffix(Some(config)))
    }

    /// The layout of [`Self::provisional_facts`]' lane, with a block at the start of
    /// the port pool and offsets in order. `None` when it cannot be laid out without
    /// a running lane.
    pub(crate) async fn provisional_view(
        &self,
        config: &ProjectConfig,
        facts: &LaneFacts,
    ) -> Option<LaneView> {
        let needs = devlanes_resolve::port_needs(config, &self.integ.contributors).ok()?;
        let size = block_size(needs.min_block_size(), literal(&config.lanes.block_size)).ok()?;
        let block = PortBlock::new(self.pool().ok()?.start, size).ok()?;
        let offsets =
            allocate_offsets(&BTreeMap::new(), &needs.names, &needs.reserved, &[block]).ok()?;
        let dotenv = read_dotenv(facts.project_dir.clone(), facts.project_dir.clone()).await;
        let view = self.view_of(
            &format!("{}@{}", facts.project, facts.index),
            config,
            facts,
            (block, &offsets),
            (&EnvMap::new(), &dotenv, &Value::Null),
            None,
        );
        match view {
            Ok(view) => Some(view),
            Err(error) => {
                tracing::debug!(code = %error.code, "no provisional layout");
                None
            }
        }
    }
}

/// Among several projects sharing the folder, the request's `lane_project`, else
/// all of them.
fn narrow(projects: Vec<Arc<ProjectRt>>, context: &Context) -> Vec<Arc<ProjectRt>> {
    match context
        .lane_project
        .as_ref()
        .and_then(|name| projects.iter().find(|project| project.name() == *name))
    {
        Some(project) if projects.len() > 1 => vec![project.clone()],
        _ => projects,
    }
}

/// `project`'s main lane (index 0), else its only assigned lane; `ambiguous`
/// between several others and `not_found` without any.
fn main_lane(project: &Arc<ProjectRt>) -> Result<LanePair, RpcError> {
    let lanes: Vec<(u32, Arc<LaneRt>)> = project
        .lanes()
        .into_iter()
        .filter_map(|lane| {
            let st = lane.st();
            let index = st.record.as_ref().filter(|_| st.present)?.index;
            drop(st);
            Some((index, lane))
        })
        .collect();
    if let Some((_, lane)) = lanes.iter().find(|(index, _)| *index == 0) {
        return Ok((project.clone(), lane.clone()));
    }
    if let [(_, lane)] = lanes.as_slice() {
        return Ok((project.clone(), lane.clone()));
    }
    let code = if lanes.is_empty() {
        "not_found"
    } else {
        "ambiguous"
    };
    Err(RpcError::new(
        code,
        json!({"reference": project.name(), "candidates": lane_refs(project)}),
        3,
    ))
}

/// `lane_required` (exit 2): `-p` named `project` from outside its worktrees and the
/// command needs `--lane`; `candidates` are the project's lanes.
pub(crate) fn lane_required(project: &ProjectRt) -> RpcError {
    RpcError::new(
        "lane_required",
        json!({"project": project.name(), "candidates": lane_refs(project)}),
        2,
    )
}

/// References of `project`'s present, assigned lanes, sorted.
fn lane_refs(project: &ProjectRt) -> Vec<String> {
    let name = project.name();
    let mut refs: Vec<String> = project
        .lanes()
        .iter()
        .filter_map(|lane| {
            let st = lane.st();
            (st.present && st.record.is_some()).then(|| st.reference(&name))
        })
        .collect();
    refs.sort();
    refs
}

/// `no_config` (exit 3) for a cwd with no `devlanes.yaml` up to its repository
/// root: `path` is the cwd, `searched_to` the last directory looked in.
pub(crate) fn no_config(cwd: &Path, searched_to: Option<&Path>) -> RpcError {
    RpcError::new(
        "no_config",
        json!({"path": cwd, "searched_to": searched_to}),
        3,
    )
}

/// `project_not_started` (exit 3): `file` names `project`, which nobody started, so
/// a command that only reads has nothing to read.
pub(crate) fn project_not_started(project: &str, file: &Path) -> RpcError {
    RpcError::new(
        "project_not_started",
        json!({"project": project, "file": file}),
        3,
    )
}

/// `project_not_started` for a lookup whose policy gave no lane, which only
/// policies that treat a project nobody registered as nothing to do produce.
fn absent(context: &Context) -> RpcError {
    RpcError::new("project_not_started", json!({"path": context.cwd}), 3)
}
