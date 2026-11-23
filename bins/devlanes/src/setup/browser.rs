//! The `browser` step: the native messaging host manifest that lets the
//! devlanes browser extension start this devlanes, for Chrome, Chromium,
//! Brave and Edge.
//!
//! On macOS and Linux each browser reads `NativeMessagingHosts/` in its own
//! folder, so the manifest goes into the folder of each browser that exists.
//! On Windows the browsers read a registry key that names the manifest, so one
//! manifest goes under `%USERPROFILE%\.devlanes\browser\` and all four keys
//! point at it, whatever is installed. Playwright's Chromium reads the
//! Chromium key without any Chromium folder, which is why Windows doesn't
//! look for the browsers first.
//!
//! The step never counts as missing. `setup` applies it with the other steps,
//! `--check` only reports it, and `--remove` deletes only what runs this
//! install.

use std::{
    io,
    path::{Path, PathBuf},
};

use devlanes_browser_host::HOST_NAME;
use devlanes_cli::{Rendered, Runtime};
use devlanes_core::paths::{same_path, user_home};
use devlanes_proto::RpcError;
use devlanes_update::InstallMethod;
use serde_json::{Value, json};

use crate::output::emit;

/// A browser the step registers the host for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Browser {
    Chrome,
    Chromium,
    Brave,
    Edge,
}

impl Browser {
    /// Every browser, in the order setup names them.
    pub(crate) const ALL: [Self; 4] = [Self::Chrome, Self::Chromium, Self::Brave, Self::Edge];

    /// The name setup and doctor print.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Chromium => "Chromium",
            Self::Brave => "Brave",
            Self::Edge => "Edge",
        }
    }

    /// The browser's own folder under the home, on macOS and Linux.
    fn folder(self, layout: Layout) -> PathBuf {
        let parts: &[&str] = match (layout, self) {
            (Layout::MacOs, Self::Chrome) => {
                &["Library", "Application Support", "Google", "Chrome"]
            }
            (Layout::MacOs, Self::Chromium) => &["Library", "Application Support", "Chromium"],
            (Layout::MacOs, Self::Brave) => &[
                "Library",
                "Application Support",
                "BraveSoftware",
                "Brave-Browser",
            ],
            (Layout::MacOs, Self::Edge) => &["Library", "Application Support", "Microsoft Edge"],
            (_, Self::Chrome) => &[".config", "google-chrome"],
            (_, Self::Chromium) => &[".config", "chromium"],
            (_, Self::Brave) => &[".config", "BraveSoftware", "Brave-Browser"],
            (_, Self::Edge) => &[".config", "microsoft-edge"],
        };
        parts.iter().collect()
    }

    /// The browser's key under the registry root, on Windows.
    fn key(self) -> &'static str {
        match self {
            Self::Chrome => r"Google\Chrome",
            Self::Chromium => "Chromium",
            Self::Brave => r"BraveSoftware\Brave-Browser",
            Self::Edge => r"Microsoft\Edge",
        }
    }
}

/// Where the browsers look for native messaging hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// A manifest in each browser's folder under `~/Library/Application Support`.
    MacOs,
    /// A manifest in each browser's folder under `~/.config`.
    Linux,
    /// One manifest, named by a key per browser under `HKCU`.
    Windows,
}

impl Layout {
    /// This OS's layout.
    fn current() -> Self {
        // cfg! keeps every layout compiled on every OS, so the tests of the
        // macOS and Linux layouts run everywhere.
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// Where the step reads and writes. Tests pass a temporary home and a test
/// key, so they never touch the real browser folders or `HKCU\Software`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Location {
    /// The user's home folder.
    pub(crate) home: PathBuf,
    pub(crate) layout: Layout,
    /// The `HKCU` key the Windows browser keys sit under, `Software`.
    pub(crate) registry_root: String,
}

impl Location {
    /// This user's home and this OS's layout; `None` when the home is unknown.
    pub(crate) fn current() -> Option<Self> {
        user_home().map(|home| Self {
            home,
            layout: Layout::current(),
            registry_root: "Software".into(),
        })
    }

    /// The one manifest of the Windows layout.
    fn windows_manifest(&self) -> PathBuf {
        self.home
            .join(".devlanes")
            .join("browser")
            .join(manifest_name())
    }

    /// The registry key, under `HKCU`, that points `browser` at a manifest.
    fn key(&self, browser: Browser) -> String {
        format!(
            r"{}\{}\NativeMessagingHosts\{HOST_NAME}",
            self.registry_root,
            browser.key()
        )
    }

    /// The manifest `browser` reads on macOS and Linux.
    fn unix_manifest(&self, browser: Browser) -> PathBuf {
        self.home
            .join(browser.folder(self.layout))
            .join("NativeMessagingHosts")
            .join(manifest_name())
    }
}

/// `run.devlanes.browser.json`.
fn manifest_name() -> String {
    format!("{HOST_NAME}.json")
}

/// The devlanes install the manifest names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Install {
    method: InstallMethod,
    /// The path that runs this install after an upgrade.
    entry: PathBuf,
}

impl Install {
    /// An install of `method` whose stable path is `entry`.
    #[cfg(test)]
    pub(crate) fn new(method: InstallMethod, entry: PathBuf) -> Self {
        Self { method, entry }
    }

    /// The running install, from [`InstallMethod::current_with_entry`].
    /// Blocking, since it reads the filesystem.
    pub(crate) fn current() -> Self {
        let (method, entry) = InstallMethod::current_with_entry();
        Self { method, entry }
    }

