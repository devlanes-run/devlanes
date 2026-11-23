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
    /// cache, which they may clear or replace at any time.
    fn refusal(&self) -> Option<String> {
        let (runner, install) = match self.method {
            InstallMethod::Uvx => ("uvx", "uv tool install devlanes"),
            InstallMethod::PipxRun => ("pipx run", "pipx install devlanes"),
            _ => return None,
        };
        Some(format!(
            "{runner} runs devlanes from a cache, so the browser extension can't start it. Install devlanes with {install}, then run devlanes setup again"
        ))
    }

    /// A note for builds that may move or go away, such as a checkout's
    /// `target/` folder.
    fn note(&self) -> Option<String> {
        matches!(
            self.method,
            InstallMethod::Source | InstallMethod::Local { .. }
        )
        .then(|| format!("the host runs this build at {}", self.entry.display()))
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
        !matches!(self.found, Found::NoBrowser | Found::Current)
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
    },
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
        Err(error) => return Manifest::Unreadable(format!("{}: {error}", file.display())),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Manifest::Unreadable(format!("{}: {error}", file.display())),
    };
    match value.get("path").and_then(Value::as_str) {
        Some(path) => Manifest::Runs {
            path: PathBuf::from(path),
            current: &value == expected,
        },
        None => Manifest::Unreadable(format!("{} has no path", file.display())),
    }
}

/// What a manifest means for a registration of `install`.
fn found_of(manifest: Manifest, install: &Install) -> Found {
    match manifest {
        Manifest::Missing => Found::Stale {
            runs: None,
            ours: false,
        },
        Manifest::Unreadable(reason) => Found::Unreadable(reason),
        Manifest::Runs { current: true, .. } => Found::Current,
        Manifest::Runs { path, .. } => Found::Stale {
            ours: same_path(&path, &install.entry),
            runs: Some(path),
        },
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
        Err(error) => Found::Unreadable(format!(r"HKCU\{key}: {error}")),
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
        install,
    }
}

/// `(needed, done, reason)` of the step. `needed` means there is a browser to
/// register for, and the step never counts as missing either way.
pub(crate) fn plan(facts: &Facts) -> (bool, bool, String) {
    let (install, registrations) = match facts {
        Facts::NoHome => {
            return (
                false,
                false,
                "devlanes can't tell where your home folder is".into(),
            );
        }
        Facts::Refused(reason) => return (false, false, reason.clone()),
        Facts::Found {
            install,
            registrations,
        } => (install, registrations),
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
    let missing = names(&|found| *found == Found::Missing);
    let stale = names(&|found| matches!(found, Found::Stale { .. } | Found::Unreadable(_)));
    let note = install
        .note()
        .map(|note| format!(". {}", capitalise(&note)))
        .unwrap_or_default();
    if registered.is_none() && missing.is_none() && stale.is_none() {
        return (
            false,
            false,
            "no Chrome, Chromium, Brave or Edge folder in your home".into(),
        );
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
    #[error(r"couldn't change the registry key HKCU\{key}: {source}. Run devlanes setup again")]
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
    let temporary = tempfile::NamedTempFile::new_in(dir).map_err(failed)?;
    std::fs::write(temporary.path(), text).map_err(failed)?;
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
        if todo.is_empty() {
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
            } => {
                format!("it runs {}, not this devlanes", runs.display())
            }
            Found::Stale { runs: None, .. } => "the manifest it names is missing".into(),
            Found::Unreadable(reason) => format!("it can't be read: {reason}"),
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
                "left {} in place: it runs {}, not this devlanes",
                file.display(),
                path.display()
            )),
            Manifest::Missing | Manifest::Unreadable(_) => {}
        }
    }
    removal
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

/// What `--remove` deletes, one `remove <place>` line each. Read-only.
pub(super) async fn removal_lines(location: Option<&Location>) -> Vec<String> {
    let Some(location) = location.cloned() else {
        return Vec::new();
    };
    blocking(move || removal(&location, &Install::current()))
        .await
        .map(|removal| {
            removal
                .delete
                .iter()
                .map(|target| format!("remove {target}"))
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
                    "in place: it runs {}, not this devlanes",
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

    /// uvx and pipx run installs are refused. Builds from a checkout get a
    /// note that the host runs that build.
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
        assert!(!location.unix_manifest(Browser::Chromium).exists());
        let build = temp.path().join("target").join("debug").join("devlanes");
        let install = Install::new(InstallMethod::Source, build.clone());
        let (needed, done, reason) = plan(&facts(Some(&location), install));
        assert!(needed && !done);
        assert_eq!(
            reason,
            format!(
                "setup registers the browser extension host for Chromium. The host runs this build at {}",
                build.display()
            )
        );
        assert!(!plan(&facts(None, script(&build))).0);
    }

    /// Deletes a test key on drop, so a failed test leaves nothing behind.
    #[cfg(windows)]
    struct TestKey(String);

    #[cfg(windows)]
    impl Drop for TestKey {
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
