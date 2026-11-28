//! Installing a release the way devlanes was installed. Script and standalone
//! installs download the release archive, check it as `install.sh` and
//! `install.ps1` do and stage it under `$DEVLANES_HOME/update/<version>/`;
//! [`apply`] then renames the staged binaries over the installed ones.
//! Package-manager installs run the package manager. Every other method only
//! names the command to run ([`Plan::Manual`]). Installs hold [`Lock`].

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    InstallMethod,
    check::{self, CheckError, Urls},
    version::{Version, is_newer},
};

/// The target triple of this build, which names its release archive.
pub const TARGET: &str = env!("DEVLANES_TARGET");

/// The archive format of this OS's releases.
#[cfg(unix)]
const ARCHIVE_SUFFIX: &str = ".tar.gz";
#[cfg(windows)]
const ARCHIVE_SUFFIX: &str = ".zip";

/// The executables the main release archive holds next to `LICENSE`, each
/// installed into the install's directory. Every released `devlanes update`
/// refuses a main archive with any other entry, so a new binary ships in an
/// archive of its own, as [`GUI_BINARY`] does.
#[cfg(unix)]
pub const BINARIES: &[&str] = &["devlanes", "devlanes-hosts"];
#[cfg(windows)]
pub const BINARIES: &[&str] = &["devlanes.exe"];

/// The executable of the GUI archive, `devlanes-VERSION-TARGET-gui.tar.gz`
/// (`.zip` on Windows), which holds it and `LICENSE` under the same top
/// directory as the main archive. It is installed next to [`BINARIES`],
/// where `devlanes` looks for it.
#[cfg(unix)]
pub const GUI_BINARY: &str = "devlanes-gui";
#[cfg(windows)]
pub const GUI_BINARY: &str = "devlanes-gui.exe";

/// What the GUI archive's name adds after the target. It must not follow
/// `devlanes-`: released checksum parsers strip `devlanes-` and would read
/// `gui-...` as a version. After the target, they skip the line, as they
/// skip the debug archives.
const GUI_ARCHIVE_TAG: &str = "-gui";

/// Upper bound on `SHA256SUMS`, a line per archive.
const MAX_SUMS_BYTES: usize = 64 * 1024;
/// Upper bound on a release archive.
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
/// Limit for downloading the archive.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(600);
/// Limit for `tar` and for the staged binary's `--version`.
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);
/// Limit for one package-manager command.
pub const PACKAGE_MANAGER_TIMEOUT: Duration = Duration::from_secs(600);
/// Lines of a failed command's stderr an error keeps.
const TAIL_LINES: usize = 12;

/// Why an install did not happen.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The requested version is not a release version.
    #[error("{0:?} is not a release version")]
    Version(String),
    /// Another install holds the lock past the wait.
    #[error("another devlanes update is running")]
    Busy,
    /// devlanes cannot install this way; `command` is what a person runs.
    #[error("devlanes cannot update this install itself; run {command}")]
    Manual { command: String },
    /// A release file could not be read.
    #[error("cannot download {url}: {source}")]
    Fetch {
        url: String,
        #[source]
        source: CheckError,
    },
    /// The release does not hold what it should: a missing archive, a bad
    /// checksum, unexpected entries, another version.
    #[error("{0}")]
    Release(String),
    /// A filesystem step failed.
    #[error("{what}: {source}")]
    Io {
        what: String,
        #[source]
        source: io::Error,
    },
    /// A package manager failed.
    #[error("{}", .0.title)]
    Command(Box<CommandFailure>),
}

/// A package-manager command that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFailure {
    /// What went wrong: `npm install -g devlanes@0.9.1 exited with 1`.
    pub title: String,
    /// The command as a person would type it.
    pub command: String,
    /// The last lines of its stderr.
    pub tail: String,
    /// What to do next.
    pub hint: String,
}

impl InstallError {
    /// An [`InstallError::Io`] for `what`.
    fn io(what: impl Into<String>) -> impl FnOnce(io::Error) -> Self {
        let what = what.into();
        move |source| Self::Io { what, source }
    }
}

/// A program and its arguments, run without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv {
    /// The executable, by path or by name to look up on `PATH`.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
}

impl Argv {
    /// `program args...` for messages, with the program's file name only.
    pub fn display(&self) -> String {
        let program = self.program.file_name().map_or_else(
            || self.program.to_string_lossy(),
            |name| name.to_string_lossy(),
        );
        std::iter::once(program.into_owned())
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What installing a release means for an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Download, check and stage the release archive, then rename its
    /// binaries into `dir`.
    Archive { dir: PathBuf },
    /// Run these commands in order.
    PackageManager(Vec<Argv>),
    /// devlanes leaves it to a person, who runs `command`.
    Manual { command: String },
}

impl Plan {
    /// The plan for installing `version` over `method`, whose running
    /// executable is `exe`. A script or standalone install in a directory
    /// this user cannot write to, and methods devlanes does not drive (cargo,
    /// `uvx`, `pipx run`, pip, project dependencies, source builds), are
    /// manual. Package managers come from the install's own prefix when it
    /// names one, else from `PATH`. `version` must be exactly a release
    /// version ([`Version::parse_release`]). `retained` says whether the
    /// running executable may be replaced ([`crate::own_exe_retained`]); on
    /// Windows a package manager cannot replace it otherwise, so the plan
    /// says to stop the daemon first and run it by hand. Blocking filesystem
    /// work.
    pub fn new(
        method: &InstallMethod,
        exe: &Path,
        version: &str,
        retained: bool,
    ) -> Result<Self, InstallError> {
        if Version::parse_release(version).is_none() {
            return Err(InstallError::Version(version.to_owned()));
        }
        if let InstallMethod::Script(dir) = method {
            if writable(dir) {
                return Ok(Self::Archive { dir: dir.clone() });
            }
            #[cfg(unix)]
            let command = crate::method::sudo_script_upgrade(dir);
            #[cfg(windows)]
            let command = method.upgrade_command(version);
            return Ok(Self::Manual { command });
        }
        Ok(match package_manager(method, version, exe) {
            Some(_) if cfg!(windows) && !retained => Self::Manual {
                command: format!(
                    "devlanes daemon stop, then {}",
                    method.upgrade_command(version)
                ),
            },
            Some(commands) => Self::PackageManager(on_disk_or_path(commands)),
            None => Self::Manual {
                command: method.upgrade_command(version),
            },
        })
    }
}

/// What `devlanes update` installs over the running build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Install this release, which is newer than the running build.
    Install { version: String },
    /// The running build is this release.
    Current,
    /// The release is older than the running build, so nothing is installed.
    Older,
}

/// What `devlanes update` installs over the running build `running` when
/// `wanted` is the latest release it reads. Only a newer release installs,
/// since an older devlanes may refuse the state database a newer build
/// migrated.
pub fn target(running: &str, wanted: &str) -> Target {
    if is_newer(wanted, running) {
        Target::Install {
            version: wanted.to_owned(),
        }
    } else if !is_newer(running, wanted) {
        Target::Current
    } else {
        Target::Older
    }
}

/// `commands` with every derived tool path that does not exist replaced by
/// the tool's name, which `PATH` then resolves. Blocking filesystem work.
fn on_disk_or_path(commands: Vec<Argv>) -> Vec<Argv> {
    commands
        .into_iter()
        .map(|mut argv| {
            if argv.program.is_absolute() && !argv.program.is_file() {
                argv.program = argv
                    .program
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or(argv.program);
            }
            argv
        })
        .collect()
}

/// The commands that remove `method`'s install of devlanes, whose running
/// executable is `exe`, with the tool found as for an update (see
/// [`Plan::new`]); `None` for methods no package manager removes: script and
/// standalone installs (their files are deleted instead), cached runs
/// (`uvx`, `pipx run`), project dependencies and source builds. Blocking
/// filesystem work.
pub fn removal_commands(method: &InstallMethod, exe: &Path) -> Option<Vec<Argv>> {
    let argv = |program: PathBuf, args: &[&str]| Argv {
        program,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    };
    let tool = |name: &str| tool_path(method, exe).unwrap_or_else(|| PathBuf::from(os::tool(name)));
    let commands = match method {
        InstallMethod::Npm => vec![argv(tool("npm"), &["uninstall", "-g", "devlanes"])],
        InstallMethod::Pnpm => vec![argv(tool("pnpm"), &["remove", "-g", "devlanes"])],
        InstallMethod::Bun => vec![argv(tool("bun"), &["remove", "-g", "devlanes"])],
        InstallMethod::Yarn => vec![argv(tool("yarn"), &["global", "remove", "devlanes"])],
        InstallMethod::Uv => vec![argv(tool("uv"), &["tool", "uninstall", "devlanes"])],
        InstallMethod::Pipx => vec![argv(tool("pipx"), &["uninstall", "devlanes"])],
        InstallMethod::Homebrew => vec![argv(
            tool("brew"),
            &["uninstall", "devlanes-run/tap/devlanes"],
        )],
        InstallMethod::CargoInstall | InstallMethod::CargoBinstall => {
            let mut args = vec!["uninstall"];
            args.extend(os::CARGO_PACKAGES);
            // Only when it is there: cargo fails an uninstall that names a
            // package it never installed, and installs from before it or a
            // build that failed have none.
            if exe.with_file_name(GUI_BINARY).is_file() {
                args.push(crate::GUI_PACKAGE);
            }
            vec![argv(PathBuf::from(os::tool("cargo")), &args)]
        }
        // The environment's own Python, since its `pip` may not be on `PATH`;
        // never another Python from `PATH`, which would remove another install.
        InstallMethod::Pip(venv) => {
            return Some(vec![argv(
                os::venv_python(venv),
                &["-m", "pip", "uninstall", "-y", "devlanes"],
            )]);
        }
        InstallMethod::UvPip(venv) => vec![Argv {
            program: PathBuf::from(os::tool("uv")),
            args: vec![
                "pip".into(),
                "uninstall".into(),
                "--python".into(),
                venv.display().to_string(),
                "devlanes".into(),
            ],
        }],
        _ => return None,
    };
    Some(on_disk_or_path(commands))
}