    /// Why this install can't be registered: uvx and pipx run devlanes from a
    /// cache, which they may clear or replace at any time, and a relative
    /// entry means devlanes couldn't find its own binary.
    fn refusal(&self) -> Option<String> {
        if !self.entry.is_absolute() {
            return Some(format!(
                "devlanes can't find the path of its own binary, only {}, so the browser extension couldn't start it. Run devlanes setup again by its full path",
                self.entry.display()
            ));
        }
        let (runner, install) = match self.method {
            InstallMethod::Uvx => ("uvx", "uv tool install devlanes"),
            InstallMethod::PipxRun => ("pipx run", "pipx install devlanes"),
            _ => return None,
        };
        Some(format!(
            "{runner} runs devlanes from a cache, so the browser extension can't start it. Install devlanes with {install}, then run devlanes setup again"
        ))
    }

    /// Whether this is a `Source` or `Local` build, whose binary may move or
    /// go away.
    fn is_build(&self) -> bool {
        matches!(
            self.method,
            InstallMethod::Source | InstallMethod::Local { .. }
        )
    }

    /// A note for `Source` and `Local` builds, whose binary may move or go
    /// away.
    fn note(&self) -> Option<String> {
        self.is_build().then(|| {
            format!(
                "the host runs this build at {}. Run devlanes setup again if you move or delete it",
                self.entry.display()
            )
        })
    }

    /// The manifest that registers this install.
    fn manifest(&self) -> Value {
        json!({
            "name": HOST_NAME,
            "description": "devlanes browser extension host",
            "path": self.entry.to_string_lossy(),
            "type": "stdio",
            "allowed_origins": devlanes_browser_host::origin::allowed_origins(),
        })
    }
}

/// What a browser has registered under the host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    /// macOS and Linux: the browser's folder doesn't exist.
    NoBrowser,
    Missing,
    /// This install's manifest, as setup writes it.
    Current,
    /// A registration setup replaces. `runs` is the program it starts,
    /// `None` when its manifest is missing. `ours` when that is this
    /// install's entry, so `--remove` deletes it.
    Stale {
        runs: Option<PathBuf>,
        ours: bool,
    },
    /// Another install's working registration, which a `Source` or `Local`
    /// build leaves alone: `runs` exists. A stable install replaces it as
    /// [`Found::Stale`], so the last setup wins.
    Kept {
        runs: PathBuf,
    },
    /// The manifest or the key can't be read.
    Unreadable(String),
}

/// One browser's registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registration {
    pub(crate) browser: Browser,
    /// The manifest file, or on Windows the registry key, as setup names it.
    pub(crate) place: String,
    pub(crate) found: Found,
}

impl Registration {
    /// Whether setup writes this registration.
    fn to_write(&self) -> bool {
        !matches!(
            self.found,
            Found::NoBrowser | Found::Current | Found::Kept { .. }
        )
    }
}

/// The browser facts setup plans from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Facts {
    /// The user's home is unknown.
    NoHome,
    /// This install can't be registered, and why.
    Refused(String),
    Found {
        install: Install,
        registrations: Vec<Registration>,
        /// Windows: the shared manifest another working install holds,
        /// which a build leaves alone.
        held: Option<Held>,
    },
}

/// The shared Windows manifest, when it runs another install whose binary
/// exists and setup runs from a `Source` or `Local` build. Every key names
/// that one file, so the build can't register any browser without taking
/// the others over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Held {
    pub(crate) manifest: PathBuf,
    /// The program the manifest starts.
    pub(crate) runs: PathBuf,
}

/// The [`Held`] shared manifest at `location` for `install`, if any. Blocking.
fn held(location: &Location, install: &Install) -> Option<Held> {
    if location.layout != Layout::Windows {
        return None;
    }
    let manifest = location.windows_manifest();
    match found_of(read_manifest(&manifest, &install.manifest()), install) {
        Found::Kept { runs } => Some(Held { manifest, runs }),
        _ => None,
    }
}

/// A manifest as read from disk.
enum Manifest {
    Missing,
    Unreadable(String),
    /// It starts `path`. `current` when it is exactly what setup writes.
    Runs {
        path: PathBuf,
        current: bool,
    },
}

/// Reads the manifest at `file` and compares it with `expected`.
fn read_manifest(file: &Path, expected: &Value) -> Manifest {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Manifest::Missing,
        Err(error) => return Manifest::Unreadable(error.to_string()),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Manifest::Unreadable(error.to_string()),
    };
    match value.get("path").and_then(Value::as_str) {
        Some(path) => Manifest::Runs {
            path: PathBuf::from(path),
            current: &value == expected,
        },
        None => Manifest::Unreadable("no path".into()),
    }
}

/// What a manifest means for a registration of `install`. Blocking, since it
/// checks whether another install's program exists.
fn found_of(manifest: Manifest, install: &Install) -> Found {
    match manifest {
        Manifest::Missing => Found::Stale {
            runs: None,
            ours: false,
        },
        Manifest::Unreadable(reason) => Found::Unreadable(reason),
        Manifest::Runs { current: true, .. } => Found::Current,
        Manifest::Runs { path, .. } => {
            let ours = same_path(&path, &install.entry);
            // A build may move or go away, so it doesn't take a working
            // registration from another install.
            if !ours && install.is_build() && path.is_file() {
                Found::Kept { runs: path }
            } else {
                Found::Stale {
                    ours,
                    runs: Some(path),
                }
            }
        }
    }
}

/// Reads every browser's registration for `install`. Blocking.
pub(crate) fn inspect(location: &Location, install: &Install) -> Vec<Registration> {
    let expected = install.manifest();
    Browser::ALL
        .into_iter()
        .map(|browser| {
            if location.layout == Layout::Windows {
                return inspect_key(location, install, &expected, browser);
            }
            let file = location.unix_manifest(browser);
            let found = if !location.home.join(browser.folder(location.layout)).is_dir() {
                Found::NoBrowser
            } else {
                match read_manifest(&file, &expected) {
                    Manifest::Missing => Found::Missing,
                    manifest => found_of(manifest, install),
                }
            };
            Registration {
                browser,
                place: file.display().to_string(),
                found,
            }
        })
        .collect()
}

