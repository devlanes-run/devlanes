//! Reading a lane's live infra into its owner, [`crate::model::LaneInfra`].
//! Docker can give a restarted container new ephemeral host ports, so what a
//! lane stores is checked again when something says it may be stale: a
//! container event, a lost stretch of events, a start or restart into the
//! running lane, or a command that needs the values. Checking is one engine
//! observation of the keys in doubt; ports are read again only for keys whose
//! runs changed. Nothing restarts a unit by itself; services that read the old
//! port show drift.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use devlanes_proto::RpcError;
use serde_json::Value;
use tokio::time::Instant;

use crate::{
    engine::Inner,
    integrations::{ContainerRun, InfraContainer, InfraError, InfraSpec, InfraUp, Observation},
    model::{CurrentInfra, InfraRead, LaneRt, ProjectRt},
};

/// Gathers the `die`, `start` and `restart` events of one restart (and of
/// several containers restarting together) into one refresh.
const REFRESH_DELAY: Duration = Duration::from_millis(500);
/// The longest a background refresh or the read after a daemon restart may
/// take; its keys are checked again by the next command when it runs out.
pub(crate) const REFRESH_BOUND: Duration = Duration::from_secs(30);
/// The longest wait for restarted containers to become healthy, as a bring-up's
/// health wait.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a command waits for the lane's infra to be checked.
pub(crate) const COMMAND_INFRA_WAIT: Duration = Duration::from_secs(5);

/// What [`Inner::settle_infra`] did.
pub(crate) enum Settled {
    /// No key needed checking, none changed and none was waited for.
    Nothing,
    /// Keys were checked or waited for; `changed` when the namespace changed.
    Done { changed: bool },
}

/// Why [`Inner::refresh_infra`] stored nothing new.
pub(crate) enum RefreshFailed {
    /// The lane's infra spec or its declared names could not be had, so which
    /// names the lane has is unknown.
    Names(RpcError),
    /// The engine or Compose failed or did not answer in time; the keys the
    /// call asked about are unknown until the next check.
    Read(RpcError),
}

impl From<RefreshFailed> for RpcError {
    fn from(failed: RefreshFailed) -> Self {
        match failed {
            RefreshFailed::Names(error) | RefreshFailed::Read(error) => error,
        }
    }
}

/// Stores what a bring-up read after its `up`, whose health wait confirmed
/// every container that ran, and the setup env `up` gave. Only for callers
/// holding the lane's `op` (a start, `infra up`): a changed namespace marks
/// the views for refreshing, which the start's resolve phase does, and which
/// would keep `defer_refresh` from queuing its task if set outside `op`.
pub(crate) fn store_infra_up(lane: &LaneRt, read: InfraRead, up: InfraUp) {
    let running: Vec<(String, Vec<ContainerRun>)> = read
        .after
        .iter()
        .filter_map(|(key, observation)| match observation {
            Observation::Running(runs) => Some((key.clone(), runs.clone())),
            _ => None,
        })
        .collect();
    let mut st = lane.st();
    if st.replace_infra(read) {
        st.refresh_pending = true;
    }
    for (key, runs) in &running {
        st.mark_infra_settled(key, runs);
    }
    st.setup_env = up.setup_env;
}

/// The error of a check that ran out of time.
fn timed_out(bound: Duration) -> RpcError {
    RpcError::from(InfraError::new(
        "docker_failed",
        format!("Docker did not answer within {}s", bound.as_secs()),
    ))
}

impl Inner {
    /// The lane's infra spec with its allocated ports, as a bring-up builds it,
    /// so plain containers are named and probed on their ports.
    pub(crate) async fn restarted_infra_spec(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) -> Result<InfraSpec, RpcError> {
        let spec = self.infra_spec(project, lane, &[]).await?;
        let needs = self.integ.infra.port_needs(&spec).await.unwrap_or_default();
        Ok(self.infra_spec(project, lane, &needs).await.unwrap_or(spec))
    }

    /// One read of the lane's infra: the declared names, their runs, the
    /// namespace, and their runs again, so the ports in between are paired
    /// with the runs they belong to.
    pub(crate) async fn read_infra(&self, spec: &InfraSpec) -> Result<InfraRead, InfraError> {
        let began = Instant::now();
        let declared = self.integ.infra.declared(spec).await?;
        let keys: Vec<String> = declared.iter().cloned().collect();
        let before = self.integ.infra.observe(spec, &keys).await;
        let up = self.integ.infra.namespace(spec).await?;
        let after = self.integ.infra.observe(spec, &keys).await;
        Ok(InfraRead {
            began,
            declared,
            before,
            up,
            after,
        })
    }

    /// Brings the lane's infra up and reads it back: the stored namespace
    /// comes from the read after `up`, so its ports are paired with their
    /// runs. Returns the read and what `up` reported. Only `up` itself holds
    /// the project's shared-infra lock.
    pub(crate) async fn up_infra(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        spec: &InfraSpec,
    ) -> Result<(InfraRead, InfraUp), RpcError> {
        // Docker creates a missing bind-mount source itself (as root on Linux).
        self.ensure_lane_data(project, lane).await?;
        let up = self.infra_up(project, spec).await.map_err(RpcError::from)?;
        let read = self.read_infra(spec).await.map_err(RpcError::from)?;
        Ok((read, up))
    }

