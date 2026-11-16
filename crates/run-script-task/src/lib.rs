//! `run_script` support for Taskfiles: `task --taskfile <file> <name>` in the
//! Taskfile's directory, for go-task (<https://taskfile.dev>).
//!
//! Extra arguments go after `--`, where task exposes them to the task's
//! templates as `{{.CLI_ARGS}}`; a task that never reads it ignores them.
//!
//! The task check reads the top-level `tasks` mapping, following aliases and
//! `*` wildcards in the order task does. A task is reported missing only when
//! the file has no `includes` that could provide it.
//!
//! [`TaskfileTasks`] adapts [`plan`] to `devlanes_core::contrib::ScriptRunner`.

mod adapter;

pub use adapter::TaskfileTasks;

use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// File names task finds by itself, in its order of priority.
const DEFAULT_NAMES: [&str; 8] = [
    "Taskfile.yml",
    "taskfile.yml",
    "Taskfile.yaml",
    "taskfile.yaml",
    "Taskfile.dist.yml",
    "taskfile.dist.yml",
    "Taskfile.dist.yaml",
    "taskfile.dist.yaml",
];

/// A task as far as running it from the command line is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Key under `tasks`, possibly with `*` wildcards.
    pub name: String,
    /// Alternative names from `aliases`, possibly with `*` wildcards.
    pub aliases: Vec<String>,
    /// `internal: true`: task refuses to run it from the command line.
    pub internal: bool,
    /// Whether any text in the task mentions `CLI_ARGS`.
    pub reads_cli_args: bool,
}

/// Whether a task is defined, as far as the parser can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCheck {
    Found(Task),
    Missing { known: Vec<String> },
    Unverifiable { reason: &'static str },
}

/// Tasks of a Taskfile, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tasks {
    pub tasks: Vec<Task>,
    /// First construct that may define tasks the parser cannot see.
    pub hidden_by: Option<&'static str>,
}

impl Tasks {
    /// Finds the task `task <name>` would run: an exact name, then an exact
    /// alias, then the first name or alias whose wildcards match.
    pub fn check(&self, name: &str) -> TaskCheck {
        let found = self
            .tasks
            .iter()
            .find(|task| task.name == name)
            .or_else(|| {
                self.tasks
                    .iter()
                    .find(|task| task.aliases.iter().any(|alias| alias == name))
            })
            .or_else(|| {
                self.tasks.iter().find(|task| {
                    std::iter::once(&task.name)
                        .chain(&task.aliases)
                        .any(|pattern| pattern.contains('*') && wildcard_match(pattern, name))
                })
            });
        if let Some(task) = found {
            TaskCheck::Found(task.clone())
        } else if let Some(reason) = self.hidden_by {
            TaskCheck::Unverifiable { reason }
        } else {
            TaskCheck::Missing {
                known: self
                    .tasks
                    .iter()
                    .filter(|task| !task.internal)
                    .map(|task| task.name.clone())
                    .collect(),
            }
        }
    }
}

/// A `run_script` reference to a Taskfile task.
#[derive(Debug, Clone, Copy)]
pub struct TaskRequest<'a> {
    /// Root of the lane's worktree; the Taskfile must be inside it.
    pub worktree: &'a Path,
    /// Path to the Taskfile, relative to `worktree` or absolute inside it.
    pub source: &'a Path,
    pub name: &'a str,
    /// Arguments for `{{.CLI_ARGS}}`, typically preset flags.
    pub extra_args: &'a [String],
}

/// What to spawn for a [`TaskRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    /// Directory containing the Taskfile.
    pub cwd: PathBuf,
    /// [`TaskCheck::Found`] or [`TaskCheck::Unverifiable`].
    pub check: TaskCheck,
    /// Non-fatal findings, such as extra arguments the task never reads.
    pub notes: Vec<String>,
}

/// Why a Taskfile could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// Boxed: serde-saphyr's error is large enough to bloat every `Result`.
    #[error(transparent)]
    Yaml(Box<serde_saphyr::Error>),
    #[error("{0}")]
    Shape(&'static str),
}

/// Errors from resolving a Taskfile task.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("invalid task name `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("`{}` is outside the lane worktree `{}`", .file.display(), .worktree.display())]
    OutsideWorktree { file: PathBuf, worktree: PathBuf },
    #[error("cannot read `{}`", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
    #[error("cannot parse `{}`: {error}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        error: ParseError,
    },
    #[error("`{}` has no task `{task}` ({})", .file.display(), known_hint(.known))]
    MissingTask {
        file: PathBuf,
        task: String,
        known: Vec<String>,
    },
    #[error(
        "task `{task}` in `{}` is internal, so task refuses to run it from the command line; run a task that calls it, or remove `internal: true`",
        .file.display()
    )]
    Internal { file: PathBuf, task: String },
}