/// The Windows registration of `browser`: its key, and the manifest it names.
fn inspect_key(
    location: &Location,
    install: &Install,
    expected: &Value,
    browser: Browser,
) -> Registration {
    let key = location.key(browser);
    let found = match registry::read_string(&key, "") {
        Ok(None) => Found::Missing,
        Err(error) => Found::Unreadable(error.to_string()),
        Ok(Some(named)) => {
            let named = PathBuf::from(named);
            let own_file = same_path(&named, &location.windows_manifest());
            match found_of(read_manifest(&named, expected), install) {
                // A key that names the file setup writes, after that file
                // was deleted with `.devlanes`, is devlanes' own to clean up.
                // While the file exists, its `path` says whose it is.
                Found::Stale { runs: None, .. } if own_file => Found::Stale {
                    runs: None,
                    ours: true,
                },
                // Another manifest that runs this install still gets the
                // key pointed at the one setup keeps current.
                Found::Current if !own_file => Found::Stale {
                    runs: Some(install.entry.clone()),
                    ours: true,
                },
                // The key is the place, so the reason names the file.
                Found::Unreadable(reason) => {
                    Found::Unreadable(format!("{}: {reason}", named.display()))
                }
                found => found,
            }
        }
    };
    Registration {
        browser,
        place: format!(r"HKCU\{key}"),
        found,
    }
}

/// The step's facts for `location`, or the reason it doesn't apply. Blocking.
pub(crate) fn facts(location: Option<&Location>, install: Install) -> Facts {
    let Some(location) = location else {
        return Facts::NoHome;
    };
    if let Some(reason) = install.refusal() {
        return Facts::Refused(reason);
    }
    Facts::Found {
        registrations: inspect(location, &install),
        held: held(location, &install),
        install,
    }
}

/// `(needed, done, reason)` of the step. `needed` means there is a browser to
/// register for, and the step never counts as missing either way.
pub(crate) fn plan(facts: &Facts) -> (bool, bool, String) {
    let (install, registrations, held) = match facts {
        Facts::NoHome => {
            return (
                false,
                false,
                "devlanes can't tell where your home folder is. Set HOME (USERPROFILE on Windows), then run devlanes setup again".into(),
            );
        }
        Facts::Refused(reason) => return (false, false, reason.clone()),
        Facts::Found {
            install,
            registrations,
            held,
        } => (install, registrations, held),
    };
    let names = |keep: &dyn Fn(&Found) -> bool| {
        join(
            &registrations
                .iter()
                .filter(|registration| keep(&registration.found))
                .map(|registration| registration.browser.name())
                .collect::<Vec<_>>(),
        )
    };
    let registered = names(&|found| *found == Found::Current);
    let mut missing = names(&|found| *found == Found::Missing);
    let mut stale = names(&|found| matches!(found, Found::Stale { .. } | Found::Unreadable(_)));
    let mut kept: Vec<String> = registrations
        .iter()
        .filter_map(|registration| match &registration.found {
            Found::Kept { runs } => Some(format!(
                "{} runs {}, another devlanes install, and a build doesn't replace it",
                registration.browser.name(),
                runs.display()
            )),
            _ => None,
        })
        .collect();
    // Writing any key would rewrite the shared manifest the kept keys name.
    if let Some(held) = held {
        let blocked = names(&|found| {
            matches!(
                found,
                Found::Missing | Found::Stale { .. } | Found::Unreadable(_)
            )
        });
        (missing, stale) = (None, None);
        if let Some(blocked) = blocked {
            kept.push(format!(
                "{blocked} would need {}, which runs {}, and a build doesn't replace it",
                held.manifest.display(),
                held.runs.display()
            ));
        }
    }
    if !kept.is_empty() {
        kept.push(
            "To register this build, run devlanes setup --remove from that install first".into(),
        );
    }
    let kept = (!kept.is_empty()).then(|| kept.join(". "));
    let note = [kept.as_deref(), install.note().as_deref()]
        .into_iter()
        .flatten()
        .map(|sentence| format!(". {}", capitalise(sentence)))
        .collect::<String>();
    if registered.is_none() && missing.is_none() && stale.is_none() {
        let reason = match kept {
            Some(kept) => kept,
            None => "no Chrome, Chromium, Brave or Edge folder in your home".into(),
        };
        return (false, false, reason);
    }
    let host = "the browser extension host";
    let mut reason = match (missing, stale) {
        (None, None) => {
            let registered = registered.unwrap_or_default();
            return (
                true,
                true,
                format!("{host} is registered for {registered}{note}"),
            );
        }
        (Some(missing), None) => format!("setup registers {host} for {missing}"),
        (None, Some(stale)) => format!("setup refreshes {host} for {stale}"),
        (Some(missing), Some(stale)) => {
            format!("setup registers {host} for {missing} and refreshes it for {stale}")
        }
    };
    if let Some(registered) = registered {
        reason.push_str(&format!(", and it is registered for {registered}"));
    }
    (true, false, format!("{reason}{note}"))
}

/// `Chrome`, `Chrome and Brave`, `Chrome, Brave and Edge`; `None` for none.
fn join(names: &[&str]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some((*one).to_owned()),
        [rest @ .., last] => Some(format!("{} and {last}", rest.join(", "))),
    }
}

/// `text` with its first letter in upper case.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A failure of the browser step.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BrowserError {
    #[error("{0}")]
    Refused(String),
    #[error("couldn't write {}: {source}. Check that you can write there, then run devlanes setup again", .path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("couldn't delete {}: {source}. Delete it by hand", .path.display())]
    Delete { path: PathBuf, source: io::Error },
    #[error(r"couldn't change the registry key HKCU\{key}: {source}. Run the same devlanes setup command again")]
    Registry { key: String, source: io::Error },
}

