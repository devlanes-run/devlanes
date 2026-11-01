//! How this devlanes binary was installed, the command that upgrades it, and
//! the stable path that runs it.

use std::path::{Component, Path, PathBuf};

use devlanes_core::paths::user_home;
use devlanes_version::REPOSITORY;

/// How this devlanes binary was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    Homebrew,
    Npm,
    Pnpm,
    Bun,
    Yarn,
    CargoBinstall,
    CargoInstall,
    /// `uv tool install`.
    Uv,
    /// `uvx devlanes`, run from uv's cache.
    Uvx,
    /// `pipx install`.
    Pipx,
    /// `pipx run devlanes`, run from pipx's cache.
    PipxRun,
    /// `pip install` into the Python environment at this directory.
    Pip(PathBuf),
    /// `pip install` into the Python environment uv created at this directory,
    /// which usually has no pip.
    UvPip(PathBuf),
    /// A dependency of one project, which `manifest` (its `package.json`,
    /// `pyproject.toml` or `requirements*.txt`) pins.
    Local {
        manifest: PathBuf,
    },
    /// A `target/` directory of a checkout.
    Source,
    /// `install.sh` (`install.ps1` on Windows), or a copy placed by hand, in
    /// this directory.
    Script(PathBuf),
}

/// What [`InstallMethod::current`] reads from the filesystem for
/// [`InstallMethod::detect`], which only looks at paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallFacts {
    /// cargo binstall's `crates-v1.json` records devlanes.
    pub binstall_lists_devlanes: bool,
    /// The executable belongs to uv's devlanes tool: it sits under
    /// `<uv tool dir>/devlanes/`, or uv's receipt lists it as an entry point.
    pub uv_tool: bool,
    /// The Python environment the executable's directory sits in: its parent
    /// has a `pyvenv.cfg`.
    pub venv: Option<Venv>,
    /// The project that pins this executable: the nearest `package.json` above
    /// the outermost `node_modules` it sits in when that file lists devlanes,
    /// or the `pyproject.toml` or `requirements*.txt` in the directory that
    /// holds its Python environment.
    pub manifest: Option<PathBuf>,
}

/// Who made the Python environment around the executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Venv {
    /// pipx: `pipx_metadata.json` sits next to `pyvenv.cfg`.
    Pipx,
    /// uv: `pyvenv.cfg` has a `uv` key.
    Uv,
    /// Anything else, such as `python -m venv`.
    Other,
}

impl InstallMethod {
    /// Classifies a canonical executable path. `cargo_home` is `$CARGO_HOME` or
    /// `~/.cargo`; `facts` holds what the filesystem says about it.
    pub fn detect(exe: &Path, cargo_home: Option<&Path>, facts: InstallFacts) -> Self {
        let text = os::normalise(&exe.to_string_lossy());
        let has = |needle: &str| text.contains(&os::normalise(needle));
        if has("/Cellar/devlanes/") {
            return Self::Homebrew;
        }
        if has("/node_modules/@devlanes/") || has("/node_modules/devlanes/") {
            if has("/.bun/install/global/") {
                return Self::Bun;
            }
            if has("/pnpm/global/") {
                return Self::Pnpm;
            }
            if os::YARN_GLOBAL.iter().any(|needle| has(needle)) {
                return Self::Yarn;
            }
            // A project's package.json may sit above a global prefix too (a
            // home directory with one, say); the global layouts win.
            if !os::NPM_GLOBAL.iter().any(|needle| has(needle))
                && let Some(manifest) = facts.manifest
            {
                return Self::Local { manifest };
            }
            return if has("/.pnpm/") {
                Self::Pnpm
            } else {
                Self::Npm
            };
        }
        // On macOS and Linux, uv and pipx symlink their bin entries into the
        // tool's environment, so the canonical path names it. pipx does so on
        // Windows too when symlinks are allowed; the copy it falls back to
        // otherwise can't be told apart from a standalone binary.
        if os::UV_TOOLS.iter().any(|needle| has(needle)) {
            return Self::Uv;
        }
        if has("/pipx/venvs/devlanes/") {
            return Self::Pipx;
        }
        // Cached environments have a pyvenv.cfg too, so these come before it.
        if os::UVX_CACHE.iter().any(|needle| has(needle)) {
            return Self::Uvx;
        }
        if os::PIPX_RUN_CACHE.iter().any(|needle| has(needle)) {
            return Self::PipxRun;
        }
        // uv copies the executable on Windows, so only the tool directory or
        // its receipt ties the copy to uv.
        if facts.uv_tool {
            return Self::Uv;
        }
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(venv) = facts.venv
            && let Some(root) = dir.parent()
        {
            return match (venv, facts.manifest) {
                (Venv::Pipx, _) => Self::Pipx,
                (_, Some(manifest)) => Self::Local { manifest },
                (Venv::Uv, None) => Self::UvPip(root.to_path_buf()),
                (Venv::Other, None) => Self::Pip(root.to_path_buf()),
            };
        }
        if cargo_home.is_some_and(|home| os::same_dir(&dir, &home.join("bin"))) {
            return if facts.binstall_lists_devlanes {
                Self::CargoBinstall
            } else {
                Self::CargoInstall
            };
        }
        if has("/target/debug/") || has("/target/release/") {
            return Self::Source;
        }
        Self::Script(dir)
    }

    /// The method of the running binary. Reads the filesystem, so async code
    /// calls it from a blocking task.
    pub fn current() -> Self {
        Self::of(&running_exe().unwrap_or_else(|_| PathBuf::from("devlanes")))
    }

