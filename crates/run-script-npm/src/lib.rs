//! `run_script` support for `package.json` scripts.
//!
//! [`plan`] picks the package manager from the declared `packageManager` or
//! `devEngines.packageManager` field, falling back to lockfiles, and renders
//! `<pm> run <name>` in the manifest's directory. It reads `package.json`
//! files and lockfile headers on the way up to the lane worktree, so async
//! callers should run it on a blocking thread.
//!
//! Each manager's argv and `--` rules live in its own module (`npm`, `pnpm`,
//! `yarn`, `bun`). [`NpmScripts`] adapts [`plan`] to
//! `devlanes_core::contrib::ScriptRunner`, which places preset arguments after
//! the script name with the separator the manager needs.

mod adapter;
mod bun;
mod detect;
mod manager;
mod npm;
mod pnpm;
#[cfg(windows)]
mod windows;
mod yarn;

pub use adapter::NpmScripts;
pub use detect::{Detection, DetectionSource, detect_package_manager};
pub use manager::PackageManager;
pub use yarn::{YarnLinker, linker as yarn_linker};

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// File name handled by this runner.
pub const MANIFEST: &str = "package.json";

/// A `run_script` reference to a `package.json` script.
#[derive(Debug, Clone, Copy)]
pub struct ScriptRequest<'a> {
    /// Root of the lane's worktree. Lockfile lookup never goes above it.
    pub worktree: &'a Path,
    /// Path to the `package.json`, relative to `worktree` or absolute inside it.
    pub source: &'a Path,
    /// Script name in `scripts`.
    pub name: &'a str,
    /// Arguments forwarded to the script, typically preset flags.
    pub extra_args: &'a [String],
}

/// What to spawn for a [`ScriptRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    /// Directory containing the `package.json`.
    pub cwd: PathBuf,
    pub manager: PackageManager,
    pub detection: DetectionSource,
    /// Version from the declared field, for diagnostics.
    pub version: Option<String>,
    /// Non-fatal findings for `env --explain`, such as a missing
    /// `packageManager` field.
    pub notes: Vec<String>,
    /// The script's command line, for presets that reject hardcoded flags.
    pub script_body: String,
    /// Variables the script runs with besides the unit's (npm's package and
    /// lifecycle variables when npm itself is not started).
    pub env: BTreeMap<String, String>,
    /// Directories to put in front of `PATH`, first wins.
    pub path: Vec<PathBuf>,
}

impl LaunchPlan {
    /// One line on how the manager was chosen, with paths relative to
    /// `worktree`, e.g. `pnpm 10.4.1 (packageManager in package.json)`.
    pub fn detail(&self, worktree: &Path) -> String {
        let shown = |path: &Path| {
            path.strip_prefix(worktree)
                .unwrap_or(path)
                .display()
                .to_string()
        };
        let manager = match (&self.version, self.manager) {
            (Some(version), manager) => format!("{} {version}", manager.field_name()),
            (None, PackageManager::YarnClassic) => "yarn 1".to_owned(),
            (None, PackageManager::YarnBerry) => "yarn 2+".to_owned(),
            (None, manager) => manager.field_name().to_owned(),
        };
        let source = match &self.detection {
            DetectionSource::PackageManagerField(path) => {
                format!("packageManager in {}", shown(path))
            }
            DetectionSource::DevEngines(path) => {
                format!("devEngines.packageManager in {}", shown(path))
            }
            DetectionSource::Lockfile(path) => format!("lockfile {}", shown(path)),
            DetectionSource::Default => "default".to_owned(),
        };
        format!("{manager} ({source})")
    }
}

/// A script read from a `package.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageScript {
    pub manifest: PathBuf,
    pub name: String,
    pub body: String,
}

