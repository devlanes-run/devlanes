//! A project's shared containers going up and down. Every `InfraProvider::up`
//! and `InfraProvider::down` goes through here, under the project's
//! [`ProjectRt::shared_infra`] lock, so the clean of a project's last lane can
//! decide that no lane uses the shared containers and stop them as one step
//! against a start of another lane.

use std::sync::Arc;

use devlanes_config::ProjectConfig;
use devlanes_proto::RpcError;
use serde_json::{Value, json};

use crate::{
    engine::Inner,
    integrations::{InfraError, InfraSpec, InfraUp, SharedInfraSpec},
    model::{Closing, LaneRt, ProjectRt},
};

/// What cleaning a lane does to the project's shared containers.
pub(crate) enum SharedOnClean {
    /// Another runtime uses them ([`used_by_another`]).
    Used,
    /// The project file declares no containers.
    None,
    /// No other lane uses them: stop them with this spec.
    Stop(SharedInfraSpec),
    /// Cannot tell what to stop: the primary worktree's file does not load, or
    /// a value of the spec does not evaluate.
    Unknown(RpcError),
}

impl SharedOnClean {
    /// The `shared_infra` field of a `lane.clean.plan` result: `"stop"`, or
    /// `{skipped, message}` when what to stop is unknown; `None` otherwise.
    pub(crate) fn plan(&self) -> Option<Value> {
        match self {
            Self::Used | Self::None => None,
            Self::Stop(_) => Some(json!("stop")),
            Self::Unknown(error) => Some(skipped(error)),
        }
    }
}

/// What [`Inner::stop_shared_infra_unless_used`] did.
pub(crate) enum SharedInfraStop {
    /// Another lane uses the shared containers; nothing was stopped.
    Used,
    /// The project declares no containers.
    None,
    /// Stopped; the provider's report.
    Stopped(Value),
    /// The stop failed.
    Failed(InfraError),
    /// Nothing was stopped: what to stop is unknown ([`SharedOnClean::Unknown`]).
    Unknown(RpcError),
}

impl SharedInfraStop {
    /// The `shared_infra` field of a `lane.clean` result: the provider's report,
    /// `{error: {code, message}}` or `{skipped, message}`; `None` when nothing
    /// was to be stopped.
    pub(crate) fn report(&self) -> Option<Value> {
        match self {
            Self::Used | Self::None => None,
            Self::Stopped(report) => Some(report.clone()),
            Self::Failed(error) => Some(json!({
                "error": {"code": error.code, "message": error.message},
            })),
            Self::Unknown(error) => Some(skipped(error)),
        }
    }
}

/// `{skipped: <code>, message}` for a stop that was not tried.
fn skipped(error: &RpcError) -> Value {
    json!({"skipped": error.code, "message": error.data.get("message")})
}

/// Whether `config` runs containers (Compose files or `infra.containers`);
/// `true` when it did not load, since it may.
pub(crate) fn declares_containers(config: Option<&ProjectConfig>) -> bool {
    config
        .is_none_or(|config| config.infra.compose.is_some() || !config.infra.containers.is_empty())
}

/// Whether a runtime of `project` other than `leaving` uses the shared
/// containers: it runs something, is busy or starting, is held by another
/// clean that has not finished its stop, `lane.clean` tasks and teardown
/// ([`Closing::Hooks`]), or is a lane (`listed()`) no clean holds. A runtime in
/// [`Closing::Released`] is no user.
fn used_by_another(project: &ProjectRt, leaving: &Arc<LaneRt>) -> bool {
    project.lanes().iter().any(|lane| {
        if Arc::ptr_eq(lane, leaving) {
            return false;
        }
        let st = lane.st();
        match st.closing {
            Closing::Released => false,
            Closing::Hooks => true,
            Closing::No => {
                st.listed()
                    || st.any_running()
                    || st.busy.is_some()
                    || st.starting > 0
                    || !st.starts.is_empty()
            }
        }
    })
}

