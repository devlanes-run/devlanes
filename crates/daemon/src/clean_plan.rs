//! `lane.clean.plan` (`lane clean --dry-run`): what a clean of a lane would
//! stop, run and delete, read without stopping, running or deleting anything:
//! with a lane record also its run and task history (`history`), and the
//! project's shared containers when it is the last lane (`shared_infra`).

use std::{collections::BTreeSet, path::Path, sync::Arc};

use devlanes_config::{InfraCommand, ProjectConfig, Run, TaskWhen};
use devlanes_proto::{Context, Method, RpcError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    engine::{Inner, params},
    target::Policy,
};

/// `lane.clean.plan` parameters, the same as `lane.clean`'s.
#[derive(Debug, Default, Deserialize)]
struct PlanParams {
    #[serde(default)]
    lane: Option<String>,
}

impl Inner {
    /// `lane.clean.plan`: the [`devlanes_proto::Method::LaneCleanPlan`] reply
    /// for the lane `lane.clean` would clean. Needs no `yes`, and a worktree
    /// that is no lane yet is not given one. Sizes the engine or the file
    /// system cannot tell are `null` rather than an error.
    pub(crate) async fn lane_clean_plan_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: PlanParams = params(value)?;
        let context = &Context {
            lane: args.lane.or_else(|| context.lane.clone()),
            ..context.clone()
        };
        // As `lane clean` resolves it, under the same policy.
        let (project, lane) = self
            .lane(context, Policy::of(Method::LaneCleanPlan))
            .await?;
        let name = project.name();
        let (mut plan, record, index, runnable, config) = {
            let st = lane.st();
            let reference = st.reference(&name);
            let mut running: Vec<Value> = st
                .units()
                .iter()
                .filter(|(_, rec)| rec.host.is_alive())
                .map(|(unit, rec)| {
                    json!({"unit": format!("{reference}:{unit}"), "actor": rec.host.info().actor})
                })
                .collect();
            running.extend(st.orphans().iter().map(|(unit, orphan)| {
                json!({"unit": format!("{reference}:{unit}"), "actor": orphan.actor})
            }));
            // `clean_lane` runs no hooks for these, as `hooks_runnable` says.
            let hooks_skipped = if !st.present() {
                Some("worktree_missing")
            } else if st.record().is_none() {
                Some("unassigned")
            } else if st.env().is_none() {
                Some("needs_env")
            } else {
                None
            };
            let plan = json!({
                "lane": reference,
                "project": name,
                "index": st.index(),
                "branch": st.branch(),
                "path": st.path(),
                "worktree": if st.present() { "present" } else { "missing" },
                "running": running,
                "tasks": [],
                "teardown": [],
                "hooks_skipped": hooks_skipped,
                "containers": [],
                "compose_project": null,
                "volumes": [],
                "infra_error": null,
                "data_dir": null,
                "logs_bytes": null,
                "ports": st.view().map(|view| json!({"start": view.block.0, "size": view.block.1})),
            });
            (
                plan,
                st.record().cloned(),
                st.index(),
                hooks_skipped.is_none(),
                st.config().loaded().cloned(),
            )
        };
        // As in `clean_lane`: tasks and teardown come from the lane's current
        // plan, and a plan that fails runs neither. Only they are planned, and
        // dry, so no contributor such as a secret store is asked and the data
        // folder is not created.
        let only = config.as_deref().map(clean_units);
        if runnable && only.as_ref().is_none_or(|only| !only.is_empty()) {
            let resolved = self
                .resolve_dry(&project, &lane, config.clone(), only.as_ref())
                .await;
            match resolved {
                Ok(resolved) => {
                    let tasks: Vec<Value> = resolved
                        .tasks
                        .iter()
                        .filter(|(_, task)| task.when == Some(TaskWhen::Clean) && task.active)
                        .map(|(task, _)| {
                            json!({"name": task, "run": task_run(config.as_deref(), task)})
                        })
                        .collect();
                    let teardown: Vec<Value> = resolved
                        .teardown
                        .iter()
                        .map(|(entry, command)| {
                            json!({
                                "name": entry,
                                "run": teardown_run(config.as_deref(), entry),
                                "container": command.container,
                            })
                        })
                        .collect();
                    plan["tasks"] = json!(tasks);
                    plan["teardown"] = json!(teardown);
                }
                Err(error) => plan["hooks_skipped"] = json!(error.code),
            }
        }
        if let Some(index) = index {
            match self.integ.infra.lane_resources(&name, index).await {
                Ok(resources) => {
                    for key in ["containers", "compose_project", "volumes"] {
                        if let Some(found) = resources.get(key) {
                            plan[key] = found.clone();
                        }
                    }
                }
                // Without Docker a project that declares no containers has none to
                // remove; only one that declares them is left not knowing.
                Err(error) if declares_containers(config.as_deref()) => {
                    plan["infra_error"] = json!(error.message);
                }
                Err(_) => {}
            }
        }
        if let Some(record) = record {
            let data = self.lane_data_path(&name, record.id);
            let logs =
                index.map(|index| crate::lifecycle::lane_log_dir(&self.opts.logs, &name, index));
            let (data_bytes, logs_bytes) = tokio::task::spawn_blocking({
                let data = data.clone();
                move || (tree_size(&data), logs.as_deref().and_then(tree_size))
            })
            .await
            .unwrap_or((None, None));
            plan["data_dir"] = json!({"path": data, "bytes": data_bytes});
            plan["logs_bytes"] = json!(logs_bytes);
            plan["history"] = json!(true);
        }
        if let Some(shared) = self.shared_on_clean(&project, &lane).await.plan() {
            plan["shared_infra"] = shared;
        }
        Ok(plan)
    }
}