impl BrowserError {
    /// The stable error code.
    fn code(&self) -> &'static str {
        match self {
            Self::Refused(_) => "browser_host_refused",
            Self::Write { .. } | Self::Delete { .. } | Self::Registry { .. } => {
                "browser_host_failed"
            }
        }
    }
}

impl From<BrowserError> for RpcError {
    fn from(error: BrowserError) -> Self {
        let exit = match error {
            BrowserError::Refused(_) => 2,
            _ => 1,
        };
        RpcError::new(error.code(), json!({"message": error.to_string()}), exit)
    }
}

/// Writes `manifest` to `file` through a temporary file in the same folder,
/// so a browser never reads half a manifest.
fn write_manifest(file: &Path, manifest: &Value) -> Result<(), BrowserError> {
    let failed = |source| BrowserError::Write {
        path: file.to_path_buf(),
        source,
    };
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(failed)?;
    let mut text = serde_json::to_string_pretty(manifest)
        .map_err(io::Error::from)
        .map_err(failed)?;
    text.push('\n');
    let mut temporary = tempfile::NamedTempFile::new_in(dir).map_err(failed)?;
    io::Write::write_all(temporary.as_file_mut(), text.as_bytes()).map_err(failed)?;
    // On disk before the rename, so a crash never leaves an empty manifest
    // under the real name.
    temporary.as_file_mut().sync_all().map_err(failed)?;
    temporary
        .persist(file)
        .map_err(|error| failed(error.error))?;
    Ok(())
}

/// Creates `<home>\.devlanes` private when it is missing, as the client
/// creates the devlanes home, since the Windows manifest goes inside it.
fn devlanes_folder(location: &Location) -> Result<(), BrowserError> {
    let folder = location.home.join(".devlanes");
    if folder.exists() {
        return Ok(());
    }
    // The client judges the home when it opens it, so a problem with the new
    // folder, such as a FAT volume, is its to report.
    #[cfg(windows)]
    {
        devlanes_sys_windows::security::ensure_private_dir(&folder)
            .map(drop)
            .map_err(|source| BrowserError::Write {
                path: folder.clone(),
                source,
            })
    }
    #[cfg(not(windows))]
    std::fs::create_dir_all(&folder).map_err(|source| BrowserError::Write {
        path: folder.clone(),
        source,
    })
}

/// Writes the registrations of `registrations` that aren't current, and
/// returns a line for each. Blocking.
pub(crate) fn apply(
    location: &Location,
    install: &Install,
    registrations: &[Registration],
) -> Result<Vec<String>, BrowserError> {
    if let Some(reason) = install.refusal() {
        return Err(BrowserError::Refused(reason));
    }
    let manifest = install.manifest();
    let mut lines = Vec::new();
    let todo: Vec<&Registration> = registrations.iter().filter(|r| r.to_write()).collect();
    if location.layout == Layout::Windows {
        // Every key names the shared manifest, so with another install
        // holding it a build writes neither the file nor any key.
        if todo.is_empty() || held(location, install).is_some() {
            return Ok(lines);
        }
        let file = location.windows_manifest();
        devlanes_folder(location)?;
        write_manifest(&file, &manifest)?;
        lines.push(format!("wrote {}", file.display()));
        let value = file.to_string_lossy();
        for registration in todo {
            let key = location.key(registration.browser);
            registry::write_string(&key, "", &value)
                .map_err(|source| BrowserError::Registry { key, source })?;
            lines.push(registered(registration));
        }
        return Ok(lines);
    }
    for registration in todo {
        write_manifest(&location.unix_manifest(registration.browser), &manifest)?;
        lines.push(registered(registration));
    }
    Ok(lines)
}

/// `registered the browser extension host for Chrome: <place>`.
fn registered(registration: &Registration) -> String {
    format!(
        "registered the browser extension host for {}: {}",
        registration.browser.name(),
        registration.place
    )
}

/// Something `--remove` deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    File(PathBuf),
    /// A key under `HKCU`.
    Key(String),
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Key(key) => write!(f, r"HKCU\{key}"),
        }
    }
}

/// What `--remove` does: the registrations of this install it deletes, and
/// a line for each other one it leaves in place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Removal {
    pub(crate) delete: Vec<Target>,
    pub(crate) keep: Vec<String>,
}

/// What `--remove` changes for `install`, read-only. Blocking.
pub(crate) fn removal(location: &Location, install: &Install) -> Removal {
    let mut removal = Removal::default();
    let ours = |found: &Found| matches!(found, Found::Current | Found::Stale { ours: true, .. });
    for registration in inspect(location, install) {
        let found = &registration.found;
        if ours(found) {
            removal.delete.push(match location.layout {
                Layout::Windows => Target::Key(location.key(registration.browser)),
                _ => Target::File(location.unix_manifest(registration.browser)),
            });
            continue;
        }
        let why = match found {
            Found::NoBrowser | Found::Missing | Found::Current => continue,
            Found::Stale {
                runs: Some(runs), ..
            }
            | Found::Kept { runs } => another_install(runs),
            Found::Stale { runs: None, .. } => "the manifest it names is missing. Delete the key, or run devlanes setup to point it at this install".into(),
            Found::Unreadable(reason) => format!(
                "it can't be read ({reason}). Delete it, or run devlanes setup to replace it"
            ),
        };
        removal
            .keep
            .push(format!("left {} in place: {why}", registration.place));
    }
    // The Windows manifest goes with this install's keys, or alone when no
    // key names it any more.
    if location.layout == Layout::Windows {
        let file = location.windows_manifest();
        match read_manifest(&file, &install.manifest()) {
            Manifest::Runs { path, .. } if same_path(&path, &install.entry) => {
                removal.delete.push(Target::File(file));
            }
            Manifest::Runs { path, .. } => removal.keep.push(format!(
                "left {} in place: {}",
                file.display(),
                another_install(&path)
            )),
            Manifest::Missing | Manifest::Unreadable(_) => {}
        }
    }
    removal
}