impl Inner {
    /// What cleaning `leaving` does to `project`'s shared containers: the one
    /// rule for `lane clean` and its dry run, read from lane facts, never from
    /// Docker.
    pub(crate) async fn shared_on_clean(
        &self,
        project: &ProjectRt,
        leaving: &Arc<LaneRt>,
    ) -> SharedOnClean {
        if used_by_another(project, leaving) {
            return SharedOnClean::Used;
        }
        if self
            .primary_config(project)
            .is_some_and(|config| !declares_containers(Some(&config)))
        {
            return SharedOnClean::None;
        }
        let env = leaving.st().env.clone();
        match self.shared_infra_spec(project, env.as_deref()).await {
            Ok(spec) => SharedOnClean::Stop(spec),
            Err(error) => SharedOnClean::Unknown(error),
        }
    }

    /// `InfraProvider::up` holding `project.shared_infra` shared, so it never
    /// runs between a last clean's decision and its `down`. The only caller of
    /// `up`.
    pub(crate) async fn infra_up(
        &self,
        project: &ProjectRt,
        spec: &InfraSpec,
    ) -> Result<InfraUp, InfraError> {
        let _shared = project.shared_infra.read().await;
        self.integ.infra.up(spec).await
    }

    /// `InfraProvider::down` holding `project.shared_infra` exclusively (`infra
    /// down`). With [`Self::stop_shared_infra_unless_used`], the only callers
    /// of `down`.
    pub(crate) async fn infra_down(
        &self,
        project: &ProjectRt,
        spec: &SharedInfraSpec,
    ) -> Result<Value, InfraError> {
        let _exclusive = project.shared_infra.write().await;
        self.integ.infra.down(spec).await
    }

    /// Stops `project`'s shared containers, keeping their data, unless a lane
    /// other than `leaving` uses them: takes `project.shared_infra`
    /// exclusively, evaluates [`Self::shared_on_clean`] under it and, for
    /// `Stop`, calls `down` before releasing it. Called by a clean once
    /// `leaving` is [`Closing::Released`].
    ///
    /// When another lane already uses them before the lock, returns `Used`
    /// without taking it, so a clean that leaves other lanes does not wait for
    /// their `up`. That is safe because the caller trusts `Used`: it keeps the
    /// project, so the shared block stays reserved whether or not those users
    /// are still there by the time it decides. Users can vanish without a clean
    /// (a waiting lane whose folder is removed stops being `listed()`).
    ///
    /// Two concurrent cleans of a project's last two lanes: the first to get
    /// here sees the other in [`Closing::Hooks`] and answers `Used`; the other
    /// gets here once the first is `Released` (or retired) and stops the
    /// containers, so no `lane.clean` task or teardown runs against stopped
    /// containers. If both get here released, `down` runs twice.
    ///
    /// Ordering against starts: a start claims its lane before its infra
    /// phase, so a start whose `up` ran (or runs, holding the lock shared)
    /// before this takes the lock shows as `Used` here and nothing is stopped;
    /// a start that reaches `up` after this took the lock waits, and its `up`
    /// runs after the `down` and brings the containers back.
    pub(crate) async fn stop_shared_infra_unless_used(
        &self,
        project: &ProjectRt,
        leaving: &Arc<LaneRt>,
    ) -> SharedInfraStop {
        if used_by_another(project, leaving) {
            return SharedInfraStop::Used;
        }
        let _exclusive = project.shared_infra.write().await;
        match self.shared_on_clean(project, leaving).await {
            SharedOnClean::Used => SharedInfraStop::Used,
            SharedOnClean::None => SharedInfraStop::None,
            SharedOnClean::Unknown(error) => SharedInfraStop::Unknown(error),
            SharedOnClean::Stop(spec) => match self.integ.infra.down(&spec).await {
                Ok(report) => SharedInfraStop::Stopped(report),
                Err(error) => SharedInfraStop::Failed(error),
            },
        }
    }
}
