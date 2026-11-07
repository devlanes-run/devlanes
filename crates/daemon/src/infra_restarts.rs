//! Infra containers that restarted while their lane runs. Docker can give a
//! restarted container new ephemeral host ports, so the lane's stored `infra`
//! namespace is read again for those containers: in the background after their
//! events, and before a start or restart plans units, which also waits until
//! they are healthy. Nothing restarts a unit by itself; services that read the
//! old port show drift.

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
    integrations::{InfraContainer, InfraSpec},
    model::{LaneRt, ProjectRt},
};

/// Gathers the `die`, `start` and `restart` events of one restart (and of
/// several containers restarting together) into one refresh.
const REFRESH_DELAY: Duration = Duration::from_millis(500);
/// The longest wait for restarted containers to become healthy, as a bring-up's
/// health wait.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

/// Whether a namespace entry names a published port. A container that is down
/// when Compose is asked has none, and keeps the one the lane stored.
fn has_port(entry: &Value) -> bool {
    entry.get("port").is_some_and(|port| !port.is_null())
        || entry
            .get("ports")
            .and_then(Value::as_object)
            .is_some_and(|ports| !ports.is_empty())
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

    /// Reads the namespace entries of the containers `names` again and merges
    /// them into the lane's. An entry without a port (its container is down)
    /// keeps the stored one, and a container the read does not list keeps its
    /// stored entry with state `missing`, so its next `start` event counts as a
    /// restart again. Dropped when the namespace was written as a whole
    /// meanwhile. Returns whether the namespace changed.
    pub(crate) async fn refresh_restarted_infra(
        &self,
        lane: &Arc<LaneRt>,
        spec: &InfraSpec,
        names: &BTreeSet<String>,
    ) -> Result<bool, RpcError> {
        let _refresh = lane.infra_refresh.lock().await;
        let generation = lane.st().infra_generation;
        let up = self
            .infra_namespace_kept(spec)
            .await
            .map_err(RpcError::from)?;
        let mut st = lane.st();
        if st.infra_generation != generation {
            return Ok(false);
        }
        let Some(Value::Object(stored)) = st.infra_ns.as_mut() else {
            return Ok(false);
        };
        let mut changed = false;
        if let Value::Object(read) = &up.namespace {
            for name in names {
                if let Some(entry) = read.get(name).filter(|entry| has_port(entry))
                    && stored.get(name) != Some(entry)
                {
                    stored.insert(name.clone(), entry.clone());
                    changed = true;
                }
            }
        }
        for name in names {
            let read: Vec<InfraContainer> = up
                .containers
                .iter()
                .filter(|container| &container.name == name)
                .cloned()
                .collect();
            if read.is_empty() {
                for container in st.infra_containers.iter_mut() {
                    if &container.name == name {
                        container.state = Some("missing".into());
                    }
                }
            } else {
                st.infra_containers
                    .retain(|container| &container.name != name);
                st.infra_containers.extend(read);
            }
        }
        Ok(changed)
    }

    /// Waits until the lane's restarted containers that run are healthy, at most
    /// until `deadline` (and two minutes), then reads their namespace entries
    /// again. Containers that do not run are not waited for and stay marked, so
    /// a later start looks at them again. Fails as a bring-up's health wait
    /// does when one turns unhealthy or is not healthy in time. Returns whether
    /// the namespace changed; when it did, routes and drift follow.
    pub(crate) async fn settle_restarted_infra(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        spec: &InfraSpec,
        deadline: Instant,
    ) -> Result<bool, RpcError> {
        let marked = |lane: &LaneRt| -> (BTreeMap<String, u64>, Vec<InfraContainer>) {
            let st = lane.st();
            let containers = st
                .infra_containers
                .iter()
                .filter(|container| st.infra_restarted.contains_key(&container.name))
                .cloned()
                .collect();
            (st.infra_restarted.clone(), containers)
        };
        let (mut restarted, mut containers) = marked(lane);
        if restarted.is_empty() {
            return Ok(false);
        }
        // A container the lane never stored (down when a restarted daemon
        // learned the lane's containers) has no Docker name to wait for until
        // it is read once.
        let unstored: BTreeSet<String> = restarted
            .keys()
            .filter(|name| !containers.iter().any(|c| &c.name == *name))
            .cloned()
            .collect();
        if !unstored.is_empty() {
            self.refresh_restarted_infra(lane, spec, &unstored).await?;
            (restarted, containers) = marked(lane);
        }
        let timeout = HEALTH_TIMEOUT.min(deadline.saturating_duration_since(Instant::now()));
        let skipped = self
            .integ
            .infra
            .wait_healthy(spec, &containers, timeout)
            .await
            .map_err(RpcError::from)?;
        let down: BTreeSet<&String> = containers
            .iter()
            .filter(|container| skipped.contains(&container.docker))
            .map(|container| &container.name)
            .collect();
        // A container that is down publishes nothing to read.
        let names: BTreeSet<String> = restarted
            .keys()
            .filter(|name| !down.contains(name))
            .cloned()
            .collect();
        let changed = if names.is_empty() {
            false
        } else {
            self.refresh_restarted_infra(lane, spec, &names).await?
        };
        {
            let mut st = lane.st();
            for (name, count) in &restarted {
                // A restart counted during the wait needs another look, and so
                // does a container still not stored, which was not waited for.
                let waited = containers.iter().any(|c| &c.name == name);
                if waited && !down.contains(name) && st.infra_restarted.get(name) == Some(count) {
                    st.infra_restarted.remove(name);
                }
            }
        }
        if changed {
            let engine = self.clone();
            let project = project.clone();
            let lane = lane.clone();
            tokio::spawn(async move { engine.infra_namespace_changed(&project, &lane).await });
        }
        Ok(changed)
    }

    /// Schedules a refresh of the lane's restarted containers, unless one is
    /// scheduled already. It does not wait for health: a start or restart does.
    pub(crate) fn queue_infra_refresh(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
    ) {
        {
            let mut st = lane.st();
            if st.infra_refresh_queued {
                return;
            }
            st.infra_refresh_queued = true;
        }
        let engine = self.clone();
        let project = project.clone();
        let lane = lane.clone();
        tokio::spawn(async move {
            tokio::time::sleep(REFRESH_DELAY).await;
            let names: BTreeSet<String> = {
                let mut st = lane.st();
                st.infra_refresh_queued = false;
                st.infra_restarted.keys().cloned().collect()
            };
            if names.is_empty() {
                return;
            }
            let refreshed = match engine.restarted_infra_spec(&project, &lane).await {
                Ok(spec) => engine.refresh_restarted_infra(&lane, &spec, &names).await,
                Err(error) => Err(error),
            };
            match refreshed {
                Ok(true) => engine.infra_namespace_changed(&project, &lane).await,
                Ok(false) => engine.emit_lane(&project, &lane),
                Err(error) => {
                    tracing::debug!(code = %error.code, "restarted containers not read again");
                }
            }
        });
    }

    /// After a partial namespace change: routes follow the new ports, and the
    /// running units that read them show drift.
    async fn infra_namespace_changed(
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
