//! Core adapter: the Go main package script runner.

use crate::{GoError, PackageRequest, handles, plan};
use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{ScriptPlan, ScriptRequest, ScriptRunner},
};
use std::path::Path;

/// `run_script` runner for main packages of a `go.mod` module.
#[derive(Debug, Default, Clone, Copy)]
pub struct GoPackages;

#[async_trait]
impl ScriptRunner for GoPackages {
    fn id(&self) -> &'static str {
        "go"
    }

    /// Matches files named `go.mod`.
    fn matches(&self, source: &Path) -> bool {
        handles(source)
    }

    /// Runs [`plan`] on a blocking thread: it lists the package directory and
    /// reads the start of its Go files. The plan has no body to inspect.
    async fn resolve(&self, request: &ScriptRequest<'_>) -> Result<ScriptPlan, Error> {
        let worktree = request.worktree.to_path_buf();
        let source = request.source.to_path_buf();
        let name = request.name.to_owned();
        let extra_args = request.extra_args.to_vec();
        let planned = tokio::task::spawn_blocking(move || {
            plan(&PackageRequest {
                worktree: &worktree,
                source: &source,
                name: &name,
                extra_args: &extra_args,
            })
        })
        .await
        .map_err(|e| Error::Integration(format!("go package resolution failed: {e}")))?
        .map_err(|error| match error {
            GoError::Read { .. } => Error::Integration(error.to_string()),
            other => Error::InvalidInput(other.to_string()),
        })?;
        Ok(ScriptPlan {
            argv: planned.argv,
            cwd: planned.cwd,
            body: None,
            notes: Vec::new(),
            detail: None,
            env: Default::default(),
            path: Vec::new(),
        })
    }
}