    /// The method of the canonical executable `exe`, with the facts
    /// [`Self::detect`] needs read from the filesystem.
    fn of(exe: &Path) -> Self {
        let cargo_home = std::env::var_os("CARGO_HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| user_home().map(|home| home.join(".cargo")));
        let cargo_home = cargo_home.map(|home| dunce::canonicalize(&home).unwrap_or(home));
        let venv = exe.parent().and_then(Path::parent).and_then(|root| {
            let config = std::fs::read_to_string(root.join("pyvenv.cfg")).ok()?;
            Some(if root.join("pipx_metadata.json").is_file() {
                Venv::Pipx
            } else if pyvenv_made_by_uv(&config) {
                Venv::Uv
            } else {
                Venv::Other
            })
        });
        let manifest = match outermost_node_modules(exe) {
            Some(node_modules) => package_json_listing_devlanes(node_modules),
            // The directory that holds the environment, such as `acme` for
            // `acme/.venv/bin/devlanes`.
            None if venv.is_some() => exe.ancestors().nth(3).and_then(python_project_manifest),
            None => None,
        };
        let facts = InstallFacts {
            binstall_lists_devlanes: cargo_home.as_deref().is_some_and(|home| {
                std::fs::read_to_string(home.join("binstall/crates-v1.json"))
                    .is_ok_and(|text| binstall_lists_devlanes(&text))
            }),
            uv_tool: uv_tool_dirs().into_iter().any(|dir| {
                let tool = dunce::canonicalize(&dir).unwrap_or(dir).join("devlanes");
                is_inside(exe, &tool)
                    || std::fs::read_to_string(tool.join("uv-receipt.toml"))
                        .is_ok_and(|text| uv_receipt_lists(&text, exe))
            }),
            venv,
            manifest,
        };
        Self::detect(exe, cargo_home.as_deref(), facts)
    }

    /// A short name for doctor.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "Bun",
            Self::Yarn => "Yarn",
            Self::CargoBinstall => "cargo binstall",
            Self::CargoInstall => "cargo install",
            Self::Uv => "uv",
            Self::Uvx => "uvx",
            Self::Pipx => "pipx",
            Self::PipxRun => "pipx run",
            Self::Pip(_) => "pip",
            Self::UvPip(_) => "uv pip",
            Self::Local { .. } => "a project dependency",
            Self::Source => "a source checkout",
            Self::Script(_) => os::SCRIPT,
        }
    }

    /// How doctor names the install: [`Self::label`], except that a binary outside
    /// the install script's default directory (`~/.local/bin` under `home`, or
    /// `%LOCALAPPDATA%\Programs\devlanes` on Windows) is named by its directory,
    /// since nothing says the script put it there. pip installs also name their
    /// environment, and project dependencies their manifest.
    pub fn describe(&self, home: Option<&Path>) -> String {
        match self {
            Self::Pip(venv) | Self::UvPip(venv) => {
                format!("{}, in the environment at {}", self.label(), venv.display())
            }
            Self::Local { manifest } => {
                format!("{}, in {}", self.label(), manifest.display())
            }
            Self::Script(dir) => {
                let default = os::default_script_dir(home);
                let default = default
                    .as_deref()
                    .map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
                if default.is_some_and(|default| os::same_dir(&default, dir)) {
                    os::SCRIPT.into()
                } else {
                    format!("a standalone binary in {}", dir.display())
                }
            }
            other => other.label().into(),
        }
    }

    /// The command that installs `version` the same way. On Windows, methods
    /// that overwrite the executable the daemon runs from say to stop it first.
    pub fn upgrade_command(&self, version: &str) -> String {
        let command = match self {
            Self::Homebrew => "brew upgrade devlanes-run/tap/devlanes".into(),
            Self::Npm => "npm install -g devlanes@latest".into(),
            Self::Pnpm => "pnpm add -g devlanes@latest".into(),
            Self::Bun => "bun add -g devlanes@latest".into(),
            Self::Yarn => "yarn global add devlanes@latest".into(),
            Self::CargoBinstall => format!("cargo binstall --git {REPOSITORY} devlanes"),
            Self::CargoInstall => format!(
                "cargo install --locked --git {REPOSITORY} --tag v{version} {}",
                os::CARGO_PACKAGES
            ),
            Self::Uv => "uv tool upgrade devlanes".into(),
            Self::Uvx => "uvx devlanes@latest".into(),
            Self::Pipx => "pipx upgrade devlanes".into(),
            Self::PipxRun => "pipx run --no-cache devlanes".into(),
            Self::Pip(venv) => os::pip_upgrade(venv),
            Self::UvPip(venv) => os::uv_pip_upgrade(venv),
            Self::Local { manifest } => format!("update devlanes in {}", manifest.display()),
            Self::Source => format!("git pull, then cargo build --release (v{version})"),
            Self::Script(dir) => os::script_upgrade(dir),
        };
        if os::needs_daemon_stop(self) {
            format!("devlanes daemon stop (it stops services), then {command}")
        } else {
            command
        }
    }

    /// The path that runs this install after an upgrade, for the canonical
    /// executable `exe` of this method. Script installs keep `devlanes` in
    /// their directory. Homebrew's Cellar path names the version, and `brew
    /// cleanup` deletes it after an upgrade, so this is the `opt` link of the
    /// prefix. pnpm's store path names the version too, so this is the global
    /// `node_modules/devlanes` link when `exe` is the binary the package's
    /// postinstall placed there. Every other method upgrades in place: `exe`.
    pub fn entry(&self, exe: &Path) -> PathBuf {
        let stable = match self {
            Self::Script(dir) => {
                Some(dir.join(format!("devlanes{}", std::env::consts::EXE_SUFFIX)))
            }
            Self::Homebrew => homebrew_entry(exe),
            Self::Pnpm => pnpm_entry(exe),
            _ => None,
        };
        stable.unwrap_or_else(|| exe.to_path_buf())
    }
}

/// The executable a process should start to run devlanes again, such as the
/// daemon a client spawns or the one a login service names.
///
/// That is this install's [`InstallMethod::entry`] when the file exists, else
/// this process's executable. A long-running process (a TUI, the daemon)
/// outlives upgrades: Homebrew's `brew cleanup` deletes the old Cellar
/// directory it runs from, and Linux reports a replaced executable as
/// `<path> (deleted)`. Starting the entry runs the binary installed now.
/// Blocking filesystem work.
pub fn spawn_exe() -> std::io::Result<PathBuf> {
    let exe = running_exe()?;
    let entry = InstallMethod::of(&exe).entry(&exe);
    Ok(if entry.is_file() { entry } else { exe })
}

/// This process's executable, canonical when it still exists.
fn running_exe() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    // Linux reads `/proc/self/exe`, which names a replaced file
    // `<path> (deleted)`; the path itself may hold the new binary.
    #[cfg(target_os = "linux")]
    let exe = if exe.exists() {
        exe
    } else {
        without_deleted_suffix(exe)
    };
    Ok(dunce::canonicalize(&exe).unwrap_or(exe))
}

/// `exe` without the ` (deleted)` Linux appends to a replaced executable.
#[cfg(any(target_os = "linux", test))]
fn without_deleted_suffix(exe: PathBuf) -> PathBuf {
    match exe
        .to_str()
        .and_then(|text| text.strip_suffix(" (deleted)"))
    {
        Some(path) => PathBuf::from(path),
        None => exe,
    }
}

/// `<prefix>/opt/devlanes/bin/devlanes` for an executable under
/// `<prefix>/Cellar/devlanes/<version>/`.
fn homebrew_entry(exe: &Path) -> Option<PathBuf> {
    let (prefix, _) = exe.to_str()?.split_once("/Cellar/devlanes/")?;
    Some(Path::new(prefix).join("opt/devlanes/bin/devlanes"))
}

/// `<node_modules>/devlanes/bin/<name>`, the link pnpm keeps to the current
/// version, for `<node_modules>/.pnpm/devlanes@<version>/node_modules/devlanes/bin/<name>`.
/// `None` for any other layout, such as the platform package's binary that the
/// Node launcher runs.
fn pnpm_entry(exe: &Path) -> Option<PathBuf> {
    let parts: Vec<Component<'_>> = exe.components().collect();
    let store = parts.iter().position(|part| part.as_os_str() == ".pnpm")?;
    let (head, tail) = parts.split_at(store);
    if head.last()?.as_os_str() != "node_modules" {
        return None;
    }
    let [_, version, node_modules, package, bin, name] = tail else {
        return None;
    };
    let laid_out = version.as_os_str().to_str()?.starts_with("devlanes@")
        && node_modules.as_os_str() == "node_modules"
        && package.as_os_str() == "devlanes"
        && bin.as_os_str() == "bin";
    laid_out.then(|| {
        head.iter()
            .collect::<PathBuf>()
            .join("devlanes")
            .join("bin")
            .join(name)
    })
}

