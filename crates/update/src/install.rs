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
    version::Version,
};

/// The target triple of this build, which names its release archive.
pub const TARGET: &str = env!("DEVLANES_TARGET");

/// The archive format of this OS's releases.
#[cfg(unix)]
const ARCHIVE_SUFFIX: &str = ".tar.gz";
#[cfg(windows)]
const ARCHIVE_SUFFIX: &str = ".zip";

/// The executables a release archive holds next to `LICENSE`, each installed
/// into the install's directory.
#[cfg(unix)]
pub const BINARIES: &[&str] = &["devlanes", "devlanes-hosts"];
#[cfg(windows)]
pub const BINARIES: &[&str] = &["devlanes.exe"];

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
    pub program: PathBuf,
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
    /// names one, else from `PATH`. Blocking filesystem work.
    pub fn new(method: &InstallMethod, exe: &Path, version: &str) -> Result<Self, InstallError> {
        if Version::parse(version).is_none() {
            return Err(InstallError::Version(version.to_owned()));
        }
        if let InstallMethod::Script(dir) = method {
            return Ok(if writable(dir) {
                Self::Archive { dir: dir.clone() }
            } else {
                Self::Manual {
                    command: method.upgrade_command(version),
                }
            });
        }
        Ok(match package_manager(method, version, exe) {
            Some(commands) => Self::PackageManager(on_disk_or_path(commands)),
            None => Self::Manual {
                command: method.upgrade_command(version),
            },
        })
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
    /// The word `staged.json` records: `stable` or `next`.
    fn channel(&self) -> &'static str {
        match self {
            Self::Version(version) => check::Channel::of_version(version).as_str(),
            Self::Next => "next",
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

/// Whether `tar`'s listing of an archive holds exactly the entries a release
/// archive has below `top`: `LICENSE` and [`BINARIES`].
pub fn exact_entries(listing: &str, top: &str) -> bool {
    let mut found: Vec<&str> = listing
        .lines()
        .map(|line| line.trim_end_matches('\r').trim_end_matches('/'))
        .filter(|line| !line.is_empty() && *line != top)
        .collect();
    found.sort_unstable();
    let mut expected: Vec<String> = std::iter::once("LICENSE")
        .chain(BINARIES.iter().copied())
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
    /// `stable` or `next`.
    pub channel: String,
    /// The archive's SHA-256.
    pub sha256: String,
    /// The install's directory, where [`apply`] puts the binaries.
    pub target: PathBuf,
    /// The unpacked archive's directory, holding [`BINARIES`].
    pub files: PathBuf,
}

impl Staged {
    /// The staged release under `home`, when its record reads and its files
    /// are all there. Blocking.
    pub fn load(home: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(update_dir(home).join("staged.json")).ok()?;
        let staged: Self = serde_json::from_str(&text).ok()?;
        BINARIES
            .iter()
            .all(|name| staged.files.join(name).is_file())
            .then_some(staged)
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
    let entry = parse_sums(&String::from_utf8_lossy(&sums), TARGET, expect, &sums_url)?;
    let home = home.to_path_buf();
    let loaded = {
        let home = home.clone();
        blocking(move || Ok(Staged::load(&home))).await?
    };
    if let Some(staged) = loaded
        && staged.version == entry.version
        && staged.sha256 == entry.sha256
        && staged.target == dir
    {
        return Ok(staged);
    }
    let folder = update_dir(&home).join(&entry.version);
    let archive = folder.join(&entry.archive);
    {
        let folder = folder.clone();
        blocking(move || {
            std::fs::create_dir_all(&folder).map_err(InstallError::io(folder.display().to_string()))
        })
        .await?;
    }
    let url = format!("{base}/{}/{}", release.folder(), entry.archive);
    check::download(
        &url,
        &archive,
        ARCHIVE_TIMEOUT.max(timeout),
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
    let top = format!("devlanes-{}-{TARGET}", entry.version);
    let listing = tar(&[os::TAR_LIST, path_arg(&archive)?]).await?;
    if !exact_entries(&listing, &top) {
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
        path_arg(&folder)?,
    ])
    .await?;
    let files = folder.join(&top);
    let reported = reported_version(&files.join(BINARIES[0])).await;
    if reported.as_deref() != Some(entry.version.as_str()) {
        return Err(InstallError::Release(format!(
            "the {} archive's devlanes reports {}",
            entry.version,
            reported.as_deref().unwrap_or("no version")
        )));
    }
    let staged = Staged {
        version: entry.version,
        channel: release.channel().to_owned(),
        sha256: entry.sha256,
        target: dir.to_path_buf(),
        files,
    };
    let record = staged.clone();
    blocking(move || {
        let _ = std::fs::remove_file(&archive);
        write_record(&home, &record)?;
        prune(&home, &record.version);
        Ok(())
    })
    .await?;
    Ok(staged)
}

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Windows: replaced executables that still run (the daemon's, this
    /// process's), renamed to `devlanes.exe.old-<stamp>` and deleted later
    /// by `doctor` or the next update.
    pub in_use: Vec<PathBuf>,
}

/// Puts the binaries of `staged` into its target directory: each is copied
/// next to its target as `.<name>.new.<pid>`, made executable and renamed
/// into place, so a running devlanes keeps its open executable. On Windows
/// the running executable is renamed to `<name>.old-<stamp>` first, as
/// `install.ps1` does, and put back when the new one cannot take its place.
/// The staged files are deleted afterwards. Hold [`Lock`]. Blocking.
pub fn apply(home: &Path, staged: &Staged) -> Result<Applied, InstallError> {
    let mut applied = Applied::default();
    for name in BINARIES {
        let target = staged.target.join(name);
        let temporary = staged
            .target
            .join(format!(".{name}.new.{}", std::process::id()));
        let placed = std::fs::copy(staged.files.join(name), &temporary)
            .and_then(|_| os::make_executable(&temporary))
            .and_then(|()| os::replace(&temporary, &target));
        match placed {
            Ok(Some(old)) => applied.in_use.push(old),
            Ok(None) => {}
            Err(source) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(InstallError::Io {
                    what: format!("cannot install {}", target.display()),
                    source,
                });
            }
        }
    }
    clear(home);
    Ok(applied)
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
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(InstallError::io(path.display().to_string()))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The version `<exe> --version` prints after `devlanes `, or `None` when it
/// does not run or prints something else.
pub async fn reported_version(exe: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        TOOL_TIMEOUT,
        tokio::process::Command::new(exe)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.trim().strip_prefix("devlanes ")?;
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

    /// Mode 755, as `install.sh` sets.
    pub(super) fn make_executable(path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }

    /// Renames `new` over `target`; a running process keeps the old file.
    pub(super) fn replace(new: &Path, target: &Path) -> io::Result<Option<PathBuf>> {
        std::fs::rename(new, target).map(|()| None)
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

    /// Windows has no mode bits.
    pub(super) fn make_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// Renames `target` to `<target>.old-<stamp>` (a running executable can
    /// be renamed, not replaced), then `new` to `target`, putting the old one
    /// back when that fails. The old file is deleted when nothing runs it;
    /// otherwise its path is returned.
    pub(super) fn replace(new: &Path, target: &Path) -> io::Result<Option<PathBuf>> {
        let old = if target.exists() {
            let stamp = devlanes_core::clock::utc_stamp(crate::now_ms() / 1000);
            let mut old = PathBuf::from(format!("{}.old-{stamp}", target.display()));
            let mut n = 1;
            while old.exists() {
                old = PathBuf::from(format!("{}.old-{stamp}-{n}", target.display()));
                n += 1;
            }
            std::fs::rename(target, &old)?;
            Some(old)
        } else {
            None
        };
        if let Err(error) = std::fs::rename(new, target) {
            if let Some(old) = &old {
                let _ = std::fs::rename(old, target);
            }
            return Err(error);
        }
        Ok(old.filter(|old| std::fs::remove_file(old).is_err()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn versions_are_checked_before_any_command() {
        let error = Plan::new(
            &InstallMethod::Npm,
            Path::new("devlanes"),
            "1.0.0; rm -rf /",
        )
        .expect_err("refused");
        assert!(matches!(error, InstallError::Version(_)), "{error}");
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
                Plan::new(&method, Path::new("devlanes"), "0.9.1").expect("plan"),
                Plan::Manual {
                    command: method.upgrade_command("0.9.1")
                },
                "{method:?}"
            );
            assert!(!installs_itself(&method), "{method:?}");
        }
        let missing = InstallMethod::Script(PathBuf::from("/nonexistent/devlanes/bin"));
        assert!(matches!(
            Plan::new(&missing, Path::new("devlanes"), "0.9.1"),
            Ok(Plan::Manual { .. })
        ));
        assert!(installs_itself(&missing));
        let dir = tempfile::tempdir().expect("tempdir");
        let script = InstallMethod::Script(dir.path().to_path_buf());
        assert_eq!(
            Plan::new(&script, Path::new("devlanes"), "0.9.1").expect("plan"),
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

    #[test]
    fn displayed_commands_name_the_program_only() {
        let argv = Argv {
            program: PathBuf::from("/opt/homebrew/bin/brew"),
            args: vec!["update".into()],
        };
        assert_eq!(argv.display(), "brew update");
    }
}