/// Whether devlanes can install a release over `method` itself, so notices
/// say `devlanes update` instead of the method's own command. A script
/// install in a directory it cannot write to says so when `devlanes update`
/// runs.
pub fn installs_itself(method: &InstallMethod) -> bool {
    matches!(method, InstallMethod::Script(_))
        || package_manager(method, "0.0.0", Path::new("")).is_some()
}

/// The package-manager commands that install `version` over `method`, with
/// the tool's path derived from `exe` where the install names its prefix;
/// `None` for methods devlanes does not drive.
pub fn package_manager(method: &InstallMethod, version: &str, exe: &Path) -> Option<Vec<Argv>> {
    let argv = |program: PathBuf, args: &[&str]| Argv {
        program,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    };
    let package = format!("devlanes@{version}");
    let tool = |name: &str| tool_path(method, exe).unwrap_or_else(|| PathBuf::from(os::tool(name)));
    Some(match method {
        InstallMethod::Npm => vec![argv(tool("npm"), &["install", "-g", &package])],
        InstallMethod::Pnpm => vec![argv(tool("pnpm"), &["add", "-g", &package])],
        InstallMethod::Bun => vec![argv(tool("bun"), &["add", "-g", &package])],
        InstallMethod::Yarn => vec![argv(tool("yarn"), &["global", "add", &package])],
        InstallMethod::Uv => vec![argv(tool("uv"), &["tool", "upgrade", "devlanes"])],
        InstallMethod::Pipx => vec![argv(tool("pipx"), &["upgrade", "devlanes"])],
        InstallMethod::Homebrew => vec![
            argv(tool("brew"), &["update"]),
            argv(tool("brew"), &["upgrade", "devlanes-run/tap/devlanes"]),
        ],
        _ => return None,
    })
}

/// The package manager inside the prefix `exe` was installed under: npm's
/// `<prefix>/bin/npm` (`%APPDATA%\npm\npm.cmd` on Windows), Bun's
/// `~/.bun/bin/bun` and Homebrew's `<prefix>/bin/brew`.
fn tool_path(method: &InstallMethod, exe: &Path) -> Option<PathBuf> {
    let named = |dir: &Path, name: &str| dir.file_name().is_some_and(|n| n == name);
    match method {
        InstallMethod::Npm => exe
            .ancestors()
            .find(|dir| {
                named(dir, "node_modules") && dir.parent().is_some_and(|p| named(p, os::NPM_LIB))
            })
            .and_then(Path::parent)
            .and_then(os::npm_in),
        InstallMethod::Bun => exe
            .ancestors()
            .find(|dir| named(dir, ".bun"))
            .map(|bun| bun.join("bin").join(os::tool("bun"))),
        InstallMethod::Homebrew => exe
            .ancestors()
            .find(|dir| named(dir, "Cellar"))
            .and_then(Path::parent)
            .map(|prefix| prefix.join("bin/brew")),
        _ => None,
    }
}

/// Whether this user can create files in `dir`: it exists and a probe file
/// can be made and removed there. Blocking.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".devlanes-write-probe.{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The exclusive lock every install holds, on `$DEVLANES_HOME/update/lock`.
/// Released when dropped, and by the OS when the process ends.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Lock {
    /// Takes the lock under `home`, waiting up to `wait` while another
    /// install holds it. Blocking.
    pub fn acquire(home: &Path, wait: Duration) -> Result<Self, InstallError> {
        let dir = update_dir(home);
        std::fs::create_dir_all(&dir).map_err(InstallError::io(dir.display().to_string()))?;
        let path = dir.join("lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(InstallError::io(path.display().to_string()))?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err(InstallError::Busy),
                Err(std::fs::TryLockError::Error(source)) => {
                    return Err(InstallError::Io {
                        what: path.display().to_string(),
                        source,
                    });
                }
            }
        }
    }
}

/// `$DEVLANES_HOME/update`.
fn update_dir(home: &Path) -> PathBuf {
    home.join("update")
}

/// Which release to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Release {
    /// This version's own release, `download/v<version>/`.
    Version(String),
    /// The rolling `next` prerelease, `download/next/`, whatever it holds now.
    Next,
}

impl Release {
    /// The release that holds `version`: a `next` build has no tag, so it
    /// comes from the `next` prerelease.
    pub fn of(version: &str) -> Self {
        if check::is_next_build(version) {
            Self::Next
        } else {
            Self::Version(version.to_owned())
        }
    }

    /// The release's folder under [`Urls::releases`].
    fn folder(&self) -> String {
        match self {
            Self::Version(version) => format!("download/v{version}"),
            Self::Next => "download/next".into(),
        }
    }
}

/// The archive line of `SHA256SUMS` for this build's target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SumsEntry {
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// `devlanes-<version>-<target>.tar.gz` (`.zip` on Windows).
    pub archive: String,
    pub version: String,
}

/// Finds the archive for `target` in a `SHA256SUMS` text read from `source`,
/// with the checks `install.sh` makes: the version in the archive name is a
/// release version (and `expect` when given), and the checksum is 64
/// lowercase hex digits.
pub fn parse_sums(
    sums: &str,
    target: &str,
    expect: Option<&str>,
    source: &str,
) -> Result<SumsEntry, InstallError> {
    let suffix = format!("-{target}{ARCHIVE_SUFFIX}");
    for line in sums.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [sha256, archive] = fields[..] else {
            continue;
        };
        // `sha256sum` marks binary mode with `*`.
        let archive = archive.trim_start_matches('*');
        let Some(version) = archive
            .strip_prefix("devlanes-")
            .and_then(|rest| rest.strip_suffix(&suffix))
        else {
            continue;
        };
        if let Some(expect) = expect
            && version != expect
        {
            return Err(InstallError::Release(format!(
                "{source} lists version {version}, expected {expect}"
            )));
        }
        if !release_version(version) {
            return Err(InstallError::Release(format!(
                "unexpected version {version} in {source}"
            )));
        }
        let hex = sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(InstallError::Release(format!(
                "malformed checksum for {archive}"
            )));
        }
        return Ok(SumsEntry {
            sha256: sha256.to_owned(),
            archive: archive.to_owned(),
            version: version.to_owned(),
        });
    }
    Err(InstallError::Release(format!(
        "no {target} archive listed in {source}"
    )))
}

/// `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$`, the pattern the install
/// scripts accept.
fn release_version(text: &str) -> bool {
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

/// The name of the GUI archive of `version` for `target`:
/// `devlanes-VERSION-TARGET-gui.tar.gz`, or `.zip` on Windows.
pub fn gui_archive_name(version: &str, target: &str) -> String {
    format!("devlanes-{version}-{target}{GUI_ARCHIVE_TAG}{ARCHIVE_SUFFIX}")
}

/// The GUI archive's line in a `SHA256SUMS` text read from `source`, for
/// the release whose main archive is `main` (from [`parse_sums`]); `None`
/// when the release has none, as releases before `devlanes-gui` have not.
pub fn parse_gui_sums(
    sums: &str,
    main: &SumsEntry,
    target: &str,
) -> Result<Option<SumsEntry>, InstallError> {
    let wanted = gui_archive_name(&main.version, target);
    for line in sums.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [sha256, archive] = fields[..] else {
            continue;
        };
        if archive.trim_start_matches('*') != wanted {
            continue;
        }
        let hex = sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(InstallError::Release(format!(
                "malformed checksum for {wanted}"
            )));
        }
        return Ok(Some(SumsEntry {
            sha256: sha256.to_owned(),
            archive: wanted,
            version: main.version.clone(),
        }));
    }
    Ok(None)
}

/// Whether `tar`'s listing of an archive holds exactly the entries a release
/// archive has below `top`: `LICENSE` and [`BINARIES`].
pub fn exact_entries(listing: &str, top: &str) -> bool {
    holds_exactly(listing, top, BINARIES)
}

/// [`exact_entries`] for the GUI archive: `LICENSE` and [`GUI_BINARY`].
pub fn exact_gui_entries(listing: &str, top: &str) -> bool {
    holds_exactly(listing, top, &[GUI_BINARY])
}

/// Whether `listing` holds `LICENSE` and `binaries` below `top`, and nothing else.
fn holds_exactly(listing: &str, top: &str, binaries: &[&str]) -> bool {
    let mut found: Vec<&str> = listing
        .lines()
        .map(|line| line.trim_end_matches('\r').trim_end_matches('/'))
        .filter(|line| !line.is_empty() && *line != top)
        .collect();
    found.sort_unstable();
    let mut expected: Vec<String> = std::iter::once("LICENSE")
        .chain(binaries.iter().copied())
        .map(|name| format!("{top}/{name}"))
        .collect();
    expected.sort_unstable();
    found == expected
}

