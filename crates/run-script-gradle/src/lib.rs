//! `run_script` support for Gradle: `{ file: build.gradle.kts, name: <arguments> }`
//! runs `gradle <arguments>`, or the project's Gradle wrapper, in that file's
//! directory.
//!
//! The file is `build.gradle`, `build.gradle.kts`, `settings.gradle` or
//! `settings.gradle.kts`. Gradle finds the settings file by walking up from
//! its working directory, so a build file in a subproject runs that
//! subproject's tasks, and `:web:bootRun` works from the settings directory.
//!
//! `name` holds the arguments as typed after `gradle`, split with POSIX shell
//! quoting (`bootRun`, `quarkusDev`, `:web:bootRun --offline`). A backslash
//! escapes the next character, as in a shell. Arguments from contributors and
//! presets (`--no-daemon`, `--console=plain`, `-Ddebug=5005`) follow the
//! user's.
//!
//! The program is the nearest `gradlew` (`gradlew.bat` on Windows) from the
//! file's directory up to the worktree root, else `gradle` from `PATH` (on
//! Windows the runner finds `gradle.bat` through `PATHEXT`).
//!
//! Task names are not checked: listing them needs a Gradle configuration
//! run, which takes seconds, and Gradle fails at once with "Task 'x' not
//! found", which devlanes shows as the unit's failure.
//!
//! [`GradleTasks`] adapts [`plan`] to `devlanes_core::contrib::ScriptRunner`.

mod adapter;

pub use adapter::GradleTasks;

use std::path::{Component, Path, PathBuf};

/// File names handled by this runner.
pub const FILES: [&str; 4] = [
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// Gradle wrapper script looked up from the file's directory to the worktree root.
#[cfg(not(windows))]
pub const WRAPPER: &str = "gradlew";
/// Gradle wrapper script looked up from the file's directory to the worktree root.
#[cfg(windows)]
pub const WRAPPER: &str = "gradlew.bat";

/// Program run from `PATH` when the project has no wrapper. On Windows the
/// runner finds `gradle.bat` through `PATHEXT`.
pub const PROGRAM: &str = "gradle";

/// Options whose value is the next argument, so that argument is not a task.
/// Options without a value or with one attached (`--console=plain`,
/// `-Pkey=value`) need no entry.
const VALUE_OPTIONS: &[&str] = &[
    "-p",
    "--project-dir",
    "-g",
    "--gradle-user-home",
    "-x",
    "--exclude-task",
    "-I",
    "--init-script",
    "-c",
    "--settings-file",
    "-b",
    "--build-file",
    "--include-build",
    "--console",
    "--warning-mode",
    "--max-workers",
    "--priority",
    "--project-cache-dir",
    "-D",
    "--system-prop",
    "-P",
    "--project-prop",
    "-F",
    "--dependency-verification",
    "-M",
    "--write-verification-metadata",
    "--update-locks",
];

/// A `run_script` reference to Gradle tasks.
#[derive(Debug, Clone, Copy)]
pub struct TasksRequest<'a> {
    /// Root of the lane's worktree; the wrapper lookup never goes above it.
    pub worktree: &'a Path,
    /// Path to a Gradle build or settings file, relative to `worktree` or
    /// absolute inside it.
    pub source: &'a Path,
    /// Gradle arguments as typed after `gradle`.
    pub name: &'a str,
    /// More Gradle arguments, appended after `name`'s.
    pub extra_args: &'a [String],
}

/// What to spawn for a [`TasksRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    /// Directory containing the build or settings file.
    pub cwd: PathBuf,
    /// The wrapper in `argv[0]`, or `None` when `gradle` runs from `PATH`.
    pub wrapper: Option<PathBuf>,
    /// Worktree root, to show the wrapper's directory relative to it.
    worktree: PathBuf,
    /// Non-fatal findings, such as a wrapper without the executable bit.
    pub notes: Vec<String>,
}

/// Errors from resolving Gradle tasks.
#[derive(Debug, thiserror::Error)]
pub enum GradleError {
    #[error("invalid Gradle arguments `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("`{}` is outside the lane worktree `{}`", .file.display(), .worktree.display())]
    OutsideWorktree { file: PathBuf, worktree: PathBuf },
    #[error(
        "`{}` is not a Gradle build or settings file (build.gradle, build.gradle.kts, settings.gradle, settings.gradle.kts)",
        .file.display()
    )]
    NotGradle { file: PathBuf },
    #[error("`{}` does not exist", .file.display())]
    Missing { file: PathBuf },
}

/// Returns whether `path` names a file this runner handles.
pub fn handles(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| FILES.iter().any(|file| name == *file))
}

