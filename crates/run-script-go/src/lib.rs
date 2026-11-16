//! `run_script` support for Go main packages: `{ file: go.mod, name: api }`
//! runs `go run ./cmd/api <args>` in the module's directory.
//!
//! A bare name is a directory under `cmd/`, the usual layout for a module with
//! several programs. `.` or a name with a `/` is a package path relative to
//! `go.mod`. "go run [build flags] [-exec xprog] package [arguments...]"
//! (<https://pkg.go.dev/cmd/go#hdr-Compile_and_run_Go_program>) hands
//! everything after the package to the program, so no `--` is added. Running in
//! the module's directory makes Go pick that `go.mod`, and any `go.work` above it.
//!
//! The package check ignores build constraints: a directory counts as a main
//! package when one of its non-test `.go` files declares `package main`.
//!
//! [`GoPackages`] adapts [`plan`] to `devlanes_core::contrib::ScriptRunner`.

mod adapter;

pub use adapter::GoPackages;

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Component, Path, PathBuf},
};

/// File name handled by this runner.
pub const MANIFEST: &str = "go.mod";

/// Directory a bare package name is looked up in.
const CMD_DIR: &str = "cmd";

/// A `run_script` reference to a Go main package.
#[derive(Debug, Clone, Copy)]
pub struct PackageRequest<'a> {
    /// Root of the lane's worktree; the `go.mod` must be inside it.
    pub worktree: &'a Path,
    /// Path to a `go.mod`, relative to `worktree` or absolute inside it.
    pub source: &'a Path,
    /// A directory under `cmd/` (`api`), or a package path relative to the
    /// module (`.`, `cmd/api`, `./tools/seed`).
    pub name: &'a str,
    /// Arguments for the program, passed after the package.
    pub extra_args: &'a [String],
}

/// What to spawn for a [`PackageRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    /// Directory containing the `go.mod`.
    pub cwd: PathBuf,
    /// Package path as passed to `go run`, such as `./cmd/api`.
    pub package: String,
}

/// Errors from resolving a Go main package.
#[derive(Debug, thiserror::Error)]
pub enum GoError {
    #[error("invalid package `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("`{}` is outside the lane worktree `{}`", .file.display(), .worktree.display())]
    OutsideWorktree { file: PathBuf, worktree: PathBuf },
    #[error("`{}` is not a go.mod", .file.display())]
    NotManifest { file: PathBuf },
    #[error("cannot read `{}`", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
    #[error("`{}` has no main package `{package}` ({})", .file.display(), known_hint(.available))]
    NoMainPackage {
        file: PathBuf,
        package: String,
        available: Vec<String>,
    },
}

/// Formats the package list for [`GoError::NoMainPackage`].
fn known_hint(available: &[String]) -> String {
    if available.is_empty() {
        "no main package at the module root or under cmd/".to_owned()
    } else {
        format!("main packages: {}", available.join(", "))
    }
}

/// Returns whether `path` names a file this runner handles.
pub fn handles(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == MANIFEST)
}

/// Resolves a package reference into the command to spawn.
///
/// Lists the package directory and reads the package clause of its Go files.
/// Errors when the name is not a package path inside the module, or the
/// directory holds no main package.
pub fn plan(request: &PackageRequest<'_>) -> Result<LaunchPlan, GoError> {
    let segments = package_segments(request.name)?;
    let worktree = normalize(request.worktree);
    let file = normalize(&worktree.join(request.source));
    if !file.starts_with(&worktree) {
        return Err(GoError::OutsideWorktree { file, worktree });
    }
    if !handles(&file) {
        return Err(GoError::NotManifest { file });
    }
    std::fs::metadata(&file).map_err(|error| GoError::Read {
        path: file.clone(),
        error,
    })?;
    let cwd = file
        .parent()
        .map_or_else(|| worktree.clone(), Path::to_path_buf);
    let package = render_package(&segments);
    let dir = segments.iter().fold(cwd.clone(), |dir, s| dir.join(s));
    if !is_main_package(&dir)? {
        return Err(GoError::NoMainPackage {
            file,
            package,
            available: main_packages(&cwd)?,
        });
    }
    let mut argv = vec!["go".to_owned(), "run".to_owned(), package.clone()];
    argv.extend(request.extra_args.iter().cloned());
    Ok(LaunchPlan { argv, cwd, package })
}