    /// Checks the lane's keys in doubt ([`crate::model::LaneInfra::to_verify`]
    /// plus `extra`) and stores what changed: one engine observation, and a
    /// namespace read only for keys whose runs changed, or on a lane that
    /// stores nothing yet when something of it runs. Makes no Docker call when
    /// nothing is in doubt. Bounded by `bound`; on expiry or a failure before
    /// the observation, the keys asked about become unknown. Refreshes routes,
    /// drift and the lane's summary when the namespace changed. Returns whether
    /// it did.
    pub(crate) async fn refresh_infra(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        extra: &[String],
        bound: Duration,
    ) -> Result<bool, RefreshFailed> {
        self.refresh_infra_with(project, lane, None, extra, bound)
            .await
    }

    /// [`Self::refresh_infra`] with the spec when the caller has it.
    async fn refresh_infra_with(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        spec: Option<&InfraSpec>,
        extra: &[String],
        bound: Duration,
    ) -> Result<bool, RefreshFailed> {
        let began = Instant::now();
        let (tracked, mut keys) = {
            let st = lane.st();
            (st.infra().tracked(), st.infra().to_verify(extra))
        };
        if tracked && keys.is_empty() {
            return Ok(false);
        }
        let work = self.refresh_work(project, lane, spec, tracked, &mut keys, began);
        let result = match tokio::time::timeout(bound, work).await {
            Ok(result) => result,
            Err(_) => Err(RefreshFailed::Read(timed_out(bound))),
        };
        // Left in `keys`: asked about and not answered.
        lane.st().infra_unanswered(&keys, began);
        let changed = result?;
        if changed {
            let engine = self.clone();
            let project = project.clone();
            let lane = lane.clone();
            tokio::spawn(async move { engine.infra_namespace_changed(&project, &lane).await });
        }
        Ok(changed)
    }

    /// The Docker work of [`Self::refresh_infra_with`]. Clears `keys` once
    /// their observation is stored.
    async fn refresh_work(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        spec: Option<&InfraSpec>,
        tracked: bool,
        keys: &mut BTreeSet<String>,
        began: Instant,
    ) -> Result<bool, RefreshFailed> {
        let built;
        let spec = match spec {
            Some(spec) => spec,
            None => {
                built = self
                    .restarted_infra_spec(project, lane)
                    .await
                    .map_err(RefreshFailed::Names)?;
                &built
            }
        };
        let declared = if tracked {
            None
        } else {
            // A lane that stores nothing checks every declared name.
            let declared = self
                .integ
                .infra
                .declared(spec)
                .await
                .map_err(|error| RefreshFailed::Names(RpcError::from(error)))?;
            keys.extend(declared.iter().cloned());
            Some(declared)
        };
        let asked: Vec<String> = keys.iter().cloned().collect();
        let observed = self.integ.infra.observe(spec, &asked).await;
        let restarted = lane.st().apply_infra_observed(&observed, began);
        keys.clear();
        let read = |error: InfraError| RefreshFailed::Read(RpcError::from(error));
        match declared {
            None if restarted.is_empty() => Ok(false),
            None => {
                let infra = self.read_infra(spec).await.map_err(read)?;
                Ok(lane.st().merge_infra(&infra, &restarted))
            }
            Some(declared) => {
                let runs = observed
                    .values()
                    .any(|observation| matches!(observation, Observation::Running(_)));
                if !runs {
                    lane.st().note_infra_declared(declared);
                    return Ok(false);
                }
                let infra = self.read_infra(spec).await.map_err(read)?;
                Ok(lane.st().replace_infra(infra))
            }
        }
    }

    /// The lane's live infra for a command: the namespace of its running
    /// containers and each declared name's state. Makes no engine call when
    /// every declared container of a lane that stores its infra runs and
    /// nothing is suspect; otherwise checks the suspect, down and unknown keys
    /// (every declared key on a lane that stores nothing) through the Engine
    /// API on every call, and reads ports only for keys whose run changed. A
    /// stopped engine reads as every name down. Bounded at
    /// [`COMMAND_INFRA_WAIT`]. Never waits for health. Stores what it reads.
    /// Fails only when the lane's names are unknowable: its infra spec cannot
    /// be built or the declared names cannot be read.
    pub(crate) async fn current_infra(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) -> Result<CurrentInfra, RpcError> {
        let spec = self.infra_spec(project, lane, &[]).await?;
        if spec.is_empty() {
            return Ok(CurrentInfra {
                namespace: Value::Null,
                states: BTreeMap::new(),
            });
        }
        match self
            .refresh_infra(project, lane, &[], COMMAND_INFRA_WAIT)
            .await
        {
            Ok(_) => {}
            Err(RefreshFailed::Names(error)) => return Err(error),
            Err(RefreshFailed::Read(error)) => {
                tracing::debug!(code = %error.code, "lane infra not checked");
            }
        }
        Ok(lane.st().infra().current())
    }