/// The units a clean runs, as [`devlanes_resolve::ResolveInput::only`] names
/// them: the `lane.clean` tasks with what their steps run, and every teardown
/// entry.
fn clean_units(config: &ProjectConfig) -> BTreeSet<String> {
    let mut units: BTreeSet<String> = config
        .infra
        .teardown
        .keys()
        .map(|entry| format!("infra.teardown.{entry}"))
        .collect();
    for (name, task) in &config.tasks {
        if task.when == Some(TaskWhen::Clean)
            && let Some(task_units) = crate::resolve::task_units(config, name)
        {
            units.extend(task_units);
        }
    }
    units
}

/// Whether the lane's config runs containers (Compose files or
/// `infra.containers`); `true` when it did not load, since it may.
fn declares_containers(config: Option<&ProjectConfig>) -> bool {
    config
        .is_none_or(|config| config.infra.compose.is_some() || !config.infra.containers.is_empty())
}

/// The command of task `name` as the config writes it, expressions left as
/// they are: the `run` text, `<name> in <file>` for `run_script`, or `steps a, b`.
fn task_run(config: Option<&ProjectConfig>, name: &str) -> Option<String> {
    let task = config?.tasks.get(name)?;
    if let Some(run) = &task.run {
        return run_text(run);
    }
    if let Some(script) = &task.run_script {
        return Some(format!("{} in {}", script.name, script.file));
    }
    task.steps
        .as_ref()
        .map(|steps| format!("steps {}", steps.join(", ")))
}

/// The command of `infra.teardown.<name>` as the config writes it.
fn teardown_run(config: Option<&ProjectConfig>, name: &str) -> Option<String> {
    match config?.infra.teardown.get(name)? {
        InfraCommand::Object(options) => options.run.as_ref().and_then(run_text),
        InfraCommand::Command(run) => run_text(run),
    }
}

/// Shell text as written, or the program and its arguments joined by spaces.
fn run_text(run: &Run) -> Option<String> {
    match serde_json::to_value(run).ok()? {
        Value::String(text) => Some(text),
        Value::Array(words) => Some(
            words
                .iter()
                .map(|word| {
                    word.as_str()
                        .map_or_else(|| word.to_string(), str::to_owned)
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// Bytes in the files below `path`, a symbolic link counted as itself, not
/// what it points to: `Some(0)` when `path` does not exist and `None` when
/// part of it cannot be read. Blocks.
fn tree_size(path: &Path) -> Option<u64> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(0),
        Err(_) => return None,
    };
    if !metadata.is_dir() {
        return Some(metadata.len());
    }
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            // `DirEntry::metadata` does not follow links, so a link to a
            // folder is not walked.
            let metadata = entry.metadata().ok()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Some(total)
}