/// A release downloaded, checked and unpacked, waiting for [`apply`]; recorded
/// in `$DEVLANES_HOME/update/staged.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub version: String,
    /// The archive's SHA-256.
    pub sha256: String,
    /// The install's directory, where [`apply`] puts the binaries.
    pub target: PathBuf,
    /// The unpacked archive's directory, holding [`BINARIES`].
    pub files: PathBuf,
    /// Whether `files` also holds [`GUI_BINARY`] from the release's GUI
    /// archive. `None` in a record written before it existed, which cannot
    /// tell a release without one from a GUI archive that was never fetched,
    /// so [`Staged::load`] ignores such a record and the release is staged
    /// again.
    pub gui: Option<bool>,
}

impl Staged {
    /// The staged release under `home`, when its record reads, says whether
    /// it has the GUI binary, and its files are all there. Blocking.
    pub fn load(home: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(update_dir(home).join("staged.json")).ok()?;
        let staged: Self = serde_json::from_str(&text).ok()?;
        (staged.gui.is_some()
            && staged
                .binaries()
                .all(|name| staged.files.join(name).is_file()))
        .then_some(staged)
    }

    /// The binaries [`apply`] installs, in the order it swaps them: the
    /// helpers of [`BINARIES`], [`GUI_BINARY`] when the release had it, and
    /// `devlanes` last, as `install.sh` and `install.ps1` do.
    pub fn binaries(&self) -> impl Iterator<Item = &'static str> + use<> {
        BINARIES[1..]
            .iter()
            .copied()
            .chain((self.gui == Some(true)).then_some(GUI_BINARY))
            .chain(std::iter::once(BINARIES[0]))
    }
}

/// Downloads `release` for this build's target, checks it and unpacks it
/// under `$DEVLANES_HOME/update/<version>/`, for the install in `dir`. A
/// release already staged with the same checksum for `dir` is kept as it is.
/// Only the newest staged version stays. Hold [`Lock`] while it runs.
pub async fn stage(
    home: &Path,
    urls: &Urls,
    release: &Release,
    dir: &Path,
    timeout: Duration,
) -> Result<Staged, InstallError> {
    let base = urls.releases.trim_end_matches('/');
    let sums_url = format!("{base}/{}/SHA256SUMS", release.folder());
    let sums = check::fetch(&sums_url, timeout, MAX_SUMS_BYTES)
        .await
        .map_err(|source| InstallError::Fetch {
            url: sums_url.clone(),
            source,
        })?;
    let expect = match release {
        Release::Version(version) => Some(version.as_str()),
        Release::Next => None,
    };
    let sums = String::from_utf8_lossy(&sums);
    let entry = parse_sums(&sums, TARGET, expect, &sums_url)?;
    // A release from before devlanes-gui has no GUI archive; the update still
    // installs, and the next one brings devlanes-gui.
    let gui = parse_gui_sums(&sums, &entry, TARGET)?;
    let home = home.to_path_buf();
    let loaded = {
        let home = home.clone();
        blocking(move || Ok(Staged::load(&home))).await?
    };
    if let Some(staged) = loaded
        && staged.version == entry.version
        && staged.sha256 == entry.sha256
        && staged.target == dir
        && staged.gui == Some(gui.is_some())
    {
        return Ok(staged);
    }
    let folder = update_dir(&home).join(&entry.version);
    {
        let folder = folder.clone();
        blocking(move || {
            std::fs::create_dir_all(&folder).map_err(InstallError::io(folder.display().to_string()))
        })
        .await?;
    }
    let top = format!("devlanes-{}-{TARGET}", entry.version);
    let fetched = Fetched {
        folder: &folder,
        top: &top,
        url: &format!("{base}/{}", release.folder()),
        timeout,
    };
    fetched.unpack(&entry, exact_entries).await?;
    let files = folder.join(&top);
    let mut probe = tokio::process::Command::new(files.join(BINARIES[0]));
    // The release reports its own version, not the one this process pretends to run.
    probe.env_remove(crate::version::RUNNING_ENV);
    check_reported(
        probe_version(probe, "--version", "devlanes").await,
        &entry.version,
        "devlanes",
    )?;
    if let Some(gui) = &gui {
        // Unpacked into the same top directory, next to the main binaries.
        // Never run: it may need system libraries this machine lacks, and a
        // GUI binary that cannot start must not block an update. Its checksum
        // comes from the same SHA256SUMS whose main archive matched.
        fetched.unpack(gui, exact_gui_entries).await?;
    }
    let staged = Staged {
        version: entry.version,
        sha256: entry.sha256,
        target: dir.to_path_buf(),
        files,
        gui: Some(gui.is_some()),
    };
    let record = staged.clone();
    blocking(move || {
        write_record(&home, &record)?;
        prune(&home, &record.version);
        Ok(())
    })
    .await?;
    Ok(staged)
}

/// What [`install_gui`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiInstall {
    /// [`GUI_BINARY`] was put at this path.
    Installed(PathBuf),
    /// The release has no GUI archive, so nothing was done.
    NotInRelease,
}

/// The directory of `method`'s install when [`install_gui`] should run for
/// it: a script install this user can write to, with no [`GUI_BINARY`] next
/// to `devlanes`. A `devlanes-gui` that is there counts as installed even
/// when it cannot run, since running it is never a check. Package managers
/// install it themselves. Blocking.
pub fn gui_missing(method: &InstallMethod) -> Option<PathBuf> {
    let InstallMethod::Script(dir) = method else {
        return None;
    };
    (writable(dir) && gui_absent(dir)).then(|| dir.clone())
}

/// Whether `dir` has no [`GUI_BINARY`], not even a dangling link. Blocking.
fn gui_absent(dir: &Path) -> bool {
    dir.join(GUI_BINARY).symlink_metadata().is_err()
}

/// What [`repair_gui`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiRepair {
    /// Nothing to do: not a script install, `devlanes-gui` is there, the
    /// release has none, this is no release build, or another install
    /// changed the directory while this one waited for the lock.
    Nothing,
    /// `devlanes-gui` was installed at this path.
    Installed(PathBuf),
    /// It is missing from a script install this user cannot write to; a
    /// person runs `command`.
    Unwritable { command: String },
}

/// For a script install already at `version` that has no [`GUI_BINARY`]
/// (an updater from before it installed this release, or an update that
/// stopped between two renames), installs only that binary of `version` with
/// [`install_gui`], under [`Lock`] taken here with up to `lock_wait`. Once it
/// holds the lock it checks again that the binary is still missing and that
/// the directory's `devlanes` reports exactly `version`, since another
/// install may have run meanwhile; otherwise it does nothing. It runs
/// `devlanes`, never `devlanes-gui`, so one that is there is never replaced,
/// even when it cannot run. Package-manager installs bring it themselves.
pub async fn repair_gui(
    home: &Path,
    urls: &Urls,
    method: &InstallMethod,
    version: &str,
    lock_wait: Duration,
    timeout: Duration,
) -> Result<GuiRepair, InstallError> {
    let InstallMethod::Script(dir) = method else {
        return Ok(GuiRepair::Nothing);
    };
    if Version::parse_release(version).is_none() {
        return Ok(GuiRepair::Nothing);
    }
    // What `Plan::new` names for a directory this user cannot write to.
    #[cfg(unix)]
    let command = crate::method::sudo_script_upgrade(dir);
    #[cfg(windows)]
    let command = method.upgrade_command(version);
    let (home, dir) = (home.to_path_buf(), dir.clone());
    let checked = {
        let (home, dir) = (home.clone(), dir.clone());
        blocking(move || {
            if !gui_absent(&dir) {
                return Ok(Err(GuiRepair::Nothing));
            }
            if !writable(&dir) {
                return Ok(Err(GuiRepair::Unwritable { command }));
            }
            let lock = Lock::acquire(&home, lock_wait)?;
            // Another install may have run while this one waited.
            Ok(if gui_absent(&dir) {
                Ok(lock)
            } else {
                Err(GuiRepair::Nothing)
            })
        })
        .await?
    };
    let _lock = match checked {
        Ok(lock) => lock,
        Err(done) => return Ok(done),
    };
    let mut probe = tokio::process::Command::new(dir.join(BINARIES[0]));
    // The installed devlanes reports its own version, not the one this
    // process pretends to run.
    probe.env_remove(crate::version::RUNNING_ENV);
    if probe_version(probe, "--version", "devlanes")
        .await
        .as_deref()
        != Some(version)
    {
        return Ok(GuiRepair::Nothing);
    }
    let release = Release::of(version);
    Ok(
        match install_gui(&home, urls, &release, version, &dir, timeout).await? {
            GuiInstall::Installed(path) => GuiRepair::Installed(path),
            GuiInstall::NotInRelease => GuiRepair::Nothing,
        },
    )
}