/// Whether binstall's `crates-v1.json` (concatenated JSON objects) has a
/// `devlanes` entry.
pub fn binstall_lists_devlanes(text: &str) -> bool {
    serde_json::Deserializer::from_str(text)
        .into_iter::<serde_json::Value>()
        .map_while(Result::ok)
        .any(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some("devlanes"))
}

/// The outermost `node_modules` directory `exe` sits in: the project's, when
/// pnpm or a nested install puts more of them below it.
fn outermost_node_modules(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "node_modules"))
        .last()
}

/// The nearest `package.json` above `node_modules`, when it lists devlanes.
/// Blocking filesystem work.
fn package_json_listing_devlanes(node_modules: &Path) -> Option<PathBuf> {
    let file = node_modules
        .parent()?
        .ancestors()
        .map(|dir| dir.join("package.json"))
        .find(|file| file.is_file())?;
    let text = std::fs::read_to_string(&file).ok()?;
    package_json_lists_devlanes(&text).then_some(file)
}

/// Whether a `package.json` has devlanes in its dependencies, dev dependencies
/// or optional dependencies.
fn package_json_lists_devlanes(text: &str) -> bool {
    let Ok(package) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    ["dependencies", "devDependencies", "optionalDependencies"]
        .into_iter()
        .any(|field| package.pointer(&format!("/{field}/devlanes")).is_some())
}

/// The `pyproject.toml` in `project`, else its `requirements.txt`, else its
/// first other `requirements*.txt`. Blocking filesystem work.
fn python_project_manifest(project: &Path) -> Option<PathBuf> {
    let file = |name: &str| Some(project.join(name)).filter(|file| file.is_file());
    file("pyproject.toml")
        .or_else(|| file("requirements.txt"))
        .or_else(|| {
            let mut found: Vec<PathBuf> = std::fs::read_dir(project)
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    name.starts_with("requirements") && name.ends_with(".txt")
                })
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.path())
                .collect();
            found.sort();
            found.into_iter().next()
        })
}

/// Directories uv may keep tools in: `UV_TOOL_DIR` when set, then uv's
/// defaults for this OS.
fn uv_tool_dirs() -> Vec<PathBuf> {
    std::env::var_os("UV_TOOL_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(os::default_uv_tool_dirs())
        .collect()
}

/// Whether `path` lies under `dir`, compared as paths are on this OS.
fn is_inside(path: &Path, dir: &Path) -> bool {
    let dir = os::normalise(&dir.to_string_lossy());
    os::normalise(&path.to_string_lossy()).starts_with(&format!("{}/", dir.trim_end_matches('/')))
}

/// Whether a `pyvenv.cfg` has the `uv` key that uv writes into environments
/// it creates.
fn pyvenv_made_by_uv(config: &str) -> bool {
    config.lines().any(|line| {
        line.split_once('=')
            .is_some_and(|(key, _)| key.trim() == "uv")
    })
}

/// Whether a uv tool receipt (`uv-receipt.toml`) has an entry point installed
/// at `exe`. Reads only the `install-path` strings, compared as paths are on
/// this OS, which is all detection needs and saves a TOML parser.
fn uv_receipt_lists(receipt: &str, exe: &Path) -> bool {
    let exe = os::normalise(&exe.to_string_lossy());
    receipt.split("install-path").skip(1).any(|rest| {
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            return false;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            return false;
        };
        let rest = &rest[1..];
        let Some(end) = rest.find(quote) else {
            return false;
        };
        // Basic strings escape `\`; uv writes `/`, but a hand-edited receipt may not.
        let path = match quote {
            '"' => rest[..end].replace("\\\\", "\\"),
            _ => rest[..end].to_owned(),
        };
        os::normalise(&path) == exe
    })
}

/// macOS and Linux: `install.sh`, POSIX shell quoting, paths matched as they
/// are, and installers that replace a running executable by renaming over it.
#[cfg(unix)]
mod os {
    use std::path::{Path, PathBuf};

    use super::InstallMethod;

    /// `install.sh` of the latest stable release, served by the website.
    const INSTALL_SCRIPT: &str = "https://devlanes.run/install.sh";
    /// The install script's name in doctor.
    pub(super) const SCRIPT: &str = "install.sh";
    /// The packages `cargo install` builds.
    pub(super) const CARGO_PACKAGES: &str = "devlanes devlanes-hosts-helper";
    /// Path fragments of Yarn's global directory.
    pub(super) const YARN_GLOBAL: &[&str] = &["/yarn/global/"];
    /// Path fragments of npm's global packages, `<prefix>/lib/node_modules`.
    pub(super) const NPM_GLOBAL: &[&str] = &["/lib/node_modules/"];
    /// Path fragments of uv's devlanes tool environment.
    pub(super) const UV_TOOLS: &[&str] = &["/uv/tools/devlanes/"];
    /// Path fragments of the environments `uvx` caches, under `~/.cache/uv`.
    pub(super) const UVX_CACHE: &[&str] = &["/uv/archive-v"];
    /// Path fragments of the environments `pipx run` caches: under the pipx
    /// home when `PIPX_HOME` is set, else in the user cache directory
    /// (`~/.cache/pipx` on Linux, `~/Library/Caches/pipx` on macOS).
    pub(super) const PIPX_RUN_CACHE: &[&str] =
        &["/pipx/.cache/", "/.cache/pipx/", "/Library/Caches/pipx/"];

    /// Paths are matched as they are.
    pub(super) fn normalise(path: &str) -> String {
        path.to_owned()
    }

    /// Whether two directories are the same path.
    pub(super) fn same_dir(a: &Path, b: &Path) -> bool {
        a == b
    }

    /// `install.sh`'s default directory, `~/.local/bin`.
    pub(super) fn default_script_dir(home: Option<&Path>) -> Option<PathBuf> {
        home.map(|home| home.join(".local/bin"))
    }

    /// uv's default tool directory for this user.
    pub(super) fn default_uv_tool_dirs() -> Vec<PathBuf> {
        uv_tool_dir_in(
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            devlanes_core::paths::user_home().as_deref(),
        )
        .into_iter()
        .collect()
    }

    /// uv's default tool directory, `$XDG_DATA_HOME/uv/tools` or
    /// `~/.local/share/uv/tools`. uv ignores a relative `XDG_DATA_HOME`.
    pub(super) fn uv_tool_dir_in(
        xdg_data_home: Option<PathBuf>,
        home: Option<&Path>,
    ) -> Option<PathBuf> {
        xdg_data_home
            .filter(|dir| dir.is_absolute())
            .or_else(|| home.map(|home| home.join(".local/share")))
            .map(|data| data.join("uv/tools"))
    }