/// Formats the task list for [`TaskError::MissingTask`].
fn known_hint(known: &[String]) -> String {
    if known.is_empty() {
        "it defines no tasks".to_owned()
    } else {
        format!("tasks: {}", known.join(", "))
    }
}

/// Returns whether `path` names a file this runner handles: one of the names
/// task finds by itself. Other names would also need `--taskfile` in every
/// `task` call a developer types, so they are left to `run`.
pub fn handles(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| DEFAULT_NAMES.contains(&name))
}

/// Resolves a task reference into the command to spawn.
///
/// Reads the Taskfile. Errors when it does not parse, or the task is certainly
/// missing or internal.
pub fn plan(request: &TaskRequest<'_>) -> Result<LaunchPlan, TaskError> {
    validate_name(request.name)?;
    let worktree = normalize(request.worktree);
    let file = normalize(&worktree.join(request.source));
    if !file.starts_with(&worktree) {
        return Err(TaskError::OutsideWorktree { file, worktree });
    }
    let contents = std::fs::read_to_string(&file).map_err(|error| TaskError::Read {
        path: file.clone(),
        error,
    })?;
    let tasks = parse(&contents).map_err(|error| TaskError::Parse {
        path: file.clone(),
        error,
    })?;
    let check = tasks.check(request.name);
    let mut notes = Vec::new();
    match &check {
        TaskCheck::Missing { known } => {
            return Err(TaskError::MissingTask {
                file,
                task: request.name.to_owned(),
                known: known.clone(),
            });
        }
        TaskCheck::Found(task) if task.internal => {
            return Err(TaskError::Internal {
                file,
                task: task.name.clone(),
            });
        }
        TaskCheck::Found(task) if !task.reads_cli_args && !request.extra_args.is_empty() => {
            notes.push(format!(
                "task `{}` never reads {{{{.CLI_ARGS}}}}, so it ignores the arguments after `--` ({}); add {{{{.CLI_ARGS}}}} to the command that should get them",
                task.name,
                request.extra_args.join(" ")
            ));
        }
        TaskCheck::Found(_) | TaskCheck::Unverifiable { .. } => {}
    }
    let cwd = file
        .parent()
        .map_or_else(|| worktree.clone(), Path::to_path_buf);
    let file_name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Always named: with several default files in one directory, task would
    // pick by its own priority rather than the file the config names.
    let mut argv = vec![
        "task".to_owned(),
        "--taskfile".to_owned(),
        file_name,
        request.name.to_owned(),
    ];
    if !request.extra_args.is_empty() {
        argv.push("--".to_owned());
        argv.extend(request.extra_args.iter().cloned());
    }
    Ok(LaunchPlan {
        argv,
        cwd,
        check,
        notes,
    })
}

/// Rejects names task would read as an option or a variable assignment.
fn validate_name(name: &str) -> Result<(), TaskError> {
    let reason = if name.is_empty() {
        "it is empty"
    } else if name.starts_with('-') {
        "it starts with `-` and would be read as an option"
    } else if name.contains('=') {
        "it contains `=` and would be read as a variable assignment"
    } else {
        return Ok(());
    };
    Err(TaskError::InvalidName {
        name: name.to_owned(),
        reason,
    })
}

