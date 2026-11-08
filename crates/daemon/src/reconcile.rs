//! Startup reconciliation (plan 3.7, 7.6). Runs before the daemon accepts
//! clients. Unfinished `unit_runs` whose recorded identity still verifies belong
//! to a crashed or restarted daemon: they keep running as `orphaned` units
//! (decision 16 overrides plan 4.3, see `orphans`); the rest are marked ended.
//! Containers are only re-adopted by label, never stopped, in the background
//! after readiness.

use std::sync::Arc;

use devlanes_proto::RpcError;
use serde_json::json;

use crate::{
    engine::{Inner, once},
    infra_restarts::REFRESH_BOUND,
    integrations::InfraWatch,
    registry::store_error,
};

impl Inner {
    /// Applies a container event to every lane that stores the container: its
    /// `ContainerSummary.state`, and a check of the lane's infra after a
    /// `start`, since the container may publish other ports now. Returns
    /// whether some lane stores the container.
    pub(crate) fn apply_container_event(
        self: &Arc<Self>,
        event: &crate::integrations::InfraEvent,
    ) -> bool {
        let now = tokio::time::Instant::now();
        let mut matched = false;
        for (project, lane) in self.all_lanes() {
            let effect = lane.st().apply_infra_event(event, now);
            matched |= effect.matched;
            if effect.suspect {
                self.queue_infra_refresh(&project, &lane);
            }
            if effect.changed {
                self.emit_lane(&project, &lane);
            }
        }
        matched
    }

    /// Reloads known projects, records surviving units as orphans, starts
    /// background loops and reconnects the boxes an earlier daemon had
    /// connected. Never terminates anything.
    pub(crate) async fn reconcile(self: &Arc<Self>) -> Result<(), RpcError> {
        if !once(&self.background) {
            return Ok(());
        }
        let boot = self.end_stale_runs().await;
        let mut runs = self.store.active_unit_runs().await.map_err(store_error)?;
        // Should ending them have failed, rows of another boot are still never probed.
        if let Some(boot) = &boot {
            runs.retain(|run| run.boot_id.as_ref().is_none_or(|id| id == boot));
        }
        self.load_known_projects().await?;
        self.track_orphans(runs).await;
        let names: Vec<String> = self.all_projects().iter().map(|p| p.name()).collect();
        // The first Docker call waits for the engine to wake (Docker Desktop's
        // Resource Saver took 18 s once), which would outlast the spawning
        // client's start timeout. Adoption re-attaches event watchers and
        // learns which lane containers run. Box lanes may hold those ports
        // until then, as they would have without a restart.
        let infra = self.integ.infra.clone();
        let engine = Arc::downgrade(self);
        let lanes: Vec<(String, u32)> = self
            .all_lanes()
            .iter()
            .filter_map(|(project, lane)| Some((project.name(), lane.st().index()?)))
            .collect();
        tokio::spawn(async move {
            match infra.adopt(&names).await {
                Ok(adopted) => tracing::info!(%adopted, "re-adopted containers"),
                Err(error) => {
                    tracing::debug!(code = %error.code, "container adoption skipped");
                    return;
                }
            }
            // A previous daemon may have crashed while a lane was frozen and its
            // own containers paused; this daemon starts with every lane awake.
            for (project, lane) in lanes {
                match infra.unpause_lane(&project, lane).await {
                    Ok(0) => {}
                    Ok(unpaused) => {
                        tracing::info!(%project, lane, unpaused, "unpaused containers of a frozen lane");
                    }
                    Err(error) => {
                        tracing::debug!(code = %error.code, "unpausing lane containers failed")
                    }
                }
            }
            if let Some(engine) = engine.upgrade() {
                engine.learn_lane_containers().await;
            }
        });
        self.start_dns().await;
        self.start_background();
        self.restore_remotes();
        Ok(())
    }

    /// Reads the infra of each lane with a record, which a new daemon doesn't
    /// know until a start or `infra up`: its running containers with their
    /// ports, so `status` and the routes show them, and a lane whose own
    /// containers hold ports keeps them from box lanes, also after a reboot
    /// brought the containers back by themselves.
    async fn learn_lane_containers(self: &Arc<Self>) {
        for (project, lane) in self.all_lanes() {
            if lane.st().index().is_none() {
                continue;
            }
            match self.infra_spec(&project, &lane, &[]).await {
                Ok(spec) if !spec.is_empty() => {}
                _ => continue,
            }
            if let Err(failed) = self
                .refresh_infra(&project, &lane, &[], REFRESH_BOUND)
                .await
            {
                let error = RpcError::from(failed);
                tracing::debug!(code = %error.code, "lane infra unknown after a restart");
            }
        }
    }