    /// `install.sh` into `dir`.
    pub(super) fn script_upgrade(dir: &Path) -> String {
        format!(
            "curl -fsSL {INSTALL_SCRIPT} | DEVLANES_INSTALL_DIR={} sh",
            shell_quote(&dir.to_string_lossy())
        )
    }

    /// pip upgrade through the environment's own Python, since its `pip` may
    /// not be on `PATH`.
    pub(super) fn pip_upgrade(venv: &Path) -> String {
        format!(
            "{} -m pip install --upgrade devlanes",
            shell_quote(&venv.join("bin/python").to_string_lossy())
        )
    }

    /// `uv pip` upgrade aimed at the environment, which uv created without pip.
    pub(super) fn uv_pip_upgrade(venv: &Path) -> String {
        format!(
            "uv pip install --upgrade --python {} devlanes",
            shell_quote(&venv.to_string_lossy())
        )
    }

    /// Every installer renames over a running executable.
    pub(super) fn needs_daemon_stop(_method: &InstallMethod) -> bool {
        false
    }

    /// Quotes `text` for a POSIX shell when it needs it.
    fn shell_quote(text: &str) -> String {
        let plain = !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._-+~".contains(&b));
        if plain {
            text.into()
        } else {
            format!("'{}'", text.replace('\'', "'\\''"))
        }
    }
}

/// Windows: `install.ps1`, PowerShell quoting, paths compared with `/` for `\`
/// and without case, and executables that cannot be replaced while they run.
#[cfg(windows)]
mod os {
    use std::path::{Path, PathBuf};

    use super::InstallMethod;

    /// `install.ps1` of the latest stable release, served by the website like
    /// `install.sh`.
    const INSTALL_SCRIPT: &str = "https://devlanes.run/install.ps1";
    /// The install script's name in doctor.
    pub(super) const SCRIPT: &str = "install.ps1";
    /// The packages `cargo install` builds; `devlanes-hosts-helper` is Unix-only.
    pub(super) const CARGO_PACKAGES: &str = "devlanes";
    /// Path fragments of Yarn's global directory: Yarn 1 keeps it under
    /// `%LOCALAPPDATA%\Yarn\Data\global` on Windows.
    pub(super) const YARN_GLOBAL: &[&str] = &["/yarn/global/", "/Yarn/Data/global/"];
    /// Path fragments of npm's global packages, `%APPDATA%\npm\node_modules`.
    pub(super) const NPM_GLOBAL: &[&str] = &["/npm/node_modules/"];
    /// Path fragments of uv's devlanes tool environment, including the legacy
    /// `%APPDATA%\uv\data\tools`.
    pub(super) const UV_TOOLS: &[&str] = &["/uv/tools/devlanes/", "/uv/data/tools/devlanes/"];
    /// Path fragments of the environments `uvx` caches, under
    /// `%LOCALAPPDATA%\uv\cache`.
    pub(super) const UVX_CACHE: &[&str] = &["/uv/cache/archive-v"];
    /// Path fragments of the environments `pipx run` caches: under the pipx
    /// home when `PIPX_HOME` is set, else `%LOCALAPPDATA%\pipx\pipx\Cache`.
    pub(super) const PIPX_RUN_CACHE: &[&str] = &["/pipx/.cache/", "/pipx/pipx/Cache/"];

    /// `\` as `/` and lower case, since Windows paths ignore case and the
    /// patterns use `/`.
    pub(super) fn normalise(path: &str) -> String {
        path.replace('\\', "/").to_lowercase()
    }

    /// Whether two directories are the same path, ignoring case and separators.
    pub(super) fn same_dir(a: &Path, b: &Path) -> bool {
        normalise(&a.to_string_lossy()).trim_end_matches('/')
            == normalise(&b.to_string_lossy()).trim_end_matches('/')
    }