/// Resolves Gradle arguments into the command to spawn.
///
/// Errors when the file is missing or outside the worktree, or `name` is
/// empty, has an unmatched quote or names no task. Looks for the wrapper.
pub fn plan(request: &TasksRequest<'_>) -> Result<LaunchPlan, GradleError> {
    let tokens = split_arguments(request.name)?;
    if tasks(&tokens).is_empty() {
        return Err(GradleError::InvalidName {
            name: request.name.to_owned(),
            reason: "it names no task, such as `bootRun` or `:web:quarkusDev`",
        });
    }
    let worktree = normalize(request.worktree);
    let file = normalize(&worktree.join(request.source));
    if !file.starts_with(&worktree) {
        return Err(GradleError::OutsideWorktree { file, worktree });
    }
    if !handles(&file) {
        return Err(GradleError::NotGradle { file });
    }
    if !file.is_file() {
        return Err(GradleError::Missing { file });
    }
    let cwd = file
        .parent()
        .map_or_else(|| worktree.clone(), Path::to_path_buf);
    let mut notes = Vec::new();
    if escaped_backslash(request.name, &tokens) {
        notes.push(
            "a backslash in run_script.name escaped the next character, as in a shell; use `/` or single quotes for paths".to_owned(),
        );
    }
    let wrapper = find_wrapper(&cwd, &worktree);
    if let Some(wrapper) = &wrapper
        && !executable(wrapper)
    {
        notes.push(format!(
            "`{}` is not executable; run `chmod +x {WRAPPER}` and `git update-index --chmod=+x {WRAPPER}` in its directory",
            shown(&worktree, wrapper)
        ));
    }
    let program = wrapper
        .as_deref()
        .map_or_else(|| PROGRAM.to_owned(), |path| path.display().to_string());
    let mut argv = vec![program];
    argv.extend(tokens);
    argv.extend(request.extra_args.iter().cloned());
    Ok(LaunchPlan {
        argv,
        cwd,
        wrapper,
        worktree,
        notes,
    })
}

impl LaunchPlan {
    /// One line on the program, e.g. `gradlew (wrapper in .)` or
    /// `gradle (no wrapper)`.
    pub fn detail(&self) -> String {
        match &self.wrapper {
            Some(wrapper) => {
                let dir = wrapper.parent().unwrap_or(wrapper);
                let dir = match dir.strip_prefix(&self.worktree) {
                    Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
                    Ok(relative) => relative.display().to_string(),
                    Err(_) => dir.display().to_string(),
                };
                format!("{WRAPPER} (wrapper in {dir})")
            }
            None => format!("{PROGRAM} (no wrapper)"),
        }
    }
}

/// Splits `name` with shell quoting, failing when it is empty or a quote is
/// left open.
pub fn split_arguments(name: &str) -> Result<Vec<String>, GradleError> {
    let invalid = |reason| GradleError::InvalidName {
        name: name.to_owned(),
        reason,
    };
    let tokens = shell_words::split(name).map_err(|_| invalid("it has an unmatched quote"))?;
    if tokens.is_empty() {
        return Err(invalid("it is empty"));
    }
    Ok(tokens)
}

/// The task tokens: those not starting with `-` and not the value of an
/// option in [`VALUE_OPTIONS`].
pub fn tasks(tokens: &[String]) -> Vec<&str> {
    let mut tasks = Vec::new();
    let mut skip_value = false;
    for token in tokens {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if token.starts_with('-') {
            skip_value = VALUE_OPTIONS.contains(&token.as_str());
        } else {
            tasks.push(token.as_str());
        }
    }
    tasks
}

/// Whether splitting consumed a backslash, which on Windows usually means a
/// path lost its separators.
fn escaped_backslash(name: &str, tokens: &[String]) -> bool {
    let count = |text: &str| text.matches('\\').count();
    count(name) > tokens.iter().map(|token| count(token)).sum()
}

/// The nearest wrapper from `cwd` up to `worktree`.
fn find_wrapper(cwd: &Path, worktree: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .take_while(|dir| dir.starts_with(worktree))
        .map(|dir| dir.join(WRAPPER))
        .find(|path| path.is_file())
}

/// Whether the wrapper can be spawned: a checkout made on Windows or with
/// `core.fileMode=false` can drop the executable bit.
#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// Batch files need no executable bit.
#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    true
}

/// `path` relative to the worktree for messages.
fn shown(worktree: &Path, path: &Path) -> String {
    path.strip_prefix(worktree)
        .unwrap_or(path)
        .display()
        .to_string()
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