/// Parses the tasks of a Taskfile.
pub fn parse(contents: &str) -> Result<Tasks, ParseError> {
    // No source snippet: the message ends up on one line of a launch error.
    let options = serde_saphyr::options! { with_snippet: false };
    let document: Value = serde_saphyr::from_str_with_options(contents, options)
        .map_err(|error| ParseError::Yaml(Box::new(error)))?;
    let root = match &document {
        Value::Object(root) => root,
        // An empty file has no tasks.
        Value::Null => return Ok(Tasks::default()),
        _ => return Err(ParseError::Shape("the file is not a mapping")),
    };
    let mut parsed = Tasks::default();
    if root
        .get("includes")
        .is_some_and(|includes| !is_empty(includes))
    {
        parsed.hidden_by = Some("it includes other Taskfiles");
    }
    let tasks = match root.get("tasks") {
        Some(Value::Object(tasks)) => tasks,
        None | Some(Value::Null) => return Ok(parsed),
        Some(_) => return Err(ParseError::Shape("`tasks` is not a mapping")),
    };
    for (name, value) in tasks {
        // A string or a list is the shorthand for a task with only commands.
        let (aliases, internal) = match value {
            Value::Object(task) => (
                task.get("aliases")
                    .and_then(Value::as_array)
                    .map(|aliases| {
                        aliases
                            .iter()
                            .filter_map(|alias| alias.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                task.get("internal").and_then(Value::as_bool) == Some(true),
            ),
            _ => (Vec::new(), false),
        };
        parsed.tasks.push(Task {
            name: name.clone(),
            aliases,
            internal,
            reads_cli_args: mentions_cli_args(value),
        });
    }
    Ok(parsed)
}

/// Whether `value` is null or an empty mapping or list.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

/// Whether any string in the task mentions `CLI_ARGS`. The whole task counts,
/// not only its commands, because a common pattern copies it into a variable
/// under `vars` first.
fn mentions_cli_args(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains("CLI_ARGS"),
        Value::Array(items) => items.iter().any(mentions_cli_args),
        Value::Object(map) => map.values().any(mentions_cli_args),
        _ => false,
    }
}

/// Matches `name` against a task name where each `*` stands for any text, as
/// task's `^...(.*)...$` regex does.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(index) => rest = &rest[index + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Resolves `.` and `..` lexically so the worktree check needs no filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASKFILE: &str = r#"
version: '3'

tasks:
  build: go build -o app{{exeExt}} .

  lint:
    - golangci-lint run
    - task: build

  dev:
    aliases: [d, serve:*]
    vars:
      ARGS: '{{.CLI_ARGS}}'
    cmds:
      - vite {{.ARGS}}

  start:*:*:
    cmd: echo {{index .MATCH 0}}

  start:*:
    cmds:
      - echo {{index .MATCH 0}}

  migrate:
    internal: true
    cmds:
      - ./migrate.sh
"#;

    /// Resolution follows task's order: exact name, exact alias, then the
    /// first wildcard in file order.
    #[test]
    fn finds_tasks_like_task() {
        let tasks = parse(TASKFILE).expect("parse");
        let found = |name: &str| match tasks.check(name) {
            TaskCheck::Found(task) => Some(task.name),
            _ => None,
        };
        for (call, expected) in [
            ("build", Some("build")),
            ("d", Some("dev")),
            ("serve:web", Some("dev")),
            ("start:api", Some("start:*")),
            ("start:api:3", Some("start:*:*")),
            ("start:*", Some("start:*")),
            ("start", None),
            ("dev:x", None),
        ] {
            assert_eq!(found(call).as_deref(), expected, "{call}");
        }
        assert_eq!(
            tasks.check("seed"),
            TaskCheck::Missing {
                known: ["build", "lint", "dev", "start:*:*", "start:*"]
                    .map(str::to_owned)
                    .to_vec()
            }
        );
    }

    /// Shorthand tasks have no aliases; `internal` and `CLI_ARGS` anywhere in
    /// the task are picked up.
    #[test]
    fn reads_task_flags() {
        let tasks = parse(TASKFILE).expect("parse");
        let flags: Vec<(&str, bool, bool)> = tasks
            .tasks
            .iter()
            .map(|task| (task.name.as_str(), task.internal, task.reads_cli_args))
            .collect();
        assert_eq!(
            flags,
            [
                ("build", false, false),
                ("lint", false, false),
                ("dev", false, true),
                ("start:*:*", false, false),
                ("start:*", false, false),
                ("migrate", true, false),
            ]
        );
    }

    /// Includes may provide tasks the parser cannot see; a task in the file
    /// itself is still found.
    #[test]
    fn includes_make_missing_unverifiable() {
        let tasks = parse(
            "version: '3'\nincludes:\n  docs: ./docs\n  lib:\n    taskfile: ./lib\n    flatten: true\ntasks:\n  build: make\n",
        )
        .expect("parse");
        assert_eq!(
            tasks.check("docs:serve"),
            TaskCheck::Unverifiable {
                reason: "it includes other Taskfiles"
            }
        );
        assert!(matches!(tasks.check("build"), TaskCheck::Found(_)));
    }

    /// `*` matches any text, including none and `:`, and the whole name must match.
    #[test]
    fn wildcards() {
        for (pattern, name, matches) in [
            ("a:*", "a:", true),
            ("a:*", "a:b:c", true),
            ("*:b", "a:b", true),
            ("a*b*c", "aXbYbc", true),
            ("a*b*c", "abX", false),
            ("a*a", "a", false),
        ] {
            assert_eq!(wildcard_match(pattern, name), matches, "{pattern} {name}");
        }
    }
}
