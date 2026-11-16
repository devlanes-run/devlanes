//! Core adapter: the Gradle task runner.

use crate::{TasksRequest, handles, plan};
use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{self, ScriptPlan, ScriptRunner},
};
use std::path::Path;

/// `run_script` runner for Gradle tasks in a build or settings file.
#[derive(Debug, Default, Clone, Copy)]
pub struct GradleTasks;

#[async_trait]
impl ScriptRunner for GradleTasks {
    fn id(&self) -> &'static str {
        "gradle"
    }

    /// Matches `build.gradle`, `build.gradle.kts`, `settings.gradle` and
    /// `settings.gradle.kts`.
    fn matches(&self, source: &Path) -> bool {
        handles(source)
    }

    /// Runs [`plan`] on a blocking thread because it looks for the wrapper.
    /// The plan has no shell body to inspect.
    async fn resolve(&self, request: &contrib::ScriptRequest<'_>) -> Result<ScriptPlan, Error> {
        let worktree = request.worktree.to_path_buf();
        let source = request.source.to_path_buf();
        let name = request.name.to_owned();
        let extra_args = request.extra_args.to_vec();
        let planned = tokio::task::spawn_blocking(move || {
            plan(&TasksRequest {
                worktree: &worktree,
                source: &source,
                name: &name,
                extra_args: &extra_args,
            })
        })
        .await
        .map_err(|e| Error::Integration(format!("Gradle task resolution failed: {e}")))?
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
        Ok(ScriptPlan {
            detail: Some(planned.detail()),
            argv: planned.argv,
            cwd: planned.cwd,
            body: None,
            notes: planned.notes,
            env: Default::default(),
            path: Vec::new(),
        })
    }
}