/// Puts only [`GUI_BINARY`] of `version` into the script install in `dir`,
/// for an install that has `devlanes` at that version but lost or never got
/// `devlanes-gui` (an update by a release from before it, or a crash between
/// two renames of [`apply`]). Reads `release`'s `SHA256SUMS`, requires its
/// main archive to be exactly `version`, then downloads and checks only the
/// GUI archive. It never touches the other binaries or `staged.json`, and
/// never runs the binary. The caller checks that `dir` is a writable script
/// install without [`GUI_BINARY`]. Hold [`Lock`] while it runs.
pub async fn install_gui(
    home: &Path,
    urls: &Urls,
    release: &Release,
    version: &str,
    dir: &Path,
    timeout: Duration,
) -> Result<GuiInstall, InstallError> {
    let base = urls.releases.trim_end_matches('/');
    let sums_url = format!("{base}/{}/SHA256SUMS", release.folder());
    let sums = check::fetch(&sums_url, timeout, MAX_SUMS_BYTES)
        .await
        .map_err(|source| InstallError::Fetch {
            url: sums_url.clone(),
            source,
        })?;
    let sums = String::from_utf8_lossy(&sums);
    let entry = parse_sums(&sums, TARGET, Some(version), &sums_url)?;
    let Some(gui) = parse_gui_sums(&sums, &entry, TARGET)? else {
        return Ok(GuiInstall::NotInRelease);
    };
    // Apart from the staged release's folders, which `stage` prunes.
    let folder = update_dir(home).join(format!(".gui-{}", std::process::id()));
    {
        let folder = folder.clone();
        blocking(move || {
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).map_err(InstallError::io(folder.display().to_string()))
        })
        .await?;
    }
    let top = format!("devlanes-{version}-{TARGET}");
    let fetched = Fetched {
        folder: &folder,
        top: &top,
        url: &format!("{base}/{}", release.folder()),
        timeout,
    };
    let unpacked = fetched.unpack(&gui, exact_gui_entries).await;
    let source = folder.join(&top).join(GUI_BINARY);
    let target = dir.join(GUI_BINARY);
    blocking(move || {
        let placed = unpacked.and_then(|()| place_new(&source, &target));
        let _ = std::fs::remove_dir_all(&folder);
        placed.map(|()| GuiInstall::Installed(target))
    })
    .await
}

/// Copies `source` next to `target` as `.<name>.new.<pid>`, makes it
/// executable and renames it into place. Blocking.
fn place_new(source: &Path, target: &Path) -> Result<(), InstallError> {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.new.{}", std::process::id()));
    let placed = os::retry_while_held(|| std::fs::copy(source, &temporary).map(drop))
        .and_then(|()| os::make_executable(&temporary))
        .and_then(|()| os::retry_while_held(|| std::fs::rename(&temporary, target)));
    if placed.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    placed.map_err(InstallError::io(format!(
        "cannot install {}",
        target.display()
    )))
}

/// Where [`stage`] downloads a release's archives and unpacks them.
struct Fetched<'a> {
    /// `$DEVLANES_HOME/update/<version>`.
    folder: &'a Path,
    /// The archives' top directory, `devlanes-<version>-<target>`.
    top: &'a str,
    /// The release folder's URL, without a trailing `/`.
    url: &'a str,
    /// The caller's timeout; downloads get at least [`ARCHIVE_TIMEOUT`].
    timeout: Duration,
}

impl Fetched<'_> {
    /// Downloads `entry`'s archive, checks its checksum and that `holds`
    /// accepts its listing, unpacks it into the folder and deletes it.
    async fn unpack(
        &self,
        entry: &SumsEntry,
        holds: fn(&str, &str) -> bool,
    ) -> Result<(), InstallError> {
        let archive = self.folder.join(&entry.archive);
        let url = format!("{}/{}", self.url, entry.archive);
        check::download(
            &url,
            &archive,
            ARCHIVE_TIMEOUT.max(self.timeout),
            MAX_ARCHIVE_BYTES,
        )
        .await
        .map_err(|source| InstallError::Fetch {
            url: url.clone(),
            source,
        })?;
        let actual = {
            let archive = archive.clone();
            blocking(move || sha256_file(&archive)).await?
        };
        if actual != entry.sha256 {
            return Err(InstallError::Release(format!(
                "checksum mismatch for {}: expected {}, got {actual}",
                entry.archive, entry.sha256
            )));
        }
        let listing = tar(&[os::TAR_LIST, path_arg(&archive)?]).await?;
        if !holds(&listing, self.top) {
            return Err(InstallError::Release(format!(
                "unexpected entries in {}: {}",
                entry.archive,
                listing.split_whitespace().collect::<Vec<_>>().join(", ")
            )));
        }
        tar(&[
            os::TAR_EXTRACT,
            path_arg(&archive)?,
            "-C",
            path_arg(self.folder)?,
        ])
        .await?;
        blocking(move || {
            let _ = std::fs::remove_file(&archive);
            Ok(())
        })
        .await
    }
}

/// Fails unless the staged `name` reported `version`.
fn check_reported(reported: Option<String>, version: &str, name: &str) -> Result<(), InstallError> {
    if reported.as_deref() == Some(version) {
        return Ok(());
    }
    // On Windows, a new executable that reports no version at all may be
    // one an antivirus quarantined or blocked when it was extracted or run.
    let hint = if cfg!(windows) && reported.is_none() {
        "; an antivirus may have quarantined it"
    } else {
        ""
    };
    Err(InstallError::Release(format!(
        "the {version} archive's {name} reports {}{hint}",
        reported.as_deref().unwrap_or("no version")
    )))
}

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Windows: replaced executables that still run (the daemon's, this
    /// process's, a GUI host's), renamed to `<name>.old-<stamp>` and deleted
    /// later by doctor or install.ps1.
    pub in_use: Vec<PathBuf>,
}

/// Puts the binaries of `staged` into its target directory, all or none:
/// each is copied next to its target as `.<name>.new.<pid>` and made
/// executable, then each old binary is set aside and the new one renamed
/// into place, so a running devlanes keeps its open executable. When any
/// step fails, the binaries already swapped get their old files back. On
/// Windows the running executable is set aside as `<name>.old-<stamp>`, as
/// `install.ps1` does, and a copy or rename that another process blocks for
/// a moment is tried again. The staged files are deleted afterwards. Hold
/// [`Lock`]. Blocking.
pub fn apply(home: &Path, staged: &Staged) -> Result<Applied, InstallError> {
    let failed = |target: &Path| {
        let what = format!("cannot install {}", target.display());
        move |source| InstallError::Io { what, source }
    };
    let mut placed: Vec<(PathBuf, PathBuf)> = Vec::new();
    let remove_new = |placed: &[(PathBuf, PathBuf)]| {
        for (temporary, _) in placed {
            let _ = std::fs::remove_file(temporary);
        }
    };
    for name in staged.binaries() {
        let target = staged.target.join(name);
        let temporary = staged
            .target
            .join(format!(".{name}.new.{}", std::process::id()));
        let copied =
            os::retry_while_held(|| std::fs::copy(staged.files.join(name), &temporary).map(drop))
                .and_then(|()| os::make_executable(&temporary));
        placed.push((temporary, target.clone()));
        if let Err(source) = copied {
            remove_new(&placed);
            return Err(failed(&target)(source));
        }
    }
    // Each swapped target and what was set aside for it.
    let mut swapped: Vec<(PathBuf, Aside)> = Vec::new();
    for (temporary, target) in &placed {
        let swap = os::set_aside(target).and_then(|aside| {
            os::retry_while_held(|| std::fs::rename(temporary, target))
                .map(|()| aside.clone())
                .inspect_err(|_| {
                    if let Aside::Kept(old) = &aside {
                        restore(old, target);
                    }
                })
        });
        match swap {
            Ok(aside) => swapped.push((target.clone(), aside)),
            Err(source) => {
                for (target, aside) in swapped.iter().rev() {
                    match aside {
                        Aside::Kept(old) => restore(old, target),
                        Aside::Missing => {
                            let _ = std::fs::remove_file(target);
                        }
                        // The old file is gone; the new one stays.
                        #[cfg(unix)]
                        Aside::Replaced => {}
                    }
                }
                remove_new(&placed);
                return Err(failed(target)(source));
            }
        }
    }
    let mut applied = Applied::default();
    let kept = swapped.into_iter().filter_map(|(_, aside)| match aside {
        Aside::Kept(old) => Some(old),
        _ => None,
    });
    for old in kept {
        // Windows keeps a running executable's file until it exits.
        if std::fs::remove_file(&old).is_err() {
            applied.in_use.push(old);
        }
    }
    clear(home);
    Ok(applied)
}

/// What [`os::set_aside`] did with a binary's old file before the new one
/// is renamed over it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Aside {
    /// There was none.
    Missing,
    /// It is kept at this path until the update is done, so a failure can
    /// put it back.
    Kept(PathBuf),
    /// It could not be kept (a filesystem without hard links); the rename
    /// replaces it for good.
    #[cfg(unix)]
    Replaced,
}

/// Puts `old`, which [`os::set_aside`] made of `target`, back in its place.
/// On Unix `old` is a second link to the same file until `target` changed,
/// and a rename between two links of one file does nothing, so the link
/// left over is deleted.
fn restore(old: &Path, target: &Path) {
    let _ = os::retry_while_held(|| std::fs::rename(old, target));
    let _ = std::fs::remove_file(old);
}

/// Deletes the staged release and its record under `home`.
pub fn clear(home: &Path) {
    let dir = update_dir(home);
    let _ = std::fs::remove_file(dir.join("staged.json"));
    prune(home, "");
}

/// Writes `staged.json` through a temporary file and a rename.
fn write_record(home: &Path, staged: &Staged) -> Result<(), InstallError> {
    let dir = update_dir(home);
    let path = dir.join("staged.json");
    let temporary = dir.join(format!(".staged.json.{}", std::process::id()));
    let text = serde_json::to_vec(staged).map_err(|error| InstallError::Io {
        what: path.display().to_string(),
        source: io::Error::other(error),
    })?;
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(InstallError::io(path.display().to_string()))
}