/// Why `--remove` leaves a registration that runs `runs` in place.
fn another_install(runs: &Path) -> String {
    format!(
        "it runs {}, which is another devlanes install. Run devlanes setup --remove with that install to delete it",
        runs.display()
    )
}

/// Deletes the targets of `removal` and returns a line for each. Blocking.
pub(crate) fn delete(removal: &Removal) -> Result<Vec<String>, BrowserError> {
    let mut lines = Vec::new();
    for target in &removal.delete {
        match target {
            Target::File(path) => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(BrowserError::Delete {
                        path: path.clone(),
                        source,
                    });
                }
            },
            Target::Key(key) => {
                registry::delete_tree(key).map_err(|source| BrowserError::Registry {
                    key: key.clone(),
                    source,
                })?
            }
        }
        lines.push(format!("removed {target}"));
    }
    Ok(lines)
}

/// `setup`'s run of the step: writes what isn't current and reports it.
pub(super) async fn install(
    runtime: &Runtime,
    location: &Location,
    install: &Install,
    registrations: &[Registration],
) -> Result<u8, RpcError> {
    let (location, install, registrations) =
        (location.clone(), install.clone(), registrations.to_vec());
    let lines = blocking(move || apply(&location, &install, &registrations)).await??;
    say(runtime, &lines);
    Ok(0)
}

/// `--remove`'s run of the step: deletes this install's registrations and
/// names the others it leaves.
pub(super) async fn remove(runtime: &Runtime, location: Option<&Location>) -> Result<u8, RpcError> {
    let Some(location) = location.cloned() else {
        return Ok(0);
    };
    let (removal, lines) = blocking(move || {
        let removal = removal(&location, &Install::current());
        let lines = delete(&removal);
        (removal, lines)
    })
    .await?;
    let mut lines = lines?;
    if lines.is_empty() && removal.keep.is_empty() {
        lines.push("the browser extension host isn't registered".into());
    }
    lines.extend(removal.keep);
    say(runtime, &lines);
    Ok(0)
}

/// What `--remove` does, as `(line, kept)` pairs: `remove <place>` for each
/// registration it deletes, then a `left <place> in place` line with `kept`
/// for each one it leaves. Read-only.
pub(super) async fn removal_lines(location: Option<&Location>) -> Vec<(String, bool)> {
    let Some(location) = location.cloned() else {
        return Vec::new();
    };
    blocking(move || removal(&location, &Install::current()))
        .await
        .map(|removal| {
            let deletes = removal
                .delete
                .iter()
                .map(|target| (format!("remove {target}"), false));
            deletes
                .chain(removal.keep.into_iter().map(|line| (line, true)))
                .collect()
        })
        .unwrap_or_default()
}

/// Doctor's facts: `{registered: [names], stale: [{browser, place, runs,
/// gone, error}], found: [names]}`, where `found` lists the browsers the
/// step looked at. Read-only.
pub(crate) async fn doctor_report() -> Value {
    let Some(location) = Location::current() else {
        return Value::Null;
    };
    blocking(move || report(&location, &Install::current()))
        .await
        .unwrap_or(Value::Null)
}

/// [`doctor_report`] for `location` and `install`. Blocking.
pub(crate) fn report(location: &Location, install: &Install) -> Value {
    let registrations = inspect(location, install);
    let names = |keep: &dyn Fn(&Found) -> bool| -> Vec<&str> {
        registrations
            .iter()
            .filter(|registration| keep(&registration.found))
            .map(|registration| registration.browser.name())
            .collect()
    };
    let stale: Vec<Value> = registrations
        .iter()
        .filter_map(|registration| {
            let (runs, error) = match &registration.found {
                Found::Stale { runs, .. } => (runs.as_ref(), None),
                Found::Kept { runs } => (Some(runs), None),
                Found::Unreadable(reason) => (None, Some(reason)),
                _ => return None,
            };
            Some(json!({
                "browser": registration.browser.name(),
                "place": registration.place,
                "runs": runs.map(|runs| runs.display().to_string()),
                "gone": runs.is_some_and(|runs| !runs.exists()),
                "error": error,
            }))
        })
        .collect();
    json!({
        "registered": names(&|found| *found == Found::Current),
        "stale": stale,
        "found": names(&|found| *found != Found::NoBrowser),
    })
}

/// Prints `lines` unless `--quiet`.
fn say(runtime: &Runtime, lines: &[String]) {
    if runtime.quiet || lines.is_empty() {
        return;
    }
    emit(&Rendered {
        stdout: lines.iter().map(|line| format!("{line}\n")).collect(),
        ..Rendered::default()
    });
}

/// Runs file and registry work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))
}

/// The `HKCU` calls of the Windows layout. Only Windows picks that layout, so
/// elsewhere the calls fail.
mod registry {
    #[cfg(windows)]
    pub(super) use devlanes_sys_windows::registry::{delete_tree, read_string, write_string};

    #[cfg(not(windows))]
    use std::io;

