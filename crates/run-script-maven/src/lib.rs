//! `run_script` support for Maven: `{ file: pom.xml, name: <arguments> }` runs
//! `mvn <arguments>`, or the project's Maven wrapper, in the pom's directory.
//!
//! `name` holds the arguments as typed after `mvn`, split with POSIX shell
//! quoting (`spring-boot:run -Pdev`, `-pl web -am spring-boot:run`,
//! `spring-boot:run "-Dspring-boot.run.jvmArguments=-Xmx1g -Dx=y"`). A
//! backslash escapes the next character, as in a shell, so Windows paths need
//! `/` or single quotes. Arguments from contributors and presets (`-B -ntp`,
//! `-Ddebug=5005`) follow the user's: Maven cannot pass arguments to the
//! application, so they are Maven arguments too.
//!
//! The program is the nearest `mvnw` (`mvnw.cmd` on Windows) from the pom's
//! directory up to the worktree root. The wrapper finds `.mvn/` by walking up
//! from its working directory, so a root wrapper works for a module pom.
//! Without one, `mvn` comes from `PATH`; on Windows the runner resolves it to
//! `mvn.cmd` through `PATHEXT`, as it does for `pnpm`.
//!
//! `-pl <module> -am` is never added: `-am` runs the goal in every upstream
//! module too, which fails unless the parent pom skips the plugin there.
//!
//! `prefix:goal` tokens are checked against the plugins declared in the pom
//! and its parents inside the worktree. A miss is only a note, because Maven
//! also finds plugins through parents outside the worktree and the plugin
//! groups in `settings.xml`. The check is skipped when `-f`/`--file` picks
//! another pom.
//!
//! [`MavenGoals`] adapts [`plan`] to `devlanes_core::contrib::ScriptRunner`.

mod adapter;
mod pom;

pub use adapter::MavenGoals;

use std::path::{Component, Path, PathBuf};

/// File name handled by this runner.
pub const MANIFEST: &str = "pom.xml";

/// Maven wrapper script looked up from the pom's directory to the worktree root.
#[cfg(not(windows))]
pub const WRAPPER: &str = "mvnw";
/// Maven wrapper script looked up from the pom's directory to the worktree root.
#[cfg(windows)]
pub const WRAPPER: &str = "mvnw.cmd";

/// Program run from `PATH` when the project has no wrapper. On Windows the
/// runner finds `mvn.cmd` through `PATHEXT`.
pub const PROGRAM: &str = "mvn";

/// Options whose value is the next argument, so that argument is not a goal.
/// Options without a value, or with one attached (`-Pdev`, `-Dx=y`,
/// `--projects=web`), need no entry.
const VALUE_OPTIONS: &[&str] = &[
    "-pl",
    "--projects",
    "-P",
    "--activate-profiles",
    "-f",
    "--file",
    "-s",
    "--settings",
    "-gs",
    "--global-settings",
    "-t",
    "--toolchains",
    "-gt",
    "--global-toolchains",
    "-rf",
    "--resume-from",
    "-T",
    "--threads",
    "-b",
    "--builder",
    "-l",
    "--log-file",
    "-D",
    "--define",
    // Optional values, taken from the next argument when it is not an option.
    "-emp",
    "--encrypt-master-password",
    "-ep",
    "--encrypt-password",
    "--color",
];

/// A `run_script` reference to Maven goals.
#[derive(Debug, Clone, Copy)]
pub struct GoalsRequest<'a> {
    /// Root of the lane's worktree; wrapper and parent lookups never go above it.
    pub worktree: &'a Path,
    /// Path to a `pom.xml`, relative to `worktree` or absolute inside it.
    pub source: &'a Path,
    /// Maven arguments as typed after `mvn`.
    pub name: &'a str,
    /// More Maven arguments, appended after `name`'s.
    pub extra_args: &'a [String],
}

/// What to spawn for a [`GoalsRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    /// Directory containing the pom.
    pub cwd: PathBuf,
    /// The wrapper in `argv[0]`, or `None` when `mvn` runs from `PATH`.
    pub wrapper: Option<PathBuf>,
    /// Worktree root, to show the wrapper's directory relative to it.
    worktree: PathBuf,
    /// Non-fatal findings, such as a plugin prefix with no declared plugin.
    pub notes: Vec<String>,
}

/// Errors from resolving Maven goals.
#[derive(Debug, thiserror::Error)]
pub enum MavenError {
    #[error("invalid Maven arguments `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("`{}` is outside the lane worktree `{}`", .file.display(), .worktree.display())]
    OutsideWorktree { file: PathBuf, worktree: PathBuf },
    #[error("`{}` is not a pom.xml", .file.display())]
    NotPom { file: PathBuf },
    #[error("`{}` does not exist", .file.display())]
    Missing { file: PathBuf },
}

/// Returns whether `path` names a file this runner handles.
pub fn handles(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == MANIFEST)
}

/// Resolves Maven arguments into the command to spawn.
///
/// Errors when the pom is missing or outside the worktree, or `name` is empty,
/// has an unmatched quote or names no goal or phase. Looks for the wrapper and,
/// for `prefix:goal` tokens, reads the pom and its parents in the worktree.
pub fn plan(request: &GoalsRequest<'_>) -> Result<LaunchPlan, MavenError> {
    let tokens = split_arguments(request.name)?;
    let goals = goals(&tokens);
    if goals.is_empty() {
        return Err(MavenError::InvalidName {
            name: request.name.to_owned(),
            reason: "it names no goal or phase, such as `spring-boot:run` or `package`",
        });
    }
    let worktree = normalize(request.worktree);
    let file = normalize(&worktree.join(request.source));
    if !file.starts_with(&worktree) {
        return Err(MavenError::OutsideWorktree { file, worktree });
    }
    if !handles(&file) {
        return Err(MavenError::NotPom { file });
    }
    if !file.is_file() {
        return Err(MavenError::Missing { file });
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
    // With `-f`, Maven builds another pom, so checking this one's plugins would
    // give wrong notes.
    if !alternate_pom(&tokens) {
        notes.extend(pom::plugin_notes(&file, &worktree, &goals));
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
    /// One line on the program, e.g. `mvnw (wrapper in services/api)` or
    /// `mvn (no wrapper)`.
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
pub fn split_arguments(name: &str) -> Result<Vec<String>, MavenError> {
    let invalid = |reason| MavenError::InvalidName {
        name: name.to_owned(),
        reason,
    };
    let tokens = shell_words::split(name).map_err(|_| invalid("it has an unmatched quote"))?;
    if tokens.is_empty() {
        return Err(invalid("it is empty"));
    }
    Ok(tokens)
}

/// The goal and phase tokens: those not starting with `-` and not the value of
/// an option in [`VALUE_OPTIONS`].
pub fn goals(tokens: &[String]) -> Vec<&str> {
    let mut goals = Vec::new();
    let mut skip_value = false;
    for token in tokens {
        // An option is never a value: optional values (`-ep`, `--color`) are
        // only taken from a following argument that is not one.
        if std::mem::take(&mut skip_value) && !token.starts_with('-') {
            continue;
        }
        if token.starts_with('-') {
            skip_value = VALUE_OPTIONS.contains(&token.as_str());
        } else {
            goals.push(token.as_str());
        }
    }
    goals
}

/// Whether the arguments pick another pom with `-f` or `--file`.
fn alternate_pom(tokens: &[String]) -> bool {
    tokens
        .iter()
        .any(|token| token == "-f" || token == "--file" || token.starts_with("--file="))
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