    /// Checks the keys in doubt and the setup-backed keys `setup_keys` of a
    /// running lane ([`Self::refresh_infra`]), then waits until the running
    /// containers whose runs no bring-up or settle confirmed are healthy, at
    /// most until `deadline` (and two minutes). Containers that do not run are
    /// not waited for and stay unconfirmed. Fails on a failed read, since a
    /// start must not plan with a port it knows is stale, and as a bring-up's
    /// health wait does when one turns unhealthy or is not healthy in time.
    /// `spec` is built when needed and not given.
    pub(crate) async fn settle_infra(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        spec: Option<&InfraSpec>,
        setup_keys: &[String],
        deadline: Instant,
    ) -> Result<Settled, RpcError> {
        if lane.st().infra().settled(setup_keys) {
            return Ok(Settled::Nothing);
        }
        let bound = deadline.saturating_duration_since(Instant::now());
        let changed = self
            .refresh_infra_with(project, lane, spec, setup_keys, bound)
            .await?;
        let (waits, containers): (Vec<_>, Vec<InfraContainer>) = {
            let st = lane.st();
            let infra = st.infra();
            let waits: Vec<_> = infra
                .unsettled()
                .into_iter()
                .filter_map(|key| Some((infra.runs(&key)?.to_vec(), key)))
                .collect();
            let containers = infra
                .containers()
                .iter()
                .filter(|container| {
                    waits.iter().any(|(runs, key)| {
                        &container.name == key && runs.iter().any(|r| r.docker == container.docker)
                    })
                })
                .cloned()
                .collect();
            (waits, containers)
        };
        if waits.is_empty() {
            return Ok(if changed {
                Settled::Done { changed }
            } else {
                Settled::Nothing
            });
        }
        let skipped = if containers.is_empty() {
            Vec::new()
        } else {
            let built;
            let spec = match spec {
                Some(spec) => spec,
                None => {
                    built = self.restarted_infra_spec(project, lane).await?;
                    &built
                }
            };
            let timeout = HEALTH_TIMEOUT.min(deadline.saturating_duration_since(Instant::now()));
            self.integ
                .infra
                .wait_healthy(spec, &containers, timeout)
                .await
                .map_err(RpcError::from)?
        };
        let mut st = lane.st();
        for (runs, key) in &waits {
            if !runs.iter().any(|run| skipped.contains(&run.docker)) {
                st.mark_infra_settled(key, runs);
            }
        }
        Ok(Settled::Done { changed })
    }

    /// Schedules a check of the lane's keys in doubt, unless one is scheduled.
    /// It does not wait for health: a start or restart does. Dropped when the
    /// lane or its project goes away.
    pub(crate) fn queue_infra_refresh(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        if !lane.st().queue_infra_refresh() {
            return;
        }
        let engine = self.clone();
        let project = project.clone();
        let lane = lane.clone();
        tokio::spawn(async move {
            let gone = async {
                tokio::select! {
                    () = engine.shutdown.cancelled() => {}
                    () = project.watch.cancelled() => {}
                    () = lane.watch.cancelled() => {}
                }
            };
            tokio::pin!(gone);
            tokio::select! {
                () = &mut gone => return,
                () = tokio::time::sleep(REFRESH_DELAY) => {}
            }
            lane.st().infra_refresh_taken();
            let refreshed = tokio::select! {
                () = &mut gone => return,
                refreshed = engine.refresh_infra(&project, &lane, &[], REFRESH_BOUND) => refreshed,
            };
            match refreshed {
                Ok(_) => engine.emit_lane(&project, &lane),
                Err(failed) => {
                    let error = RpcError::from(failed);
                    tracing::debug!(code = %error.code, "lane infra not checked again");
                }
            }
        });
    }

    /// Events may have been lost: every lane that stores its infra checks all
    /// of it again.
    pub(crate) fn resync_infra(self: &Arc<Self>) {
        let now = Instant::now();
        for (project, lane) in self.all_lanes() {
            if lane.st().suspect_all_infra(now) {
                self.queue_infra_refresh(&project, &lane);
            }
        }
    }

    /// [`Self::resync_infra`] for the lanes of `project`, after its shared
    /// containers were stopped, so no lane keeps them as running when their
    /// events are lost.
    pub(crate) fn resync_project_infra(self: &Arc<Self>, project: &Arc<ProjectRt>) {
        let now = Instant::now();
        for lane in project.lanes() {
            if lane.st().suspect_all_infra(now) {
                self.queue_infra_refresh(project, &lane);
            }
        }
    }

    /// After a namespace change outside a start: routes follow the new ports,
    /// and the running units that read them show drift.
    pub(crate) async fn infra_namespace_changed(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        self.refresh_views(project).await;
        let running = lane.st().any_running();
        if running {
            self.update_drift(project, lane).await;
        }
        self.emit_lane(project, lane);
    }
}
