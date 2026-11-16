//! Core adapter: the Taskfile script runner.

use crate::{TaskError, TaskRequest, handles, plan};
use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{ScriptPlan, ScriptRequest, ScriptRunner},
};
use std::path::Path;

/// `run_script` runner for Taskfiles (`Taskfile.yml` and the other names task
/// finds by itself).
#[derive(Debug, Default, Clone, Copy)]
pub struct TaskfileTasks;

#[async_trait]
impl ScriptRunner for TaskfileTasks {
    fn id(&self) -> &'static str {
        "task"
    }

    /// Matches the Taskfile names task looks for.
    fn matches(&self, source: &Path) -> bool {
        handles(source)
    }

    /// Runs [`plan`] on a blocking thread because it reads the Taskfile. The
    /// plan has no body; task commands are not inspected by presets.
    async fn resolve(&self, request: &ScriptRequest<'_>) -> Result<ScriptPlan, Error> {
        let worktree = request.worktree.to_path_buf();
        let source = request.source.to_path_buf();
        let name = request.name.to_owned();
        let extra_args = request.extra_args.to_vec();
        let planned = tokio::task::spawn_blocking(move || {
            plan(&TaskRequest {
                worktree: &worktree,
                source: &source,
                name: &name,
                extra_args: &extra_args,
            })
        })
        .await
        .map_err(|e| Error::Integration(format!("Taskfile task resolution failed: {e}")))?
        .map_err(|error| match error {
            TaskError::Read { .. } => Error::Integration(error.to_string()),
            other => Error::InvalidInput(other.to_string()),
        })?;
        Ok(ScriptPlan {
            argv: planned.argv,
            cwd: planned.cwd,
            body: None,
            notes: planned.notes,
            detail: None,
            env: Default::default(),
            path: Vec::new(),
        })
    }
}