/// Errors from reading manifests and resolving the package manager.
#[derive(Debug, thiserror::Error)]
pub enum NpmError {
    #[error("invalid script name `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("`{}` is outside the lane worktree `{}`", .file.display(), .worktree.display())]
    OutsideWorktree { file: PathBuf, worktree: PathBuf },
    #[error("`{}` is not a package.json", .file.display())]
    NotManifest { file: PathBuf },
    #[error("cannot read `{}`", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
    #[error("cannot parse `{}`", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        error: serde_json::Error,
    },
    #[error("`{}` has no script `{name}` ({})", .file.display(), available_hint(.available))]
    MissingScript {
        file: PathBuf,
        name: String,
        available: Vec<String>,
    },
    #[error(
        "`{}` has lockfiles of several package managers ({}); set `packageManager` in its package.json or delete the stale lockfile",
        .dir.display(),
        .lockfiles.join(", ")
    )]
    AmbiguousLockfiles {
        dir: PathBuf,
        lockfiles: Vec<String>,
    },
    /// A script argument `cmd.exe` would cut short (a line break).
    #[error("argument {0:?} cannot be passed to an npm script through cmd.exe")]
    UnsafeArgument(String),
}

/// Formats the script list for [`NpmError::MissingScript`].
fn available_hint(available: &[String]) -> String {
    if available.is_empty() {
        "it defines no scripts".to_owned()
    } else {
        format!("available: {}", available.join(", "))
    }
}

/// The subset of `package.json` devlanes reads.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default)]
    pub(crate) scripts: BTreeMap<String, String>,
    #[serde(default, rename = "packageManager")]
    pub(crate) package_manager: Option<String>,
    /// Kept as raw JSON: only `packageManager` inside it is read, in either
    /// the object or the array form.
    #[serde(default, rename = "devEngines")]
    pub(crate) dev_engines: Option<serde_json::Value>,
    /// Presence marks an npm, yarn or bun workspace root.
    #[serde(default)]
    pub(crate) workspaces: Option<serde_json::Value>,
    /// The fields npm exposes to scripts as `npm_package_*` variables, which
    /// only the Windows plan sets itself.
    #[cfg(windows)]
    #[serde(default)]
    pub(crate) name: Option<serde_json::Value>,
    #[cfg(windows)]
    #[serde(default)]
    pub(crate) version: Option<serde_json::Value>,
    #[cfg(windows)]
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
    #[cfg(windows)]
    #[serde(default)]
    pub(crate) engines: Option<serde_json::Value>,
    #[cfg(windows)]
    #[serde(default)]
    pub(crate) bin: Option<serde_json::Value>,
}

/// Parses a `package.json`, returning `None` when the file does not exist.
pub(crate) fn read_manifest(path: &Path) -> Result<Option<Manifest>, NpmError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(NpmError::Read {
                path: path.to_owned(),
                error,
            });
        }
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| NpmError::Parse {
            path: path.to_owned(),
            error,
        })
}

/// Returns whether `path` names a file this runner handles.
pub fn handles(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == MANIFEST)
}

/// Reads script `name` from the manifest at `manifest`.
///
/// Fails when the file is missing, is not valid JSON or has no such script.
pub fn read_script(manifest: &Path, name: &str) -> Result<PackageScript, NpmError> {
    let parsed = read_existing(manifest)?;
    script_of(&parsed, manifest, name)
}

/// [`read_manifest`] for a file that must exist.
fn read_existing(manifest: &Path) -> Result<Manifest, NpmError> {
    read_manifest(manifest)?.ok_or_else(|| NpmError::Read {
        path: manifest.to_owned(),
        error: std::io::ErrorKind::NotFound.into(),
    })
}

/// Script `name` of `parsed`, read from `manifest`.
fn script_of(parsed: &Manifest, manifest: &Path, name: &str) -> Result<PackageScript, NpmError> {
    match parsed.scripts.get(name) {
        Some(body) => Ok(PackageScript {
            manifest: manifest.to_owned(),
            name: name.to_owned(),
            body: body.clone(),
        }),
        None => Err(NpmError::MissingScript {
            file: manifest.to_owned(),
            name: name.to_owned(),
            available: parsed.scripts.keys().cloned().collect(),
        }),
    }
}