    /// `install.ps1`'s default directory, `%LOCALAPPDATA%\Programs\devlanes`.
    /// The profile directory in `home` does not locate it.
    pub(super) fn default_script_dir(_home: Option<&Path>) -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty())
            .map(|dir| script_dir_in(Path::new(&dir)))
    }

    /// `install.ps1`'s default directory under a `%LOCALAPPDATA%` directory.
    pub(super) fn script_dir_in(local_app_data: &Path) -> PathBuf {
        local_app_data.join("Programs").join("devlanes")
    }

    /// uv's default tool directories under `%APPDATA%`.
    pub(super) fn default_uv_tool_dirs() -> Vec<PathBuf> {
        std::env::var_os("APPDATA")
            .filter(|dir| !dir.is_empty())
            .map(|dir| uv_tool_dirs_in(Path::new(&dir)))
            .unwrap_or_default()
    }

    /// uv's tool directories under an `%APPDATA%` directory: `uv\data\tools`,
    /// which uv keeps using when `uv\data` exists, and `uv\tools`. Detection
    /// tries both, so it needn't know which one uv picked.
    pub(super) fn uv_tool_dirs_in(app_data: &Path) -> Vec<PathBuf> {
        let uv = app_data.join("uv");
        vec![uv.join("data").join("tools"), uv.join("tools")]
    }

    /// `install.ps1` into `dir`, as one PowerShell line. Named as PowerShell,
    /// since cmd.exe and Git Bash would fail on it.
    pub(super) fn script_upgrade(dir: &Path) -> String {
        format!(
            "in PowerShell: $env:DEVLANES_INSTALL_DIR={}; irm {INSTALL_SCRIPT} | iex",
            powershell_quote(&dir.to_string_lossy())
        )
    }

    /// pip upgrade through the environment's own Python, since its `pip` may
    /// not be on `PATH`.
    pub(super) fn pip_upgrade(venv: &Path) -> String {
        let python = venv.join("Scripts").join("python.exe");
        let python = python.to_string_lossy();
        if plain(&python) {
            format!("{python} -m pip install --upgrade devlanes")
        } else {
            format!(
                "in PowerShell: & {} -m pip install --upgrade devlanes",
                powershell_quote(&python)
            )
        }
    }

    /// `uv pip` upgrade aimed at the environment, which uv created without pip.
    pub(super) fn uv_pip_upgrade(venv: &Path) -> String {
        let venv = venv.to_string_lossy();
        if plain(&venv) {
            format!("uv pip install --upgrade --python {venv} devlanes")
        } else {
            format!(
                "in PowerShell: uv pip install --upgrade --python {} devlanes",
                powershell_quote(&venv)
            )
        }
    }

    /// Whether `path` reads the same unquoted in PowerShell and cmd.exe, so
    /// the command needs no shell named.
    fn plain(path: &str) -> bool {
        !path.is_empty()
            && path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || r"\/:._-".contains(c))
    }

    /// Quotes `text` as a PowerShell single-quoted string.
    fn powershell_quote(text: &str) -> String {
        format!("'{}'", text.replace('\'', "''"))
    }

    /// Windows refuses to overwrite or delete a running executable, and the
    /// daemon runs from the installed one. `install.ps1` renames it out of the
    /// way; npm, cargo, uv, pipx, pip and a rebuild do not, so they can fail
    /// while it runs. `uvx` and `pipx run` fetch the new release into another
    /// environment and leave the running one alone. Homebrew has no Windows
    /// build, so its advice stays as on every other OS.
    pub(super) fn needs_daemon_stop(method: &InstallMethod) -> bool {
        !matches!(
            method,
            InstallMethod::Script(_)
                | InstallMethod::Homebrew
                | InstallMethod::Uvx
                | InstallMethod::PipxRun
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects `path` with no filesystem facts.
    fn detect(path: &str) -> InstallMethod {
        detect_with(path, InstallFacts::default())
    }

    /// Detects `path` with `facts` and a fixed cargo home.
    fn detect_with(path: &str, facts: InstallFacts) -> InstallMethod {
        InstallMethod::detect(Path::new(path), Some(Path::new("/Users/a/.cargo")), facts)
    }

    /// Facts for an executable in a Python environment made by `venv`.
    fn in_venv(venv: Venv) -> InstallFacts {
        InstallFacts {
            venv: Some(venv),
            ..InstallFacts::default()
        }
    }

    /// Facts for an executable that belongs to uv's devlanes tool.
    const UV_TOOL: InstallFacts = InstallFacts {
        binstall_lists_devlanes: false,
        uv_tool: true,
        venv: None,
        manifest: None,
    };

    #[test]
    fn detects_install_methods_from_paths() {
        assert_eq!(
            detect("/opt/homebrew/Cellar/devlanes/0.2.0/bin/devlanes"),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect("/home/linuxbrew/.linuxbrew/Cellar/devlanes/0.2.0/bin/devlanes"),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect(
                "/usr/local/lib/node_modules/devlanes/node_modules/@devlanes/darwin-arm64/bin/devlanes"
            ),
            InstallMethod::Npm
        );
        assert_eq!(
            detect(
                "/Users/a/.nvm/versions/node/v24.1.0/lib/node_modules/@devlanes/linux-x64/bin/devlanes"
            ),
            InstallMethod::Npm
        );
        assert_eq!(
            detect(
                "/Users/a/Library/pnpm/global/5/.pnpm/@devlanes+darwin-arm64@0.2.0/node_modules/@devlanes/darwin-arm64/bin/devlanes"
            ),
            InstallMethod::Pnpm
        );
        assert_eq!(
            detect("/Users/a/.bun/install/global/node_modules/@devlanes/darwin-arm64/bin/devlanes"),
            InstallMethod::Bun
        );
        assert_eq!(
            detect("/Users/a/.config/yarn/global/node_modules/@devlanes/linux-arm64/bin/devlanes"),
            InstallMethod::Yarn
        );
        assert_eq!(
            detect("/Users/a/.cargo/bin/devlanes"),
            InstallMethod::CargoInstall
        );
        assert_eq!(
            detect_with(
                "/Users/a/.cargo/bin/devlanes",
                InstallFacts {
                    binstall_lists_devlanes: true,
                    ..InstallFacts::default()
                }
            ),
            InstallMethod::CargoBinstall
        );
        assert_eq!(
            detect("/Users/a/dev/devlanes/target/release/devlanes"),
            InstallMethod::Source
        );
        assert_eq!(
            detect("/Users/a/.local/bin/devlanes"),
            InstallMethod::Script(PathBuf::from("/Users/a/.local/bin"))
        );
        assert_eq!(
            InstallMethod::detect(
                Path::new("/opt/tools/devlanes"),
                None,
                InstallFacts::default()
            ),
            InstallMethod::Script(PathBuf::from("/opt/tools"))
        );
    }

    #[test]
    fn detects_python_installs() {
        assert_eq!(
            detect("/Users/a/.local/share/uv/tools/devlanes/bin/devlanes"),
            InstallMethod::Uv
        );
        assert_eq!(
            detect("/home/a/.data/uv/tools/devlanes/bin/devlanes"),
            InstallMethod::Uv
        );
        assert_eq!(
            detect("/home/a/.local/share/pipx/venvs/devlanes/bin/devlanes"),
            InstallMethod::Pipx
        );
        assert_eq!(
            detect("/Users/a/.local/pipx/venvs/devlanes/bin/devlanes"),
            InstallMethod::Pipx
        );
        // uv and pipx environments have a pyvenv.cfg too; the tool wins.
        assert_eq!(
            detect_with(
                "/Users/a/.local/share/uv/tools/devlanes/bin/devlanes",
                in_venv(Venv::Uv)
            ),
            InstallMethod::Uv
        );
        assert_eq!(
            detect_with(
                "/Users/a/.local/pipx/venvs/devlanes/bin/devlanes",
                in_venv(Venv::Pipx)
            ),
            InstallMethod::Pipx
        );
        // A uv tool directory or pipx home elsewhere is known from the filesystem.
        assert_eq!(
            detect_with("/Users/a/.local/bin/devlanes", UV_TOOL),
            InstallMethod::Uv
        );
        assert_eq!(
            detect_with("/opt/acme/devlanes/bin/devlanes", in_venv(Venv::Pipx)),
            InstallMethod::Pipx
        );
        assert_eq!(
            detect_with("/Users/a/acme/.venv/bin/devlanes", in_venv(Venv::Other)),
            InstallMethod::Pip(PathBuf::from("/Users/a/acme/.venv"))
        );
        assert_eq!(
            detect_with("/Users/a/acme/.venv/bin/devlanes", in_venv(Venv::Uv)),
            InstallMethod::UvPip(PathBuf::from("/Users/a/acme/.venv"))
        );
        assert_eq!(
            detect("/Users/a/acme/.venv/bin/devlanes"),
            InstallMethod::Script(PathBuf::from("/Users/a/acme/.venv/bin"))
        );
    }

    /// Paths that name another installer beat the filesystem facts.
    #[test]
    fn python_facts_come_after_other_installers() {
        assert_eq!(
            detect_with(
                "/opt/homebrew/Cellar/devlanes/0.2.0/bin/devlanes",
                in_venv(Venv::Other)
            ),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect_with(
                "/Users/a/.nvm/versions/node/v24.1.0/lib/node_modules/@devlanes/linux-x64/bin/devlanes",
                in_venv(Venv::Uv)
            ),
            InstallMethod::Npm
        );
        // The uv tool fact is checked before cargo's bin directory.
        assert_eq!(
            detect_with("/Users/a/.cargo/bin/devlanes", UV_TOOL),
            InstallMethod::Uv
        );
    }

    #[test]
    fn python_environment_files() {
        assert!(pyvenv_made_by_uv(
            "home = /usr/bin\nimplementation = CPython\nuv = 0.11.26\nversion_info = 3.12.10\n"
        ));
        assert!(pyvenv_made_by_uv("uv=0.11.26"));
        assert!(!pyvenv_made_by_uv(
            "home = /usr/bin\ninclude-system-site-packages = false\nversion = 3.12.10\n"
        ));
        assert!(!pyvenv_made_by_uv("prompt = uv\nuvx = 1\n"));
        assert!(is_inside(
            Path::new("/srv/uv-tools/devlanes/bin/devlanes"),
            Path::new("/srv/uv-tools/devlanes")
        ));
        assert!(is_inside(
            Path::new("/srv/uv-tools/devlanes/bin/devlanes"),
            Path::new("/srv/uv-tools/devlanes/")
        ));
        assert!(!is_inside(
            Path::new("/srv/uv-tools/devlanes-old/bin/devlanes"),
            Path::new("/srv/uv-tools/devlanes")
        ));
    }

    #[test]
    fn uv_receipt_entry_points() {
        let receipt = r#"[tool]
requirements = [{ name = "devlanes" }]
entrypoints = [
    { name = "devlanes", install-path = "/Users/a/.local/bin/devlanes", from = "devlanes" },
    { name = "devlanes-hosts", install-path='/Users/a/.local/bin/devlanes-hosts', from = "devlanes" },
]
"#;
        assert!(uv_receipt_lists(
            receipt,
            Path::new("/Users/a/.local/bin/devlanes")
        ));
        assert!(uv_receipt_lists(
            receipt,
            Path::new("/Users/a/.local/bin/devlanes-hosts")
        ));
        assert!(!uv_receipt_lists(
            receipt,
            Path::new("/Users/a/.local/bin/devlanes-other")
        ));
        assert!(!uv_receipt_lists(receipt, Path::new("/opt/tools/devlanes")));
        assert!(!uv_receipt_lists(
            "install-path",
            Path::new("/Users/a/.local/bin/devlanes")
        ));
        assert!(!uv_receipt_lists(
            "garbage",
            Path::new("/Users/a/.local/bin/devlanes")
        ));
    }

    #[test]
    fn python_installs_in_doctor() {
        assert_eq!(InstallMethod::Uv.describe(None), "uv");
        assert_eq!(InstallMethod::Uvx.describe(None), "uvx");
        assert_eq!(InstallMethod::Pipx.describe(None), "pipx");
        assert_eq!(InstallMethod::PipxRun.describe(None), "pipx run");
        assert_eq!(
            InstallMethod::Pip(PathBuf::from("/srv/acme")).label(),
            "pip"
        );
        assert_eq!(
            InstallMethod::Pip(PathBuf::from("/srv/acme")).describe(None),
            "pip, in the environment at /srv/acme"
        );
        assert_eq!(
            InstallMethod::UvPip(PathBuf::from("/srv/acme")).describe(None),
            "uv pip, in the environment at /srv/acme"
        );
    }

    /// `uvx` and `pipx run` keep cached environments with a pyvenv.cfg.
    #[cfg(unix)]
    #[test]
    fn detects_unix_python_caches() {
        for path in [
            "/home/a/.cache/uv/archive-v0/Ab12cD/bin/devlanes",
            "/Users/a/.cache/uv/archive-v0/Ab12cD/bin/devlanes",
        ] {
            assert_eq!(detect_with(path, in_venv(Venv::Uv)), InstallMethod::Uvx);
        }
        for path in [
            "/home/a/.cache/pipx/0f3c9a/bin/devlanes",
            "/Users/a/Library/Caches/pipx/0f3c9a/bin/devlanes",
            "/home/a/.local/share/pipx/.cache/0f3c9a/bin/devlanes",
        ] {
            assert_eq!(
                detect_with(path, in_venv(Venv::Pipx)),
                InstallMethod::PipxRun
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn uv_tool_dir_follows_absolute_xdg_data_home() {
        let home = Some(Path::new("/home/a"));
        assert_eq!(
            os::uv_tool_dir_in(Some(PathBuf::from("/data")), home),
            Some(PathBuf::from("/data/uv/tools"))
        );
        assert_eq!(
            os::uv_tool_dir_in(Some(PathBuf::from("data")), home),
            Some(PathBuf::from("/home/a/.local/share/uv/tools"))
        );
        assert_eq!(
            os::uv_tool_dir_in(Some(PathBuf::new()), home),
            Some(PathBuf::from("/home/a/.local/share/uv/tools"))
        );
        assert_eq!(os::uv_tool_dir_in(None, None), None);
    }

    /// Only install.sh's default directory is named install.sh.
    #[cfg(unix)]
    #[test]
    fn standalone_binaries_are_named_by_directory() {
        let home = Path::new("/nonexistent-home");
        assert_eq!(
            InstallMethod::Script(home.join(".local/bin")).describe(Some(home)),
            "install.sh"
        );
        assert_eq!(
            InstallMethod::Script(PathBuf::from("/opt/tools")).describe(Some(home)),
            "a standalone binary in /opt/tools"
        );
        assert_eq!(InstallMethod::Homebrew.describe(Some(home)), "Homebrew");
    }

    /// Committed TUI screens show Homebrew's advice, so it reads the same on every OS.
    #[test]
    fn homebrew_advice_is_the_same_everywhere() {
        assert_eq!(
            InstallMethod::Homebrew.upgrade_command("0.3.0"),
            "brew upgrade devlanes-run/tap/devlanes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn upgrade_commands() {
        assert_eq!(
            InstallMethod::Pnpm.upgrade_command("0.3.0"),
            "pnpm add -g devlanes@latest"
        );
        assert_eq!(
            InstallMethod::CargoInstall.upgrade_command("0.3.0"),
            "cargo install --locked --git https://github.com/devlanes-run/devlanes --tag v0.3.0 devlanes devlanes-hosts-helper"
        );
        assert_eq!(
            InstallMethod::Script(PathBuf::from("/Users/a/.local/bin")).upgrade_command("0.3.0"),
            "curl -fsSL https://devlanes.run/install.sh | DEVLANES_INSTALL_DIR=/Users/a/.local/bin sh"
        );
        assert_eq!(
            InstallMethod::Script(PathBuf::from("/Users/a/My Tools")).upgrade_command("0.3.0"),
            "curl -fsSL https://devlanes.run/install.sh | DEVLANES_INSTALL_DIR='/Users/a/My Tools' sh"
        );
        assert_eq!(
            InstallMethod::Uv.upgrade_command("0.3.0"),
            "uv tool upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::Pipx.upgrade_command("0.3.0"),
            "pipx upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::Uvx.upgrade_command("0.3.0"),
            "uvx devlanes@latest"
        );
        assert_eq!(
            InstallMethod::PipxRun.upgrade_command("0.3.0"),
            "pipx run --no-cache devlanes"
        );
        assert_eq!(
            InstallMethod::Pip(PathBuf::from("/Users/a/acme/.venv")).upgrade_command("0.3.0"),
            "/Users/a/acme/.venv/bin/python -m pip install --upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::Pip(PathBuf::from("/Users/a/My Projects/.venv"))
                .upgrade_command("0.3.0"),
            "'/Users/a/My Projects/.venv/bin/python' -m pip install --upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::UvPip(PathBuf::from("/Users/a/acme/.venv")).upgrade_command("0.3.0"),
            "uv pip install --upgrade --python /Users/a/acme/.venv devlanes"
        );
        assert_eq!(
            InstallMethod::UvPip(PathBuf::from("/Users/a/it's/.venv")).upgrade_command("0.3.0"),
            "uv pip install --upgrade --python '/Users/a/it'\\''s/.venv' devlanes"
        );
    }

    #[test]
    fn binstall_manifest() {
        let text =
            r#"{"name":"cargo-nextest","version_req":"*"}{"name":"devlanes","version_req":"*"}"#;
        assert!(binstall_lists_devlanes(text));
        assert!(!binstall_lists_devlanes(
            r#"{"name":"devlanes-hosts-helper"}"#
        ));
        assert!(!binstall_lists_devlanes("garbage"));
    }

    #[cfg(windows)]
    #[test]
    fn detects_windows_install_methods_from_paths() {
        let cargo = Path::new(r"C:\Users\a\.cargo");
        let detect = |path: &str| {
            InstallMethod::detect(Path::new(path), Some(cargo), InstallFacts::default())
        };
        assert_eq!(
            detect(
                r"C:\Users\a\AppData\Roaming\npm\node_modules\devlanes\node_modules\@devlanes\win32-x64\bin\devlanes.exe"
            ),
            InstallMethod::Npm
        );
        assert_eq!(
            detect(
                r"C:\Users\a\AppData\Local\pnpm\global\5\.pnpm\@devlanes+win32-arm64@0.4.0\node_modules\@devlanes\win32-arm64\bin\devlanes.exe"
            ),
            InstallMethod::Pnpm
        );
        assert_eq!(
            detect(
                r"C:\Users\a\.bun\install\global\node_modules\@devlanes\win32-x64\bin\devlanes.exe"
            ),
            InstallMethod::Bun
        );
        assert_eq!(
            detect(
                r"C:\Users\a\AppData\Local\Yarn\Data\global\node_modules\@devlanes\win32-x64\bin\devlanes.exe"
            ),
            InstallMethod::Yarn
        );
        // Canonical paths keep the case on disk; CARGO_HOME may be spelled otherwise.
        assert_eq!(
            detect(r"c:\users\A\.Cargo\bin\devlanes.exe"),
            InstallMethod::CargoInstall
        );
        assert_eq!(
            InstallMethod::detect(
                Path::new(r"C:\Users\a\.cargo\bin\devlanes.exe"),
                Some(cargo),
                InstallFacts {
                    binstall_lists_devlanes: true,
                    ..InstallFacts::default()
                }
            ),
            InstallMethod::CargoBinstall
        );
        assert_eq!(
            detect(r"D:\src\devlanes\Target\Release\devlanes.exe"),
            InstallMethod::Source
        );
        assert_eq!(
            detect(r"C:\Users\a\AppData\Local\Programs\devlanes\devlanes.exe"),
            InstallMethod::Script(PathBuf::from(r"C:\Users\a\AppData\Local\Programs\devlanes"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn detects_windows_python_installs() {
        let detect = |path: &str, facts: InstallFacts| {
            InstallMethod::detect(
                Path::new(path),
                Some(Path::new(r"C:\Users\a\.cargo")),
                facts,
            )
        };
        // uv copies the executable into its bin directory; the receipt names it.
        let uv_copy = r"C:\Users\a\.local\bin\devlanes.exe";
        let receipt = r#"[tool]
entrypoints = [
    { name = "devlanes", install-path = "C:/Users/a/.local/bin/devlanes.exe", from = "devlanes" },
]
"#;
        assert!(uv_receipt_lists(receipt, Path::new(uv_copy)));
        assert!(uv_receipt_lists(
            receipt,
            Path::new(r"c:\users\A\.Local\bin\DEVLANES.EXE")
        ));
        assert!(!uv_receipt_lists(
            receipt,
            Path::new(r"C:\Users\a\AppData\Local\Programs\devlanes\devlanes.exe")
        ));
        assert!(uv_receipt_lists(
            r#"install-path = "C:\\Users\\a\\.local\\bin\\devlanes.exe""#,
            Path::new(uv_copy)
        ));
        assert_eq!(detect(uv_copy, UV_TOOL), InstallMethod::Uv);
        assert_eq!(
            detect(uv_copy, InstallFacts::default()),
            InstallMethod::Script(PathBuf::from(r"C:\Users\a\.local\bin"))
        );
        for path in [
            r"C:\Users\a\AppData\Roaming\uv\tools\devlanes\Scripts\devlanes.exe",
            r"C:\Users\a\AppData\Roaming\uv\data\tools\devlanes\Scripts\devlanes.exe",
        ] {
            assert_eq!(detect(path, in_venv(Venv::Uv)), InstallMethod::Uv);
        }
        assert!(is_inside(
            Path::new(r"c:\users\a\appdata\roaming\uv\tools\devlanes\scripts\devlanes.exe"),
            Path::new(r"C:\Users\a\AppData\Roaming\uv\tools\devlanes")
        ));
        // pipx symlinks into its environment when Windows allows symlinks.
        for path in [
            r"C:\Users\a\pipx\venvs\devlanes\Scripts\devlanes.exe",
            r"C:\Users\a\AppData\Local\pipx\pipx\venvs\devlanes\Scripts\devlanes.exe",
        ] {
            assert_eq!(detect(path, in_venv(Venv::Pipx)), InstallMethod::Pipx);
        }
        assert_eq!(
            detect(
                r"C:\Users\a\AppData\Local\uv\cache\archive-v0\Ab12cD\Scripts\devlanes.exe",
                in_venv(Venv::Uv)
            ),
            InstallMethod::Uvx
        );
        for path in [
            r"C:\Users\a\AppData\Local\pipx\pipx\Cache\0f3c9a\Scripts\devlanes.exe",
            r"C:\Users\a\pipx\.cache\0f3c9a\Scripts\devlanes.exe",
        ] {
            assert_eq!(detect(path, in_venv(Venv::Pipx)), InstallMethod::PipxRun);
        }
        assert_eq!(
            detect(r"D:\acme\.venv\Scripts\devlanes.exe", in_venv(Venv::Other)),
            InstallMethod::Pip(PathBuf::from(r"D:\acme\.venv"))
        );
        assert_eq!(
            detect(r"D:\acme\.venv\Scripts\devlanes.exe", in_venv(Venv::Uv)),
            InstallMethod::UvPip(PathBuf::from(r"D:\acme\.venv"))
        );
        assert_eq!(
            os::uv_tool_dirs_in(Path::new(r"C:\Users\a\AppData\Roaming")),
            vec![
                PathBuf::from(r"C:\Users\a\AppData\Roaming\uv\data\tools"),
                PathBuf::from(r"C:\Users\a\AppData\Roaming\uv\tools"),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn install_ps1_directory_comes_from_local_app_data() {
        let dir = os::script_dir_in(Path::new(r"C:\Users\a\AppData\Local"));
        assert_eq!(
            dir,
            PathBuf::from(r"C:\Users\a\AppData\Local\Programs\devlanes")
        );
        assert!(os::same_dir(
            &dir,
            Path::new(r"c:\users\a\appdata\local\programs\devlanes\")
        ));
        assert!(!os::same_dir(&dir, Path::new(r"C:\tools")));
        // The real %LOCALAPPDATA% decides what doctor calls install.ps1.
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let default = os::script_dir_in(Path::new(&local));
            assert_eq!(InstallMethod::Script(default).describe(None), "install.ps1");
        }
        assert_eq!(
            InstallMethod::Script(PathBuf::from(r"C:\tools")).describe(None),
            r"a standalone binary in C:\tools"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_upgrades_stop_the_daemon_when_they_replace_its_exe() {
        assert_eq!(
            InstallMethod::Npm.upgrade_command("0.4.0"),
            "devlanes daemon stop (it stops services), then npm install -g devlanes@latest"
        );
        assert_eq!(
            InstallMethod::CargoBinstall.upgrade_command("0.4.0"),
            "devlanes daemon stop (it stops services), then cargo binstall --git https://github.com/devlanes-run/devlanes devlanes"
        );
        assert_eq!(
            InstallMethod::CargoInstall.upgrade_command("0.4.0"),
            "devlanes daemon stop (it stops services), then cargo install --locked --git https://github.com/devlanes-run/devlanes --tag v0.4.0 devlanes"
        );
        assert_eq!(
            InstallMethod::Uv.upgrade_command("0.4.0"),
            "devlanes daemon stop (it stops services), then uv tool upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::Pipx.upgrade_command("0.4.0"),
            "devlanes daemon stop (it stops services), then pipx upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::Pip(PathBuf::from(r"D:\acme\.venv")).upgrade_command("0.4.0"),
            r"devlanes daemon stop (it stops services), then D:\acme\.venv\Scripts\python.exe -m pip install --upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::Pip(PathBuf::from(r"C:\Users\o'neil\My Env")).upgrade_command("0.4.0"),
            r"devlanes daemon stop (it stops services), then in PowerShell: & 'C:\Users\o''neil\My Env\Scripts\python.exe' -m pip install --upgrade devlanes"
        );
        assert_eq!(
            InstallMethod::UvPip(PathBuf::from(r"D:\acme\.venv")).upgrade_command("0.4.0"),
            r"devlanes daemon stop (it stops services), then uv pip install --upgrade --python D:\acme\.venv devlanes"
        );
        assert_eq!(
            InstallMethod::UvPip(PathBuf::from(r"D:\My Env")).upgrade_command("0.4.0"),
            r"devlanes daemon stop (it stops services), then in PowerShell: uv pip install --upgrade --python 'D:\My Env' devlanes"
        );
        // Cached runs fetch the release into a new environment.
        assert_eq!(
            InstallMethod::Uvx.upgrade_command("0.4.0"),
            "uvx devlanes@latest"
        );
        assert_eq!(
            InstallMethod::PipxRun.upgrade_command("0.4.0"),
            "pipx run --no-cache devlanes"
        );
        assert_eq!(
            InstallMethod::Script(PathBuf::from(r"C:\Users\o'neil\Programs\devlanes"))
                .upgrade_command("0.4.0"),
            "in PowerShell: $env:DEVLANES_INSTALL_DIR='C:\\Users\\o''neil\\Programs\\devlanes'; irm https://devlanes.run/install.ps1 | iex"
        );
    }

    #[test]
    fn deleted_suffix_is_stripped() {
        assert_eq!(
            without_deleted_suffix(PathBuf::from("/home/a/.local/bin/devlanes (deleted)")),
            PathBuf::from("/home/a/.local/bin/devlanes")
        );
        assert_eq!(
            without_deleted_suffix(PathBuf::from("/home/a/.local/bin/devlanes")),
            PathBuf::from("/home/a/.local/bin/devlanes")
        );
        assert_eq!(
            without_deleted_suffix(PathBuf::from("/home/a/(deleted)/devlanes")),
            PathBuf::from("/home/a/(deleted)/devlanes")
        );
    }

    #[test]
    fn package_json_dependency_fields() {
        assert!(package_json_lists_devlanes(
            r#"{"dependencies":{"devlanes":"^0.9.0"}}"#
        ));
        assert!(package_json_lists_devlanes(
            r#"{"devDependencies":{"vite":"7","devlanes":"0.9.0"}}"#
        ));
        assert!(package_json_lists_devlanes(
            r#"{"optionalDependencies":{"devlanes":"*"}}"#
        ));
        assert!(!package_json_lists_devlanes(
            r#"{"peerDependencies":{"devlanes":"*"}}"#
        ));
        assert!(!package_json_lists_devlanes(
            r#"{"name":"devlanes","dependencies":{"@devlanes/linux-x64":"0.9.0"}}"#
        ));
        assert!(!package_json_lists_devlanes("garbage"));
    }

    #[test]
    fn outermost_node_modules_is_the_projects() {
        assert_eq!(
            outermost_node_modules(Path::new(
                "/p/acme/node_modules/.pnpm/devlanes@0.9.0/node_modules/devlanes/bin/devlanes"
            )),
            Some(Path::new("/p/acme/node_modules"))
        );
        assert_eq!(
            outermost_node_modules(Path::new("/p/acme/bin/devlanes")),
            None
        );
    }

    /// The nearest package.json decides, and it must list devlanes.
    #[test]
    fn project_manifests_on_disk() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let project = root.path().join("acme");
        let node_modules = project.join("node_modules");
        std::fs::create_dir_all(node_modules.join("devlanes/bin"))?;
        std::fs::write(
            root.path().join("package.json"),
            r#"{"dependencies":{"devlanes":"*"}}"#,
        )?;
        assert_eq!(
            package_json_listing_devlanes(&node_modules),
            Some(root.path().join("package.json"))
        );
        std::fs::write(project.join("package.json"), r#"{"dependencies":{}}"#)?;
        assert_eq!(package_json_listing_devlanes(&node_modules), None);
        std::fs::write(
            project.join("package.json"),
            r#"{"devDependencies":{"devlanes":"0.9.0"}}"#,
        )?;
        assert_eq!(
            package_json_listing_devlanes(&node_modules),
            Some(project.join("package.json"))
        );

        let python = root.path().join("tool");
        std::fs::create_dir_all(&python)?;
        assert_eq!(python_project_manifest(&python), None);
        std::fs::write(python.join("requirements-dev.txt"), "devlanes\n")?;
        assert_eq!(
            python_project_manifest(&python),
            Some(python.join("requirements-dev.txt"))
        );
        std::fs::write(python.join("requirements.txt"), "devlanes\n")?;
        assert_eq!(
            python_project_manifest(&python),
            Some(python.join("requirements.txt"))
        );
        std::fs::write(python.join("pyproject.toml"), "[project]\n")?;
        assert_eq!(
            python_project_manifest(&python),
            Some(python.join("pyproject.toml"))
        );
        Ok(())
    }
}