    /// Ends records no process can belong to any more: task runs of the previous
    /// daemon, and unit runs recorded under another boot, whose PIDs now name
    /// unrelated processes and so are never probed. Returns the current boot ID,
    /// `None` when the platform cannot tell.
    async fn end_stale_runs(&self) -> Option<String> {
        // Nothing can wait for these runs any more, so they ended as stopped ones do.
        let stopped = crate::units::UnitState::Stopped(devlanes_core::ExitOutcome {
            code: None,
            signal: None,
        });
        match self
            .store
            .abandon_task_runs(stopped.word().to_owned())
            .await
        {
            Ok(0) => {}
            Ok(abandoned) => tracing::info!(abandoned, "ended task runs of a previous daemon"),
            Err(error) => tracing::warn!(%error, "unfinished task runs not ended"),
        }
        let boot = tokio::task::spawn_blocking(devlanes_sys::boot_id)
            .await
            .ok()
            .and_then(|boot| {
                boot.map_err(|error| tracing::warn!(%error, "boot id unavailable"))
                    .ok()
            });
        if let Some(boot) = &boot {
            match self.store.finish_other_boot_runs(boot.clone()).await {
                Ok(0) => {}
                Ok(ended) => tracing::info!(ended, "ended unit runs of an earlier boot"),
                Err(error) => tracing::warn!(%error, "unit runs of an earlier boot not ended"),
            }
        }
        boot
    }

    /// Rebuilds projects and lanes from SQLite so `status` shows them before any
    /// client cwd is seen. Lanes without a client env show `needs_env` when wanted.
    async fn load_known_projects(self: &Arc<Self>) -> Result<(), RpcError> {
        let projects = self.store.projects().await.map_err(store_error)?;
        let configs = self.store.project_configs().await.map_err(store_error)?;
        for record in projects {
            let Some(config) = configs.iter().find(|config| config.project == record.id) else {
                continue;
            };
            let project = self
                .register_project(
                    record.id,
                    record.name.clone(),
                    record.root_path.clone(),
                    record.common_dir.clone(),
                    record.primary_branch.clone(),
                    (config.dir.clone(), config.file.clone()),
                )
                .await?;
            for lane in project.lanes() {
                if lane.st().present {
                    self.reload_config(&project, &lane).await;
                }
            }
            self.refresh_views(&project).await;
        }
        Ok(())
    }

    /// Re-lists every project, then starts watchers, the sampler and retention.
    fn start_background(self: &Arc<Self>) {
        let engine = self.clone();
        tokio::spawn(async move {
            for project in engine.all_projects() {
                let (root, common_dir, primary) = {
                    let st = project.st();
                    (
                        st.root.clone(),
                        st.common_dir.clone(),
                        st.primary_branch.clone(),
                    )
                };
                let repository = match common_dir {
                    Some(common_dir) => match devlanes_git::list(&root, &common_dir).await {
                        Ok(worktrees) => Some(devlanes_git::Repository {
                            common_dir,
                            root,
                            primary_branch: primary,
                            worktrees,
                        }),
                        Err(error) => {
                            tracing::debug!(%error, "project re-list failed");
                            None
                        }
                    },
                    None => None,
                };
                if repository.is_some() {
                    let _ = engine.relist(&project).await;
                }
                engine.watch_project(&project, repository).await;
            }
        });
        tokio::spawn(self.clone().sampler_loop());
        tokio::spawn(self.clone().state_maintenance_loop());
        tokio::spawn(self.clone().log_maintenance_loop());
        tokio::spawn(self.clone().orphan_watch_loop());
        tokio::spawn(self.clone().update_check_loop());
        tokio::spawn(self.clone().idle_loop());
        if let Some(mut events) = self.integ.infra.events() {
            let engine = self.clone();
            tokio::spawn(async move {
                loop {
                    let event = tokio::select! {
                        () = engine.shutdown.cancelled() => return,
                        event = events.recv() => event,
                    };
                    match event {
                        // The stream carries every container on the engine;
                        // clients hear only of the ones a lane uses.
                        Ok(InfraWatch::Container(event)) => {
                            if engine.apply_container_event(&event) {
                                engine.bus.emit("infra.changed", json!(event));
                            }
                        }
                        Ok(InfraWatch::Resync) => engine.resync_infra(),
                        // Events of other containers can push out a lane's
                        // own, so the lanes check their containers again.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            tracing::debug!(missed, "container events lost; checking lane infra");
                            engine.resync_infra();
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            });
        }
    }
}