/// Resolves a script reference into the command to spawn.
///
/// On Windows an npm-managed script runs without npm: its `pre`, main and
/// `post` steps run as npm runs them, with npm's package variables and
/// `node_modules\.bin` directories (see `windows.rs`), because npm turns any
/// stop of its script into exit code 1 there. Elsewhere, and for the other
/// managers, the plan is `<pm> run <name>`.
///
/// Errors when the source escapes the worktree, the script is missing or,
/// without a declared field, lockfiles of different managers share a
/// directory.
pub fn plan(request: &ScriptRequest<'_>) -> Result<LaunchPlan, NpmError> {
    validate_name(request.name)?;
    let worktree = normalize(request.worktree);
    let file = normalize(&worktree.join(request.source));
    if !file.starts_with(&worktree) {
        return Err(NpmError::OutsideWorktree { file, worktree });
    }
    if !handles(&file) {
        return Err(NpmError::NotManifest { file });
    }
    let manifest = read_existing(&file)?;
    let script = script_of(&manifest, &file, request.name)?;
    let cwd = file
        .parent()
        .map_or_else(|| worktree.clone(), Path::to_path_buf);
    let detection = detect_package_manager(&cwd, &worktree)?;
    let launch = LaunchPlan {
        argv: detection.manager.run_argv(request.name, request.extra_args),
        cwd,
        manager: detection.manager,
        detection: detection.source,
        version: detection.version,
        notes: detection.notes,
        script_body: script.body,
        env: BTreeMap::new(),
        path: Vec::new(),
    };
    #[cfg(windows)]
    let launch = if launch.manager == PackageManager::Npm {
        without_npm(launch, &file, &manifest, &worktree, request)?
    } else {
        launch
    };
    Ok(launch)
}

/// `launch` for an npm script, run without npm (see [`plan`]).
#[cfg(windows)]
fn without_npm(
    mut launch: LaunchPlan,
    file: &Path,
    manifest: &Manifest,
    worktree: &Path,
    request: &ScriptRequest<'_>,
) -> Result<LaunchPlan, NpmError> {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let npm = windows::npm_plan(
        file,
        manifest,
        worktree,
        home.as_deref(),
        request.name,
        &launch.script_body,
        request.extra_args,
    )?;
    launch.argv = npm.argv;
    launch.env = npm.env;
    launch.path = npm.path;
    launch.notes.push(format!(
        "on Windows npm scripts run through {} without npm",
        npm.shell
    ));
    Ok(launch)
}

/// Rejects names the package manager would parse as one of its own options.
fn validate_name(name: &str) -> Result<(), NpmError> {
    let reason = if name.is_empty() {
        "it is empty"
    } else if name.starts_with('-') {
        "it starts with `-` and would be read as an option"
    } else {
        return Ok(());
    };
    Err(NpmError::InvalidName {
        name: name.to_owned(),
        reason,
    })
}

/// Resolves `.` and `..` lexically. The worktree check must not follow
/// symlinks or require the file to exist before a precise error is possible.
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

    /// Only npm gets a separator, and only when something is forwarded.
    #[test]
    fn separator_rules() {
        let args = vec!["--port".to_owned(), "1".to_owned()];
        assert_eq!(
            PackageManager::Npm.run_argv("dev", &args),
            ["npm", "run", "dev", "--", "--port", "1"]
        );
        assert_eq!(
            PackageManager::Npm.run_argv("dev", &[]),
            ["npm", "run", "dev"]
        );
        for manager in [
            PackageManager::Pnpm,
            PackageManager::YarnClassic,
            PackageManager::YarnBerry,
            PackageManager::Bun,
        ] {
            assert_eq!(manager.run_argv("dev", &args)[3..], ["--port", "1"]);
        }
    }

    /// Parent components cannot climb out of the worktree unnoticed.
    #[test]
    fn normalize_resolves_parent_components() {
        assert_eq!(
            normalize(Path::new("/w/a/../b/./package.json")),
            Path::new("/w/b/package.json")
        );
        assert!(!normalize(Path::new("/w/../x/package.json")).starts_with("/w"));
    }
}