    /// The error every registry call gives off Windows.
    #[cfg(not(windows))]
    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "this OS has no registry")
    }

    /// Fails: this OS has no registry.
    #[cfg(not(windows))]
    pub(super) fn read_string(_subkey: &str, _name: &str) -> io::Result<Option<String>> {
        Err(unsupported())
    }

    /// Fails: this OS has no registry.
    #[cfg(not(windows))]
    pub(super) fn write_string(_subkey: &str, _name: &str, _value: &str) -> io::Result<()> {
        Err(unsupported())
    }

    /// Fails: this OS has no registry.
    #[cfg(not(windows))]
    pub(super) fn delete_tree(_subkey: &str) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script install whose entry is `entry`.
    fn script(entry: &Path) -> Install {
        Install::new(
            InstallMethod::Script(entry.parent().expect("parent").to_path_buf()),
            entry.to_path_buf(),
        )
    }

    /// A manifest that runs `path`, as another install writes it.
    fn other_manifest(path: &Path) -> String {
        json!({"name": HOST_NAME, "path": path, "type": "stdio", "allowed_origins": []}).to_string()
    }

    /// The `found` of every browser, in [`Browser::ALL`]'s order.
    fn founds(location: &Location, install: &Install) -> Vec<Found> {
        inspect(location, install)
            .into_iter()
            .map(|registration| registration.found)
            .collect()
    }

    /// On the macOS and Linux layouts, setup writes a manifest into each
    /// browser folder that exists and refreshes a stale one. `--remove`
    /// deletes the manifests that run this install, including an outdated
    /// one, and leaves another install's in place.
    #[test]
    fn unix_layouts_register_refresh_and_remove_only_this_install() {
        for layout in [Layout::MacOs, Layout::Linux] {
            let temp = tempfile::tempdir().expect("tempdir");
            let location = Location {
                home: temp.path().join("user"),
                layout,
                registry_root: String::new(),
            };
            let install = script(&temp.path().join("bin").join("devlanes"));
            let folder = |browser: Browser| location.home.join(browser.folder(layout));
            let manifest = |browser: Browser| location.unix_manifest(browser);
            for browser in [Browser::Chrome, Browser::Brave] {
                std::fs::create_dir_all(folder(browser)).expect("browser folder");
            }
            let gone = temp.path().join("gone").join("devlanes");
            std::fs::create_dir_all(manifest(Browser::Brave).parent().expect("parent"))
                .expect("hosts folder");
            std::fs::write(manifest(Browser::Brave), other_manifest(&gone)).expect("write");

            let registrations = inspect(&location, &install);
            assert_eq!(
                registrations
                    .iter()
                    .map(|registration| registration.found.clone())
                    .collect::<Vec<_>>(),
                [
                    Found::Missing,
                    Found::NoBrowser,
                    Found::Stale {
                        runs: Some(gone.clone()),
                        ours: false
                    },
                    Found::NoBrowser,
                ],
                "{layout:?}"
            );
            let facts = Facts::Found {
                install: install.clone(),
                registrations: registrations.clone(),
                held: None,
            };
            assert_eq!(
                plan(&facts),
                (
                    true,
                    false,
                    "setup registers the browser extension host for Chrome and refreshes it for Brave"
                        .into()
                )
            );
            let lines = apply(&location, &install, &registrations).expect("apply");
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(
                lines[0].starts_with("registered the browser extension host for Chrome: "),
                "{lines:?}"
            );
            let written: Value = serde_json::from_str(
                &std::fs::read_to_string(manifest(Browser::Chrome)).expect("read"),
            )
            .expect("json");
            assert_eq!(written, install.manifest());
            assert_eq!(
                written["allowed_origins"],
                json!(["chrome-extension://pggffblellmckocpflaaeligadjpkahj/"])
            );
            assert_eq!(written["path"], json!(install.entry));
            assert_eq!(
                founds(&location, &install),
                [
                    Found::Current,
                    Found::NoBrowser,
                    Found::Current,
                    Found::NoBrowser
                ]
            );
            assert!(
                plan(&Facts::Found {
                    install: install.clone(),
                    registrations: inspect(&location, &install),
                    held: None,
                })
                .1
            );
            // Nothing to write the second time.
            assert!(
                apply(&location, &install, &inspect(&location, &install))
                    .expect("apply")
                    .is_empty()
            );

            // Edge runs another install that exists, and Chromium has an
            // outdated manifest of this one.
            let other = temp.path().join("other").join("devlanes");
            std::fs::create_dir_all(other.parent().expect("parent")).expect("other");
            std::fs::write(&other, "").expect("other binary");
            for (browser, text) in [
                (Browser::Edge, other_manifest(&other)),
                (Browser::Chromium, other_manifest(&install.entry)),
            ] {
                std::fs::create_dir_all(manifest(browser).parent().expect("parent"))
                    .expect("hosts folder");
                std::fs::write(manifest(browser), text).expect("write");
            }
            let report = report(&location, &install);
            assert_eq!(report["registered"], json!(["Chrome", "Brave"]), "{report}");
            assert_eq!(
                report["found"],
                json!(["Chrome", "Chromium", "Brave", "Edge"])
            );
            assert_eq!(report["stale"][1]["browser"], "Edge", "{report}");
            assert_eq!(report["stale"][1]["gone"], false, "{report}");

            let removal = removal(&location, &install);
            assert_eq!(
                removal.delete,
                [Browser::Chrome, Browser::Chromium, Browser::Brave]
                    .map(|browser| Target::File(manifest(browser)))
            );
            assert_eq!(removal.keep.len(), 1, "{removal:?}");
            assert!(
                removal.keep[0].ends_with(&format!(
                    "in place: it runs {}, which is another devlanes install. Run devlanes setup --remove with that install to delete it",
                    other.display()
                )),
                "{removal:?}"
            );
            assert_eq!(delete(&removal).expect("delete").len(), 3);
            for browser in [Browser::Chrome, Browser::Chromium, Browser::Brave] {
                assert!(!manifest(browser).exists(), "{browser:?}");
            }
            assert_eq!(
                std::fs::read_to_string(manifest(Browser::Edge)).expect("read"),
                other_manifest(&other)
            );
            assert!(super::removal(&location, &install).delete.is_empty());
        }
    }

    /// uvx and pipx run installs are refused, and so is an entry that isn't
    /// an absolute path. Builds from a checkout get a note that the host
    /// runs that build.
    #[test]
    fn caches_are_refused_and_builds_are_named() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = Location {
            home: temp.path().to_path_buf(),
            layout: Layout::Linux,
            registry_root: String::new(),
        };
        std::fs::create_dir_all(temp.path().join(".config").join("chromium")).expect("folder");
        for method in [InstallMethod::Uvx, InstallMethod::PipxRun] {
            let install = Install::new(method, temp.path().join("cache").join("devlanes"));
            let Facts::Refused(reason) = facts(Some(&location), install.clone()) else {
                panic!("not refused");
            };
            assert!(reason.contains("runs devlanes from a cache"), "{reason}");
            assert!(matches!(
                apply(&location, &install, &inspect(&location, &install)),
                Err(BrowserError::Refused(_))
            ));
        }
        // `current_with_entry` falls back to a bare `devlanes` when the
        // binary's own path can't be read.
        let relative = Install::new(
            InstallMethod::Script(PathBuf::new()),
            PathBuf::from("devlanes"),
        );
        let Facts::Refused(reason) = facts(Some(&location), relative.clone()) else {
            panic!("not refused");
        };
        assert!(reason.contains("by its full path"), "{reason}");
        assert!(matches!(
            apply(&location, &relative, &inspect(&location, &relative)),
            Err(BrowserError::Refused(_))
        ));
        assert!(!location.unix_manifest(Browser::Chromium).exists());
        let build = temp.path().join("target").join("debug").join("devlanes");
        let install = Install::new(InstallMethod::Source, build.clone());
        let (needed, done, reason) = plan(&facts(Some(&location), install));
        assert!(needed && !done);
        assert_eq!(
            reason,
            format!(
                "setup registers the browser extension host for Chromium. The host runs this build at {}. Run devlanes setup again if you move or delete it",
                build.display()
            )
        );
        assert!(!plan(&facts(None, script(&build))).0);
    }

    /// A `Source` or `Local` build leaves another install's working
    /// registration alone and says so. A registration whose program is gone
    /// is still refreshed. A stable install takes over either one.
    #[test]
    fn builds_leave_a_working_registration_alone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = Location {
            home: temp.path().join("user"),
            layout: Layout::Linux,
            registry_root: String::new(),
        };
        let other = temp.path().join("opt").join("devlanes");
        std::fs::create_dir_all(other.parent().expect("parent")).expect("opt");
        std::fs::write(&other, "").expect("other binary");
        let gone = temp.path().join("gone").join("devlanes");
        for (browser, runs) in [(Browser::Chrome, &other), (Browser::Brave, &gone)] {
            let file = location.unix_manifest(browser);
            std::fs::create_dir_all(file.parent().expect("parent")).expect("hosts folder");
            std::fs::write(&file, other_manifest(runs)).expect("manifest");
        }
        let build = temp.path().join("target").join("debug").join("devlanes");
        for install in [
            Install::new(InstallMethod::Source, build.clone()),
            Install::new(
                InstallMethod::Local {
                    manifest: temp.path().join("package.json"),
                },
                build.clone(),
            ),
        ] {
            let registrations = inspect(&location, &install);
            assert_eq!(
                registrations[0].found,
                Found::Kept {
                    runs: other.clone()
                }
            );
            let (needed, done, reason) = plan(&Facts::Found {
                install: install.clone(),
                registrations: registrations.clone(),
                held: None,
            });
            assert!(needed && !done);
            // The whole line, so a doubled ". " or a trailing space shows.
            assert_eq!(
                reason,
                format!(
                    "setup refreshes the browser extension host for Brave. \
                     Chrome runs {}, another devlanes install, and a build doesn't replace it. \
                     To register this build, run devlanes setup --remove from that install first. \
                     The host runs this build at {}. Run devlanes setup again if you move or delete it",
                    other.display(),
                    build.display()
                )
            );
            let lines = apply(&location, &install, &registrations).expect("apply");
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert_eq!(
                std::fs::read_to_string(location.unix_manifest(Browser::Chrome)).expect("read"),
                other_manifest(&other)
            );
            // Put Brave back for the next install.
            std::fs::write(
                location.unix_manifest(Browser::Brave),
                other_manifest(&gone),
            )
            .expect("manifest");
        }
        let stable = script(&temp.path().join("bin").join("devlanes"));
        let registrations = inspect(&location, &stable);
        assert_eq!(
            registrations[0].found,
            Found::Stale {
                runs: Some(other.clone()),
                ours: false
            }
        );
        apply(&location, &stable, &registrations).expect("apply");
        assert_eq!(founds(&location, &stable)[0], Found::Current);
    }

    /// Deletes a test key on drop, so a failed test leaves nothing behind.
    #[cfg(windows)]
    struct TestKey(String);

    #[cfg(windows)]
    impl Drop for TestKey {
        /// Deletes the key and everything under it. A failure is ignored,
        /// since `drop` can't return it.
        fn drop(&mut self) {
            let _ = registry::delete_tree(&self.0);
        }
    }

    /// On Windows, one manifest under `.devlanes\browser` and all four keys
    /// pointing at it, under a test key instead of `HKCU\Software`.
    /// `--remove` deletes this install's keys and manifest, and leaves a key
    /// that names another install's manifest.
    #[cfg(windows)]
    #[test]
    fn windows_registers_every_key_and_removes_only_this_install() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-{}",
            std::process::id()
        ));
        let location = Location {
            home: temp.path().join("user"),
            layout: Layout::Windows,
            registry_root: root.0.clone(),
        };
        std::fs::create_dir_all(&location.home).expect("home");
        let install = script(&temp.path().join("bin").join("devlanes.exe"));
        assert_eq!(founds(&location, &install), vec![Found::Missing; 4]);

        let lines = apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(lines.len(), 5, "{lines:?}");
        let file = location
            .home
            .join(".devlanes")
            .join("browser")
            .join("run.devlanes.browser.json");
        assert!(file.is_file());
        for browser in Browser::ALL {
            assert_eq!(
                registry::read_string(&location.key(browser), "").expect("read"),
                Some(file.display().to_string())
            );
        }
        assert!(
            location
                .key(Browser::Chrome)
                .ends_with(r"\Google\Chrome\NativeMessagingHosts\run.devlanes.browser")
        );
        assert_eq!(founds(&location, &install), vec![Found::Current; 4]);

        let other = temp.path().join("other.json");
        let other_exe = temp.path().join("other").join("devlanes.exe");
        std::fs::write(&other, other_manifest(&other_exe)).expect("other manifest");
        let edge = location.key(Browser::Edge);
        registry::write_string(&edge, "", &other.display().to_string()).expect("edge key");
        assert_eq!(
            founds(&location, &install)[3],
            Found::Stale {
                runs: Some(other_exe.clone()),
                ours: false
            }
        );
        let report = report(&location, &install);
        assert_eq!(report["stale"][0]["gone"], true, "{report}");

        let removal = removal(&location, &install);
        assert_eq!(removal.delete.len(), 4, "{removal:?}");
        assert_eq!(removal.keep.len(), 1, "{removal:?}");
        delete(&removal).expect("delete");
        assert!(!file.exists());
        for browser in [Browser::Chrome, Browser::Chromium, Browser::Brave] {
            assert_eq!(
                registry::read_string(&location.key(browser), "").expect("read"),
                None
            );
        }
        assert_eq!(
            registry::read_string(&edge, "").expect("read"),
            Some(other.display().to_string())
        );
    }

    /// A `Source` build leaves the shared manifest alone while it runs
    /// another working install: three keys name it and Edge has none, and
    /// setup writes neither the file nor Edge's key.
    #[cfg(windows)]
    #[test]
    fn a_build_leaves_the_shared_windows_manifest_alone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-held-{}",
            std::process::id()
        ));
        let location = Location {
            home: temp.path().join("user"),
            layout: Layout::Windows,
            registry_root: root.0.clone(),
        };
        let other = temp.path().join("other").join("devlanes.exe");
        std::fs::create_dir_all(other.parent().expect("parent")).expect("other");
        std::fs::write(&other, "").expect("other binary");
        let shared = location.windows_manifest();
        std::fs::create_dir_all(shared.parent().expect("parent")).expect("browser folder");
        std::fs::write(&shared, other_manifest(&other)).expect("shared manifest");
        for browser in [Browser::Chrome, Browser::Chromium, Browser::Brave] {
            registry::write_string(&location.key(browser), "", &shared.display().to_string())
                .expect("key");
        }
        let build = temp
            .path()
            .join("target")
            .join("debug")
            .join("devlanes.exe");
        let install = Install::new(InstallMethod::Source, build.clone());
        let facts = facts(Some(&location), install.clone());
        let Facts::Found {
            registrations,
            held,
            ..
        } = &facts
        else {
            panic!("{facts:?}");
        };
        assert_eq!(
            held,
            &Some(Held {
                manifest: shared.clone(),
                runs: other.clone()
            })
        );
        let (needed, done, reason) = plan(&facts);
        assert!(!needed && !done, "{reason}");
        assert!(
            reason.contains(&format!(
                "Edge would need {}, which runs {}, and a build doesn't replace it. To register this build",
                shared.display(),
                other.display()
            )),
            "{reason}"
        );
        assert!(!reason.contains(". ."), "{reason}");
        assert!(
            apply(&location, &install, registrations)
                .expect("apply")
                .is_empty()
        );
        assert_eq!(
            std::fs::read_to_string(&shared).expect("read"),
            other_manifest(&other)
        );
        assert_eq!(
            registry::read_string(&location.key(Browser::Edge), "").expect("read"),
            None
        );
    }

    /// After `.devlanes` was deleted, the keys still name the file setup
    /// writes there. They are devlanes' own, so `--remove` deletes them. A
    /// key that names a missing file elsewhere stays.
    #[cfg(windows)]
    #[test]
    fn windows_removes_its_keys_once_the_manifest_is_gone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-gone-{}",
            std::process::id()
        ));
        let location = Location {
            home: temp.path().join("user"),
            layout: Layout::Windows,
            registry_root: root.0.clone(),
        };
        std::fs::create_dir_all(&location.home).expect("home");
        let install = script(&temp.path().join("bin").join("devlanes.exe"));
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        std::fs::remove_dir_all(location.home.join(".devlanes")).expect("delete .devlanes");
        let elsewhere = temp.path().join("missing.json");
        let edge = location.key(Browser::Edge);
        registry::write_string(&edge, "", &elsewhere.display().to_string()).expect("edge key");
        assert_eq!(
            founds(&location, &install)[..3],
            vec![
                Found::Stale {
                    runs: None,
                    ours: true
                };
                3
            ]
        );
        let removal = removal(&location, &install);
        assert_eq!(
            removal.delete,
            [Browser::Chrome, Browser::Chromium, Browser::Brave]
                .map(|browser| Target::Key(location.key(browser)))
        );
        assert_eq!(removal.keep.len(), 1, "{removal:?}");
        assert!(
            removal.keep[0].contains("the manifest it names is missing"),
            "{removal:?}"
        );
        delete(&removal).expect("delete");
        for browser in [Browser::Chrome, Browser::Chromium, Browser::Brave] {
            assert_eq!(
                registry::read_string(&location.key(browser), "").expect("read"),
                None
            );
        }
        assert!(registry::read_string(&edge, "").expect("read").is_some());
    }
}