/// Turns a `run_script` name into the package's directory segments below the
/// module directory. A bare name means `cmd/<name>`.
fn package_segments(name: &str) -> Result<Vec<&str>, GoError> {
    let invalid = |reason| {
        Err(GoError::InvalidName {
            name: name.to_owned(),
            reason,
        })
    };
    if name.is_empty() {
        return invalid("it is empty");
    }
    if name.starts_with('-') {
        return invalid("it starts with `-` and would be read as an option");
    }
    if name.contains("...") {
        return invalid("`...` patterns match several packages; name one main package");
    }
    if name.contains('@') {
        return invalid("a version runs a package outside this module");
    }
    if name.contains('\\') {
        return invalid("use `/` to separate directories");
    }
    if name.starts_with('/')
        || matches!(
            Path::new(name).components().next(),
            Some(Component::Prefix(_) | Component::RootDir)
        )
    {
        return invalid("it is an absolute path; give a path relative to go.mod");
    }
    if name != "." && name != ".." && !name.contains('/') {
        return Ok(vec![CMD_DIR, name]);
    }
    let mut segments = Vec::new();
    for segment in name.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return invalid("it leaves the module directory");
                }
            }
            other => segments.push(other),
        }
    }
    Ok(segments)
}

/// Renders segments the way `go run` needs a local package: `.` or `./a/b`.
fn render_package<S: AsRef<str>>(segments: &[S]) -> String {
    let mut package = ".".to_owned();
    for segment in segments {
        package.push('/');
        package.push_str(segment.as_ref());
    }
    package
}

/// Returns whether `dir` holds a non-test Go file declaring `package main`.
/// A missing directory is not a main package.
fn is_main_package(dir: &Path) -> Result<bool, GoError> {
    let read = |error| GoError::Read {
        path: dir.to_path_buf(),
        error,
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(read(error)),
    };
    for entry in entries {
        let path = entry.map_err(read)?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // go ignores files starting with `.` or `_`, and tests are never the program.
        if !name.ends_with(".go")
            || name.ends_with("_test.go")
            || name.starts_with(['.', '_'])
            || !path.is_file()
        {
            continue;
        }
        if package_clause(&path)?.as_deref() == Some("main") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Main packages at the module root and directly under `cmd/`, for the error.
fn main_packages(module: &Path) -> Result<Vec<String>, GoError> {
    let mut found = Vec::new();
    if is_main_package(module)? {
        found.push(".".to_owned());
    }
    let cmd = module.join(CMD_DIR);
    let Ok(entries) = std::fs::read_dir(&cmd) else {
        return Ok(found);
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| GoError::Read {
            path: cmd.clone(),
            error,
        })?;
        if let Some(name) = entry.file_name().to_str()
            && is_main_package(&entry.path())?
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    found.extend(names.iter().map(|name| render_package(&[CMD_DIR, name])));
    Ok(found)
}

/// Reads the package name of a Go file: the first token after blank lines,
/// `//` comments and `/* */` comments must be `package <name>`. Stops at the
/// clause, so large files are not read whole.
fn package_clause(path: &Path) -> Result<Option<String>, GoError> {
    let read = |error| GoError::Read {
        path: path.to_path_buf(),
        error,
    };
    let mut reader = BufReader::new(File::open(path).map_err(read)?);
    let mut in_block = false;
    let mut first = true;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw).map_err(read)? == 0 {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&raw);
        let mut line: &str = &text;
        if first {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
            first = false;
        }
        loop {
            if in_block {
                match line.split_once("*/") {
                    Some((_, rest)) => {
                        in_block = false;
                        line = rest;
                    }
                    None => break,
                }
            }
            line = line.trim_start();
            if line.is_empty() || line.starts_with("//") {
                break;
            }
            if let Some(rest) = line.strip_prefix("/*") {
                in_block = true;
                line = rest;
                continue;
            }
            return Ok(parse_clause(line));
        }
    }
}

/// Parses `package <name>` at the start of `line`.
fn parse_clause(line: &str) -> Option<String> {
    let rest = line.strip_prefix("package")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_owned())
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