/// Deletes the version folders under `$DEVLANES_HOME/update` except `keep`.
fn prune(home: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(update_dir(home)) else {
        return;
    };
    for entry in entries.flatten() {
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_dir && entry.file_name() != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The lowercase hex SHA-256 of a file. Blocking.
pub fn sha256_file(path: &Path) -> Result<String, InstallError> {
    let mut file = File::open(path).map_err(InstallError::io(path.display().to_string()))?;
    let mut hasher = HashWriter(Sha256::new());
    io::copy(&mut file, &mut hasher).map_err(InstallError::io(path.display().to_string()))?;
    Ok(hasher
        .0
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Lets `io::copy` feed a SHA-256, since digest 0.11 hashers no longer
/// implement `io::Write`.
struct HashWriter(Sha256);

impl io::Write for HashWriter {
    /// Hashes `bytes`; never fails.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    /// Nothing is buffered.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The version `<exe> --version` prints after `devlanes `, or `None` when it
/// does not run or prints something else.
pub async fn reported_version(exe: &Path) -> Option<String> {
    probe_version(tokio::process::Command::new(exe), "--version", "devlanes").await
}

/// The version `command arg` prints as its second word when the first is
/// `name`: `devlanes --version` prints `devlanes <version>`, and
/// `devlanes-gui version` adds a `build=` word after it.
async fn probe_version(
    mut command: tokio::process::Command,
    arg: &str,
    name: &str,
) -> Option<String> {
    let output = tokio::time::timeout(
        TOOL_TIMEOUT,
        command
            .arg(arg)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut words = text.split_whitespace();
    let version = (words.next() == Some(name))
        .then(|| words.next())
        .flatten()?;
    (output.status.success() && Version::parse(version).is_some()).then(|| version.to_owned())
}

/// Runs the system `tar` with `args` and returns its stdout.
async fn tar(args: &[&str]) -> Result<String, InstallError> {
    let program = os::tar();
    let output = tokio::time::timeout(
        TOOL_TIMEOUT,
        tokio::process::Command::new(&program)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| InstallError::Release("tar timed out".into()))?
    .map_err(InstallError::io(format!(
        "cannot run {}",
        program.display()
    )))?;
    if !output.status.success() {
        return Err(InstallError::Release(format!(
            "tar {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A path as a command argument; the release files live under the devlanes
/// home, whose path is UTF-8 on every supported system.
fn path_arg(path: &Path) -> Result<&str, InstallError> {
    path.to_str().ok_or_else(|| InstallError::Io {
        what: path.display().to_string(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "not UTF-8"),
    })
}

/// Runs blocking `work` off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, InstallError> + Send + 'static,
) -> Result<T, InstallError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| InstallError::Io {
            what: "update task".into(),
            source: io::Error::other(error),
        })?
}

/// Runs `commands` in order, each within [`PACKAGE_MANAGER_TIMEOUT`],
/// appending their output to `log` (`$DEVLANES_HOME/logs/update.log`). The
/// first that fails ends the run with its stderr's last lines.
pub async fn run_package_manager(commands: &[Argv], log: &Path) -> Result<(), InstallError> {
    run_commands(commands, log, "devlanes update").await
}

/// [`run_package_manager`] for any devlanes command that runs a package
/// manager; a failure's hint says to retry with `retry`.
pub async fn run_commands(commands: &[Argv], log: &Path, retry: &str) -> Result<(), InstallError> {
    for argv in commands {
        run_one(argv, log, PACKAGE_MANAGER_TIMEOUT, retry).await?;
    }
    Ok(())
}

/// Runs one package-manager command for [`run_commands`].
async fn run_one(
    argv: &Argv,
    log: &Path,
    timeout: Duration,
    retry: &str,
) -> Result<(), InstallError> {
    let command = argv.display();
    let failure = |title: String, tail: String| {
        InstallError::Command(Box::new(CommandFailure {
            title,
            command: command.clone(),
            tail,
            hint: format!("retry with {retry}, or run {command} yourself"),
        }))
    };
    if let Some(dir) = log.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(InstallError::io(dir.display().to_string()))?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .await
        .map_err(InstallError::io(log.display().to_string()))?;
    let header = format!("\n$ {command}  ({})\n", crate::now_ms());
    let _ = file.write_all(header.as_bytes()).await;
    let stdout = file
        .try_clone()
        .await
        .map_err(InstallError::io(log.display().to_string()))?
        .into_std()
        .await;
    let mut child = tokio::process::Command::new(&argv.program)
        .args(&argv.args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| failure(format!("cannot run {command}: {error}"), String::new()))?;
    let mut stderr = child.stderr.take();
    let read = async {
        let mut bytes = Vec::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_end(&mut bytes).await;
        }
        bytes
    };
    let waited = tokio::time::timeout(timeout, async { tokio::join!(read, child.wait()) }).await;
    let (bytes, status) = match waited {
        Ok(done) => done,
        Err(_) => {
            let _ = child.kill().await;
            return Err(failure(
                format!(
                    "{command} did not finish in {} minutes",
                    timeout.as_secs() / 60
                ),
                String::new(),
            ));
        }
    };
    let _ = file.write_all(&bytes).await;
    let tail = tail(&String::from_utf8_lossy(&bytes));
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(failure(
            match status.code() {
                Some(code) => format!("{command} exited with {code}"),
                None => format!("{command} was killed"),
            },
            tail,
        )),
        Err(error) => Err(failure(format!("{command} failed: {error}"), tail)),
    }
}

/// The last [`TAIL_LINES`] non-empty lines of `text`.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// macOS and Linux: `tar` from `PATH` and renames over running executables.
#[cfg(unix)]
mod os {
    use std::{
        io,
        path::{Path, PathBuf},
    };

    use super::Aside;

    /// The parent of npm's global `node_modules`: `<prefix>/lib`.
    pub(super) const NPM_LIB: &str = "lib";
    /// The packages `cargo install` builds, which `cargo uninstall` removes.
    pub(super) const CARGO_PACKAGES: &[&str] = &["devlanes", "devlanes-hosts-helper"];
    /// `tar` flags that list a gzip archive.
    pub(super) const TAR_LIST: &str = "-tzf";
    /// `tar` flags that extract a gzip archive.
    pub(super) const TAR_EXTRACT: &str = "-xzf";

    /// A package manager's program name.
    pub(super) fn tool(name: &str) -> String {
        name.to_owned()
    }

    /// npm in the prefix that holds `lib/node_modules`, given that `lib`.
    pub(super) fn npm_in(lib: &Path) -> Option<PathBuf> {
        Some(lib.parent()?.join("bin/npm"))
    }

    /// The Python of the environment at `venv`.
    pub(super) fn venv_python(venv: &Path) -> PathBuf {
        venv.join("bin/python")
    }

    /// The system `tar`.
    pub(super) fn tar() -> PathBuf {
        PathBuf::from("tar")
    }

    /// Runs `operation` once: an open file never blocks a rename or copy here.
    pub(super) fn retry_while_held(
        mut operation: impl FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        operation()
    }

    /// Mode 755, as `install.sh` sets.
    pub(super) fn make_executable(path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }

    /// Keeps `target` as a second link, `.<name>.old.<pid>`, so a rename
    /// can replace `target` with no moment where it is missing and the old
    /// file can still come back.
    pub(super) fn set_aside(target: &Path) -> io::Result<Aside> {
        set_aside_with(target, |from, to| std::fs::hard_link(from, to))
    }

    /// [`set_aside`], making the second link with `link`.
    pub(super) fn set_aside_with(
        target: &Path,
        link: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<Aside> {
        if target.symlink_metadata().is_err() {
            return Ok(Aside::Missing);
        }
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let old = target.with_file_name(format!(".{name}.old.{}", std::process::id()));
        let _ = std::fs::remove_file(&old);
        match link(target, &old) {
            Ok(()) => Ok(Aside::Kept(old)),
            // Some filesystems (FAT, exFAT, some network and FUSE mounts)
            // have no hard links. The rename still replaces `target` in one
            // step, as install.sh does; only a failure later in the same
            // update cannot bring this file back.
            Err(_) => Ok(Aside::Replaced),
        }
    }
}

/// Windows: System32's `tar.exe`, which reads zip archives, `.cmd` shims of
/// the Node package managers, and running executables renamed out of the way.
#[cfg(windows)]
mod os {
    use std::{
        io,
        path::{Path, PathBuf},
    };

    use super::Aside;

    /// The parent of npm's global `node_modules`: `%APPDATA%\npm`.
    pub(super) const NPM_LIB: &str = "npm";
    /// The packages `cargo install` builds, which `cargo uninstall` removes;
    /// `devlanes-hosts-helper` is Unix-only.
    pub(super) const CARGO_PACKAGES: &[&str] = &["devlanes"];
    /// `tar` flags that list an archive (zip included).
    pub(super) const TAR_LIST: &str = "-tf";
    /// `tar` flags that extract an archive.
    pub(super) const TAR_EXTRACT: &str = "-xf";

    /// A package manager's program name: npm, pnpm and Yarn install `.cmd`
    /// shims, which `PATH` lookup finds only by their full name.
    pub(super) fn tool(name: &str) -> String {
        match name {
            "npm" | "pnpm" | "yarn" => format!("{name}.cmd"),
            other => format!("{other}.exe"),
        }
    }

    /// npm's shim in `%APPDATA%\npm`, which holds `node_modules`.
    pub(super) fn npm_in(prefix: &Path) -> Option<PathBuf> {
        Some(prefix.join("npm.cmd"))
    }

    /// The Python of the environment at `venv`.
    pub(super) fn venv_python(venv: &Path) -> PathBuf {
        venv.join("Scripts").join("python.exe")
    }

    /// System32's `tar.exe` (bsdtar, which reads zip); a Git Bash `tar` on
    /// `PATH` first would not.
    pub(super) fn tar() -> PathBuf {
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join(r"System32\tar.exe"))
            .unwrap_or_else(|| PathBuf::from("tar.exe"))
    }

    /// Runs `operation`, a copy or rename of an install's binary, again while
    /// another process holds the file without sharing delete or write, as an
    /// antivirus or indexer may for a moment after an executable is written
    /// or run. Blocks for up to about 0.6 s.
    pub(super) fn retry_while_held(operation: impl FnMut() -> io::Result<()>) -> io::Result<()> {
        devlanes_sys_windows::file::retry_while_held(operation)
    }

    /// Windows has no mode bits.
    pub(super) fn make_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// Renames `target` to `<target>.old-<stamp>`, since a running
    /// executable can be renamed but not replaced.
    pub(super) fn set_aside(target: &Path) -> io::Result<Aside> {
        if !target.exists() {
            return Ok(Aside::Missing);
        }
        let stamp = devlanes_core::clock::utc_stamp(crate::now_ms() / 1000);
        let mut old = PathBuf::from(format!("{}.old-{stamp}", target.display()));
        let mut n = 1;
        while old.exists() {
            old = PathBuf::from(format!("{}.old-{stamp}-{n}", target.display()));
            n += 1;
        }
        retry_while_held(|| std::fs::rename(target, &old))?;
        Ok(Aside::Kept(old))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the filesystem has no hard links, the old binary cannot be
    /// kept, so the new one replaces it for good instead of the update
    /// failing.
    #[cfg(unix)]
    #[test]
    fn without_hard_links_the_new_binary_replaces_the_old_one() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("devlanes");
        std::fs::write(&target, "old")?;
        let unsupported = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::Unsupported));
        assert_eq!(os::set_aside_with(&target, unsupported)?, Aside::Replaced);
        assert_eq!(std::fs::read(&target)?, b"old");
        let Aside::Kept(old) = os::set_aside(&target)? else {
            panic!("a second link is kept");
        };
        assert_eq!(std::fs::read(&old)?, b"old");
        assert_eq!(os::set_aside(&dir.path().join("none"))?, Aside::Missing);
        Ok(())
    }

    #[test]
    fn sums_name_the_archive_and_version() {
        let sha = "a".repeat(64);
        let sums = format!(
            "{sha}  devlanes-0.9.1-aarch64-apple-darwin.tar.gz\n{sha}  devlanes-0.9.1-{TARGET}{ARCHIVE_SUFFIX}\n"
        );
        let entry = parse_sums(&sums, TARGET, None, "SUMS").expect("entry");
        assert_eq!(entry.version, "0.9.1");
        assert_eq!(
            entry.archive,
            format!("devlanes-0.9.1-{TARGET}{ARCHIVE_SUFFIX}")
        );
        assert_eq!(entry.sha256, sha);
        assert!(parse_sums(&sums, TARGET, Some("0.9.1"), "SUMS").is_ok());
        let error = parse_sums(&sums, TARGET, Some("0.9.2"), "SUMS").expect_err("pinned");
        assert_eq!(
            error.to_string(),
            "SUMS lists version 0.9.1, expected 0.9.2"
        );
        let next = format!("{sha} *devlanes-0.9.1-next.14-{TARGET}{ARCHIVE_SUFFIX}\n");
        assert_eq!(
            parse_sums(&next, TARGET, None, "SUMS")
                .expect("next")
                .version,
            "0.9.1-next.14"
        );
    }

    /// `parse_sums` is the parser every released `devlanes update` runs: this
    /// test relies on it being unchanged since it shipped in v0.10.0, so any
    /// change to it must keep released behaviour. `install.sh` and
    /// `install.ps1` match lines the same way. Fed the
    /// `SHA256SUMS` of a release with GUI archives, listed first here so no
    /// order hides a match, it still finds the main archive, and the GUI
    /// archive is found apart. A GUI name right after `devlanes-` would
    /// break every released updater.
    #[test]
    fn released_parsers_skip_the_gui_archives() {
        let version = "0.9.1";
        let unix = [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
        ];
        let windows = ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"];
        let line = |n: usize, name: String| format!("{}  {name}\n", format!("{n:x}").repeat(64));
        let mut sums = String::new();
        for (n, target) in unix.iter().chain(&windows).enumerate() {
            let suffix = if windows.contains(target) {
                "zip"
            } else {
                "tar.gz"
            };
            sums.push_str(&line(
                n % 10,
                format!("devlanes-{version}-{target}-gui.{suffix}"),
            ));
        }
        for target in unix {
            sums.push_str(&line(10, format!("devlanes-{version}-{target}.tar.gz")));
        }
        for target in windows {
            sums.push_str(&line(11, format!("devlanes-{version}-{target}.zip")));
        }
        for target in unix.iter().chain(&windows) {
            sums.push_str(&line(
                12,
                format!("devlanes-{version}-{target}-debug.tar.gz"),
            ));
        }
        let own = if cfg!(windows) {
            &windows[..]
        } else {
            &unix[..]
        };
        for target in own {
            let main = parse_sums(&sums, target, Some(version), "SUMS").expect(target);
            assert_eq!(
                main.archive,
                format!("devlanes-{version}-{target}{ARCHIVE_SUFFIX}")
            );
            assert_eq!(
                main.sha256,
                if cfg!(windows) { "b" } else { "a" }.repeat(64)
            );
            let gui = parse_gui_sums(&sums, &main, target).expect("parsed");
            assert_eq!(
                gui.map(|gui| gui.archive),
                Some(gui_archive_name(version, target))
            );
        }
        let early = format!(
            "{}  devlanes-gui-{version}-{TARGET}{ARCHIVE_SUFFIX}\n{sums}",
            "c".repeat(64)
        );
        let error = parse_sums(&early, TARGET, None, "SUMS").expect_err("a gui- version");
        assert_eq!(
            error.to_string(),
            format!("unexpected version gui-{version} in SUMS")
        );
    }

    #[test]
    fn sums_refuse_what_the_scripts_refuse() {
        let sha = "a".repeat(64);
        let line = |sha: &str, version: &str| {
            format!("{sha}  devlanes-{version}-{TARGET}{ARCHIVE_SUFFIX}\n")
        };
        for (sums, message) in [
            (line(&sha, "0.9"), "unexpected version 0.9 in SUMS"),
            (line(&sha, "0.9.1-"), "unexpected version 0.9.1- in SUMS"),
            (line(&sha, "0.9.1+x"), "unexpected version 0.9.1+x in SUMS"),
            (line(&"A".repeat(64), "0.9.1"), "malformed checksum for"),
            (line(&"a".repeat(63), "0.9.1"), "malformed checksum for"),
            (String::new(), "no "),
            (format!("{sha}  devlanes-0.9.1-other-target.zip\n"), "no "),
        ] {
            let error = parse_sums(&sums, TARGET, None, "SUMS").expect_err(&sums);
            assert!(error.to_string().starts_with(message), "{error}");
        }
    }

    #[test]
    fn archives_hold_exactly_the_release_files() {
        let top = format!("devlanes-0.9.1-{TARGET}");
        let mut listing = format!("{top}/\n{top}/LICENSE\n");
        for name in BINARIES {
            listing.push_str(&format!("{top}/{name}\n"));
        }
        assert!(exact_entries(&listing, &top));
        assert!(exact_entries(
            &listing.replace(&format!("{top}/\n"), ""),
            &top
        ));
        assert!(!exact_entries(&format!("{listing}{top}/extra\n"), &top));
        assert!(!exact_entries(&format!("{listing}../evil\n"), &top));
        assert!(!exact_entries(&listing.replace("LICENSE", "README"), &top));
        assert!(!exact_entries(
            &listing.replace(&top, "devlanes-0.9.0"),
            &top
        ));
    }

    #[test]
    fn tails_keep_the_last_lines() {
        let text: String = (1..=20).map(|n| format!("line {n}\n\n")).collect();
        let kept = tail(&text);
        assert_eq!(kept.lines().count(), TAIL_LINES);
        assert!(
            kept.starts_with("line 9\n") && kept.ends_with("line 20"),
            "{kept}"
        );
    }

    /// Only a newer release installs: a `next` build moves to a later `next`
    /// build, and never back to an older release.
    #[test]
    fn older_releases_never_install() {
        assert_eq!(target("0.9.1", "0.9.0"), Target::Older);
        assert_eq!(target("0.9.1", "0.9.1-next.3"), Target::Older);
        assert_eq!(target("0.9.1", "v0.9.1"), Target::Current);
        assert_eq!(
            target("0.9.1-next.14", "0.9.1-next.20"),
            Target::Install {
                version: "0.9.1-next.20".into()
            }
        );
        assert_eq!(target("0.9.1-next.14", "0.9.0"), Target::Older);
    }

    /// A `next` build has no tag: it comes from the `next` prerelease.
    #[test]
    fn next_builds_come_from_the_next_prerelease() {
        assert_eq!(Release::of("0.9.1-next.14"), Release::Next);
        assert_eq!(Release::of("0.9.1"), Release::Version("0.9.1".into()));
        assert_eq!(Release::of("0.9.1").folder(), "download/v0.9.1");
        assert_eq!(Release::Next.folder(), "download/next");
    }

    #[test]
    fn versions_are_checked_before_any_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = InstallMethod::Script(dir.path().to_path_buf());
        for version in ["1.0.0; rm -rf /", "v0.9.1", "0.9.1+x", " 0.9.1", "0.9.1\n"] {
            for method in [&InstallMethod::Npm, &script] {
                let error =
                    Plan::new(method, Path::new("devlanes"), version, true).expect_err("refused");
                assert!(
                    matches!(error, InstallError::Version(_)),
                    "{version:?}: {error}"
                );
            }
        }
        assert!(Plan::new(&script, Path::new("devlanes"), "0.9.1-next.3", true).is_ok());
    }

    /// A record a daemon from before devlanes-gui wrote cannot say whether
    /// the release has a GUI archive, so it is not applied; the release is
    /// staged again.
    #[test]
    fn staged_records_without_the_gui_field_are_ignored() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let (home, files) = (root.path().join("home"), root.path().join("files"));
        std::fs::create_dir_all(update_dir(&home))?;
        std::fs::create_dir_all(&files)?;
        for name in BINARIES {
            std::fs::write(files.join(name), "new")?;
        }
        let old = serde_json::json!({
            "version": "0.9.1",
            "sha256": "a".repeat(64),
            "target": root.path().join("bin"),
            "files": files,
        });
        let record = update_dir(&home).join("staged.json");
        std::fs::write(&record, old.to_string())?;
        assert_eq!(Staged::load(&home), None);
        let mut new = old;
        new["gui"] = serde_json::json!(false);
        std::fs::write(&record, new.to_string())?;
        assert_eq!(
            Staged::load(&home).map(|staged| staged.gui),
            Some(Some(false))
        );
        Ok(())
    }

    /// The names in `dir`, sorted.
    fn listing(dir: &Path) -> std::io::Result<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(dir)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()?;
        names.sort();
        Ok(names)
    }

    /// The binaries of a release with a GUI archive change together: when
    /// `devlanes-gui` cannot take its place, the helper swapped before it
    /// gets its old file back, `devlanes` is untouched, and nothing of the
    /// attempt stays in the directory. Unix only, where a directory refuses
    /// the rename.
    #[cfg(unix)]
    #[test]
    fn applying_with_the_gui_is_all_or_nothing() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let (home, files, target) = (
            root.path().join("home"),
            root.path().join("files"),
            root.path().join("bin"),
        );
        std::fs::create_dir_all(&files)?;
        std::fs::create_dir_all(&target)?;
        for name in BINARIES.iter().chain([&GUI_BINARY]) {
            std::fs::write(files.join(name), format!("new {name}"))?;
        }
        std::fs::write(target.join("devlanes"), "old devlanes")?;
        std::fs::write(target.join("devlanes-hosts"), "old helper")?;
        std::fs::create_dir_all(target.join("devlanes-gui/busy"))?;
        let staged = Staged {
            version: "0.9.1".into(),
            sha256: "a".repeat(64),
            target: target.clone(),
            files,
            gui: Some(true),
        };
        let error = apply(&home, &staged).expect_err("devlanes-gui cannot be replaced");
        assert!(error.to_string().contains("devlanes-gui"), "{error}");
        assert_eq!(
            std::fs::read_to_string(target.join("devlanes"))?,
            "old devlanes"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("devlanes-hosts"))?,
            "old helper"
        );
        assert_eq!(
            listing(&target)?,
            ["devlanes", "devlanes-gui", "devlanes-hosts"]
        );
        Ok(())
    }

    /// On Windows a `devlanes-gui.exe` another process holds without sharing
    /// delete cannot be set aside: the update fails and `devlanes.exe` keeps
    /// its old file, with nothing of the attempt left.
    #[cfg(windows)]
    #[test]
    fn applying_with_a_held_gui_keeps_the_old_devlanes() -> std::io::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir()?;
        let (home, files, target) = (
            root.path().join("home"),
            root.path().join("files"),
            root.path().join("bin"),
        );
        std::fs::create_dir_all(&files)?;
        std::fs::create_dir_all(&target)?;
        for name in BINARIES.iter().chain([&GUI_BINARY]) {
            std::fs::write(files.join(name), format!("new {name}"))?;
        }
        std::fs::write(target.join("devlanes.exe"), "old devlanes")?;
        let gui = target.join(GUI_BINARY);
        std::fs::write(&gui, "old gui")?;
        // FILE_SHARE_READ only, for the whole update.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&gui)?;
        let staged = Staged {
            version: "0.9.1".into(),
            sha256: "a".repeat(64),
            target: target.clone(),
            files,
            gui: Some(true),
        };
        let error = apply(&home, &staged).expect_err("devlanes-gui.exe is held");
        drop(held);
        assert!(error.to_string().contains("devlanes-gui"), "{error}");
        assert_eq!(
            std::fs::read_to_string(target.join("devlanes.exe"))?,
            "old devlanes"
        );
        assert_eq!(std::fs::read_to_string(&gui)?, "old gui");
        assert_eq!(listing(&target)?, ["devlanes-gui.exe", "devlanes.exe"]);
        Ok(())
    }

    /// `devlanes-hosts` is swapped before `devlanes`: when it cannot take its
    /// place, `devlanes` is untouched, and nothing of the attempt stays in the
    /// directory.
    #[cfg(unix)]
    #[test]
    fn applying_is_all_or_nothing() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let (home, files, target) = (
            root.path().join("home"),
            root.path().join("files"),
            root.path().join("bin"),
        );
        std::fs::create_dir_all(&files)?;
        std::fs::create_dir_all(&target)?;
        for name in BINARIES {
            std::fs::write(files.join(name), format!("new {name}"))?;
        }
        std::fs::write(target.join("devlanes"), "old devlanes")?;
        // A directory in the helper's place refuses the rename.
        std::fs::create_dir_all(target.join("devlanes-hosts/busy"))?;
        let staged = Staged {
            version: "0.9.1".into(),
            sha256: "a".repeat(64),
            target: target.clone(),
            files: files.clone(),
            gui: Some(false),
        };
        let error = apply(&home, &staged).expect_err("the helper cannot be replaced");
        assert!(error.to_string().contains("devlanes-hosts"), "{error}");
        assert_eq!(
            std::fs::read_to_string(target.join("devlanes"))?,
            "old devlanes"
        );
        assert_eq!(listing(&target)?, ["devlanes", "devlanes-hosts"]);

        std::fs::remove_dir_all(target.join("devlanes-hosts"))?;
        std::fs::write(target.join("devlanes-hosts"), "old helper")?;
        let applied = apply(&home, &staged).expect("applied");
        assert!(applied.in_use.is_empty(), "{applied:?}");
        for name in BINARIES {
            assert_eq!(
                std::fs::read_to_string(target.join(name))?,
                format!("new {name}")
            );
        }
        assert_eq!(listing(&target)?, ["devlanes", "devlanes-hosts"]);
        Ok(())
    }

    /// A process that holds `devlanes.exe` without sharing delete for a
    /// moment, as CI runners have done, delays the install instead of
    /// failing it with a sharing violation.
    #[cfg(windows)]
    #[test]
    fn applying_rides_out_a_brief_hold_on_the_binary() -> std::io::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir()?;
        let (home, files, target) = (
            root.path().join("home"),
            root.path().join("files"),
            root.path().join("bin"),
        );
        std::fs::create_dir_all(&files)?;
        std::fs::create_dir_all(&target)?;
        std::fs::write(files.join("devlanes.exe"), "new")?;
        let entry = target.join("devlanes.exe");
        std::fs::write(&entry, "old")?;
        // FILE_SHARE_READ only.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&entry)?;
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(held);
        });
        let staged = Staged {
            version: "0.9.1".into(),
            sha256: "a".repeat(64),
            target: target.clone(),
            files,
            gui: Some(false),
        };
        let applied = apply(&home, &staged);
        let _ = holder.join();
        assert!(applied.is_ok(), "{applied:?}");
        assert_eq!(std::fs::read_to_string(&entry)?, "new");
        Ok(())
    }

    /// Windows refuses to replace a running executable without a second
    /// link to it, so without one a package-manager install is manual and
    /// says to stop the daemon first. macOS and Linux replace it either way.
    #[test]
    fn package_managers_need_the_running_exe_retained_on_windows() {
        let unretained =
            Plan::new(&InstallMethod::Npm, Path::new("devlanes"), "0.9.1", false).expect("plan");
        if cfg!(windows) {
            assert_eq!(
                unretained,
                Plan::Manual {
                    command: "devlanes daemon stop, then npm install -g devlanes@latest".into()
                }
            );
        } else {
            assert!(
                matches!(unretained, Plan::PackageManager(_)),
                "{unretained:?}"
            );
        }
        assert!(matches!(
            Plan::new(&InstallMethod::Npm, Path::new("devlanes"), "0.9.1", true),
            Ok(Plan::PackageManager(_))
        ));
        // Script installs rename the running executable aside instead.
        let dir = tempfile::tempdir().expect("tempdir");
        let script = InstallMethod::Script(dir.path().to_path_buf());
        assert!(matches!(
            Plan::new(&script, Path::new("devlanes"), "0.9.1", false),
            Ok(Plan::Archive { .. })
        ));
    }

    #[test]
    fn manual_methods_name_their_command() {
        for method in [
            InstallMethod::CargoInstall,
            InstallMethod::CargoBinstall,
            InstallMethod::Uvx,
            InstallMethod::PipxRun,
            InstallMethod::Source,
            InstallMethod::Local {
                manifest: PathBuf::from("package.json"),
            },
        ] {
            assert_eq!(
                Plan::new(&method, Path::new("devlanes"), "0.9.1", true).expect("plan"),
                Plan::Manual {
                    command: method.upgrade_command("0.9.1")
                },
                "{method:?}"
            );
            assert!(!installs_itself(&method), "{method:?}");
        }
        let missing = InstallMethod::Script(PathBuf::from("/nonexistent/devlanes/bin"));
        let Ok(Plan::Manual { command }) =
            Plan::new(&missing, Path::new("devlanes"), "0.9.1", true)
        else {
            panic!("an unwritable directory is manual");
        };
        // install.sh may then use sudo there.
        #[cfg(unix)]
        assert_eq!(
            command,
            "curl -fsSL https://devlanes.run/install.sh | DEVLANES_INSTALL_DIR=/nonexistent/devlanes/bin DEVLANES_INSTALL_SUDO=1 sh"
        );
        #[cfg(windows)]
        assert_eq!(command, missing.upgrade_command("0.9.1"));
        assert!(installs_itself(&missing));
        let dir = tempfile::tempdir().expect("tempdir");
        let script = InstallMethod::Script(dir.path().to_path_buf());
        assert_eq!(
            Plan::new(&script, Path::new("devlanes"), "0.9.1", true).expect("plan"),
            Plan::Archive {
                dir: dir.path().to_path_buf()
            }
        );
    }

    /// Displayed commands, which errors and tests compare.
    fn shown(method: &InstallMethod, exe: &str) -> Vec<String> {
        package_manager(method, "0.9.1", Path::new(exe))
            .expect("a package manager")
            .iter()
            .map(|argv| format!("{} | {}", argv.program.display(), argv.args.join(" ")))
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn package_manager_commands() {
        assert_eq!(
            shown(
                &InstallMethod::Npm,
                "/Users/a/.nvm/versions/node/v24.1.0/lib/node_modules/devlanes/node_modules/@devlanes/darwin-arm64/bin/devlanes"
            ),
            ["/Users/a/.nvm/versions/node/v24.1.0/bin/npm | install -g devlanes@0.9.1"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Pnpm,
                "/Users/a/Library/pnpm/global/5/node_modules/.pnpm/x/devlanes"
            ),
            ["pnpm | add -g devlanes@0.9.1"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Bun,
                "/Users/a/.bun/install/global/node_modules/devlanes/bin/devlanes"
            ),
            ["/Users/a/.bun/bin/bun | add -g devlanes@0.9.1"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Yarn,
                "/Users/a/.config/yarn/global/node_modules/devlanes/bin/devlanes"
            ),
            ["yarn | global add devlanes@0.9.1"]
        );
        assert_eq!(
            shown(&InstallMethod::Uv, "/Users/a/.local/bin/devlanes"),
            ["uv | tool upgrade devlanes"]
        );
        assert_eq!(
            shown(&InstallMethod::Pipx, "/Users/a/.local/bin/devlanes"),
            ["pipx | upgrade devlanes"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Homebrew,
                "/opt/homebrew/Cellar/devlanes/0.9.0/bin/devlanes"
            ),
            [
                "/opt/homebrew/bin/brew | update",
                "/opt/homebrew/bin/brew | upgrade devlanes-run/tap/devlanes"
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn package_manager_commands() {
        assert_eq!(
            shown(
                &InstallMethod::Npm,
                r"C:\Users\a\AppData\Roaming\npm\node_modules\devlanes\node_modules\@devlanes\win32-x64\bin\devlanes.exe"
            ),
            [r"C:\Users\a\AppData\Roaming\npm\npm.cmd | install -g devlanes@0.9.1"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Pnpm,
                r"C:\Users\a\AppData\Local\pnpm\global\5\x\devlanes.exe"
            ),
            ["pnpm.cmd | add -g devlanes@0.9.1"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Bun,
                r"C:\Users\a\.bun\install\global\node_modules\devlanes\bin\devlanes.exe"
            ),
            [r"C:\Users\a\.bun\bin\bun.exe | add -g devlanes@0.9.1"]
        );
        assert_eq!(
            shown(
                &InstallMethod::Yarn,
                r"C:\Users\a\AppData\Local\Yarn\Data\global\devlanes.exe"
            ),
            ["yarn.cmd | global add devlanes@0.9.1"]
        );
        assert_eq!(
            shown(&InstallMethod::Uv, r"C:\Users\a\.local\bin\devlanes.exe"),
            ["uv.exe | tool upgrade devlanes"]
        );
        assert_eq!(
            shown(&InstallMethod::Pipx, r"C:\Users\a\.local\bin\devlanes.exe"),
            ["pipx.exe | upgrade devlanes"]
        );
    }

    /// Removal commands as `program | args`; tool paths derived from these
    /// made-up prefixes do not exist, so they fall back to `PATH`.
    fn removal(method: &InstallMethod, exe: &str) -> Option<Vec<String>> {
        removal_commands(method, Path::new(exe)).map(|commands| {
            commands
                .iter()
                .map(|argv| format!("{} | {}", argv.program.display(), argv.args.join(" ")))
                .collect()
        })
    }

    #[cfg(unix)]
    #[test]
    fn removal_commands_per_method() {
        let exe = "/Users/a/.local/bin/devlanes";
        for (method, expected) in [
            (InstallMethod::Npm, "npm | uninstall -g devlanes"),
            (InstallMethod::Pnpm, "pnpm | remove -g devlanes"),
            (InstallMethod::Bun, "bun | remove -g devlanes"),
            (InstallMethod::Yarn, "yarn | global remove devlanes"),
            (InstallMethod::Uv, "uv | tool uninstall devlanes"),
            (InstallMethod::Pipx, "pipx | uninstall devlanes"),
            (
                InstallMethod::Homebrew,
                "brew | uninstall devlanes-run/tap/devlanes",
            ),
            (
                InstallMethod::CargoInstall,
                "cargo | uninstall devlanes devlanes-hosts-helper",
            ),
            (
                InstallMethod::CargoBinstall,
                "cargo | uninstall devlanes devlanes-hosts-helper",
            ),
            (
                InstallMethod::Pip(PathBuf::from("/srv/acme/.venv")),
                "/srv/acme/.venv/bin/python | -m pip uninstall -y devlanes",
            ),
            (
                InstallMethod::UvPip(PathBuf::from("/srv/acme/.venv")),
                "uv | pip uninstall --python /srv/acme/.venv devlanes",
            ),
        ] {
            assert_eq!(
                removal(&method, exe),
                Some(vec![expected.to_owned()]),
                "{method:?}"
            );
        }
        for method in [
            InstallMethod::Uvx,
            InstallMethod::PipxRun,
            InstallMethod::Source,
            InstallMethod::Script(PathBuf::from("/Users/a/.local/bin")),
            InstallMethod::Local {
                manifest: PathBuf::from("package.json"),
            },
        ] {
            assert_eq!(removal(&method, exe), None, "{method:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn removal_commands_per_method() {
        let exe = r"C:\Users\a\.local\bin\devlanes.exe";
        for (method, expected) in [
            (InstallMethod::Npm, "npm.cmd | uninstall -g devlanes"),
            (InstallMethod::Pnpm, "pnpm.cmd | remove -g devlanes"),
            (InstallMethod::Bun, "bun.exe | remove -g devlanes"),
            (InstallMethod::Yarn, "yarn.cmd | global remove devlanes"),
            (InstallMethod::Uv, "uv.exe | tool uninstall devlanes"),
            (InstallMethod::Pipx, "pipx.exe | uninstall devlanes"),
            (
                InstallMethod::CargoInstall,
                "cargo.exe | uninstall devlanes",
            ),
            (
                InstallMethod::Pip(PathBuf::from(r"D:\acme\.venv")),
                r"D:\acme\.venv\Scripts\python.exe | -m pip uninstall -y devlanes",
            ),
        ] {
            assert_eq!(
                removal(&method, exe),
                Some(vec![expected.to_owned()]),
                "{method:?}"
            );
        }
        assert_eq!(
            removal(&InstallMethod::Script(PathBuf::from(r"C:\tools")), exe),
            None
        );
    }

    /// cargo fails an uninstall that names a package it never installed, so
    /// devlanes-gui is named only when its binary sits next to devlanes.
    #[test]
    fn cargo_removes_devlanes_gui_only_when_installed() -> io::Result<()> {
        let bin = tempfile::tempdir()?;
        let exe = bin.path().join(if cfg!(windows) {
            "devlanes.exe"
        } else {
            "devlanes"
        });
        let names = || -> Vec<String> {
            removal_commands(&InstallMethod::CargoInstall, &exe)
                .and_then(|commands| commands.into_iter().next())
                .map(|argv| argv.args)
                .unwrap_or_default()
        };
        assert!(
            !names().contains(&"devlanes-gui".to_owned()),
            "{:?}",
            names()
        );
        std::fs::write(bin.path().join(GUI_BINARY), "gui")?;
        assert_eq!(names().last().map(String::as_str), Some("devlanes-gui"));
        Ok(())
    }

    #[test]
    fn displayed_commands_name_the_program_only() {
        let argv = Argv {
            program: PathBuf::from("/opt/homebrew/bin/brew"),
            args: vec!["update".into()],
        };
        assert_eq!(argv.display(), "brew update");
    }
}
