//! The `browser` step: the native messaging host manifest that lets the
//! devlanes browser extension start this devlanes, for the Chromium browsers
//! in [`Browser::ALL`].
//!
//! Only browsers whose user data folder exists are registered. On macOS and
//! Linux each browser reads `NativeMessagingHosts/` in that folder, so the
//! manifest goes there, except Brave on macOS, which reads Chrome's folder.
//! On Windows the browsers read a registry key that names the manifest, so
//! one manifest goes under `%USERPROFILE%\.devlanes\browser\` and each
//! installed browser's key points at it.
//!
//! That file, `~/.devlanes/browser/run.devlanes.browser.json`, is kept on
//! every OS, so a user with a Chromium browser outside the list can link it
//! into that browser's `NativeMessagingHosts` folder or point a key at it.
//! A link to it in a listed browser's folder counts as this install's
//! registration. A link or key for any other browser is the user's.
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

/// A browser the step registers the host for, and where it keeps its user
/// data on each OS. A field is `None` where the browser doesn't ship, or
/// where no source backs a path. Paths use `/` and are split into
/// components, so they work on every OS.
///
/// On macOS and Linux a Chromium browser reads the user's hosts from
/// `<user data>/NativeMessagingHosts` (`DIR_USER_NATIVE_MESSAGING` in
/// Chromium's `chrome/common/chrome_paths.cc`). On Windows it reads keys
/// under `HKCU\Software` that name the manifest. Which keys depends on the
/// build, not on the browser's own registry folder: `GetManifestPathWithFlags`
/// in Chromium's `chrome/browser/extensions/api/messaging/launch_context_win.cc`
/// reads `Software\Chromium` and then `Software\Google\Chrome` in any build
/// without Google Chrome branding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Browser {
    name: &'static str,
    /// The user data folder under `~/Library/Application Support`.
    macos: Option<&'static str>,
    /// The user data folder under `~/.config`.
    linux: Option<&'static str>,
    /// The user data folder under `%LOCALAPPDATA%`, and the keys under
    /// `HKCU\Software` setup writes for the browser. The browser counts as
    /// registered only when each of them is.
    windows: Option<(&'static str, &'static [&'static str])>,
}

/// `Software\Google\Chrome`, which every Chromium build on Windows reads,
/// after its own key when it has one.
const CHROME_KEY: &str = "Google/Chrome";
/// `Software\Chromium`, which a build without Google Chrome branding reads
/// before [`CHROME_KEY`].
const CHROMIUM_KEY: &str = "Chromium";

impl Browser {
    // Chrome's channels: the folders are Chromium's `docs/user_data_dir.md`.
    // A Google Chrome build reads only `Software\Google\Chrome`
    // (`launch_context_win.cc`), so the channels share that key.
    pub(crate) const CHROME: Self = Self {
        name: "Chrome",
        macos: Some("Google/Chrome"),
        linux: Some("google-chrome"),
        windows: Some(("Google/Chrome/User Data", &[CHROME_KEY])),
    };
    pub(crate) const CHROME_BETA: Self = Self {
        name: "Chrome Beta",
        macos: Some("Google/Chrome Beta"),
        linux: Some("google-chrome-beta"),
        windows: Some(("Google/Chrome Beta/User Data", &[CHROME_KEY])),
    };
    pub(crate) const CHROME_DEV: Self = Self {
        name: "Chrome Dev",
        macos: Some("Google/Chrome Dev"),
        linux: Some("google-chrome-unstable"),
        windows: Some(("Google/Chrome Dev/User Data", &[CHROME_KEY])),
    };
    pub(crate) const CHROME_CANARY: Self = Self {
        name: "Chrome Canary",
        macos: Some("Google/Chrome Canary"),
        linux: Some("google-chrome-canary"),
        windows: Some(("Google/Chrome SxS/User Data", &[CHROME_KEY])),
    };
    // Chromium reads its own key first, so nothing in Chrome's can shadow it.
    pub(crate) const CHROMIUM: Self = Self {
        name: "Chromium",
        macos: Some("Chromium"),
        linux: Some("chromium"),
        windows: Some(("Chromium/User Data", &[CHROMIUM_KEY])),
    };
    // Edge's channels: the folders are Microsoft's "Chrome DevTools MCP"
    // page (https://learn.microsoft.com/en-us/microsoft-edge/web-platform/devtools-mcp-server),
    // and the key is its "Native messaging" page
    // (https://learn.microsoft.com/en-us/microsoft-edge/extensions/developer-guide/native-messaging).
    // Edge reads its own key first, then Chromium's and Chrome's, so its own
    // is enough. Edge has no Canary on Linux.
    pub(crate) const EDGE: Self = Self {
        name: "Edge",
        macos: Some("Microsoft Edge"),
        linux: Some("microsoft-edge"),
        windows: Some(("Microsoft/Edge/User Data", &["Microsoft/Edge"])),
    };
    pub(crate) const EDGE_BETA: Self = Self {
        name: "Edge Beta",
        macos: Some("Microsoft Edge Beta"),
        linux: Some("microsoft-edge-beta"),
        windows: Some(("Microsoft/Edge Beta/User Data", &["Microsoft/Edge"])),
    };
    pub(crate) const EDGE_DEV: Self = Self {
        name: "Edge Dev",
        macos: Some("Microsoft Edge Dev"),
        linux: Some("microsoft-edge-dev"),
        windows: Some(("Microsoft/Edge Dev/User Data", &["Microsoft/Edge"])),
    };
    pub(crate) const EDGE_CANARY: Self = Self {
        name: "Edge Canary",
        macos: Some("Microsoft Edge Canary"),
        linux: None,
        windows: Some(("Microsoft/Edge SxS/User Data", &["Microsoft/Edge"])),
    };
    // Brave: the folders are Bitwarden's and KeePassXC's installers, and
    // Brave's support pages on Windows. On Windows Brave reads Chromium's
    // key, then Chrome's, and never `Software\BraveSoftware`: brave-core
    // doesn't override `launch_context_win.cc`, and Brave 158 finds a host
    // under either key and not under its own. Both are written, so Brave
    // finds this install whichever one it reads. On macOS Brave reads
    // Chrome's folder, see [`Browser::hosts_folder`].
    pub(crate) const BRAVE: Self = Self {
        name: "Brave",
        macos: Some("BraveSoftware/Brave-Browser"),
        linux: Some("BraveSoftware/Brave-Browser"),
        windows: Some((
            "BraveSoftware/Brave-Browser/User Data",
            &[CHROMIUM_KEY, CHROME_KEY],
        )),
    };
    // Vivaldi: the folders are Bitwarden's and KeePassXC's installers, and
    // Vivaldi's help on Windows. Bitwarden writes `Software\Vivaldi`, while
    // KeePassXC-Browser's issue #48 reports Vivaldi reading Chrome's key,
    // as a build without Google Chrome branding does. Nothing settles it, so
    // setup writes Vivaldi's key and the two every such build reads.
    pub(crate) const VIVALDI: Self = Self {
        name: "Vivaldi",
        macos: Some("Vivaldi"),
        linux: Some("vivaldi"),
        windows: Some(("Vivaldi/User Data", &["Vivaldi", CHROMIUM_KEY, CHROME_KEY])),
    };
    // Arc on macOS: the folder is KeePassXC-Browser's troubleshooting guide.
    // Nothing documents where Arc on Windows reads hosts.
    pub(crate) const ARC: Self = Self {
        name: "Arc",
        macos: Some("Arc/User Data"),
        linux: None,
        windows: None,
    };

    /// Every browser, in the order setup names them.
    pub(crate) const ALL: [Self; 12] = [
        Self::CHROME,
        Self::CHROME_BETA,
        Self::CHROME_DEV,
        Self::CHROME_CANARY,
        Self::CHROMIUM,
        Self::EDGE,
        Self::EDGE_BETA,
        Self::EDGE_DEV,
        Self::EDGE_CANARY,
        Self::BRAVE,
        Self::VIVALDI,
        Self::ARC,
    ];

    /// The name setup and doctor print.
    pub(crate) fn name(self) -> &'static str {
        self.name
    }

    /// The browser's user data folder for `layout`, relative to the folder
    /// [`Location::user_data`] puts it under; `None` where it doesn't ship.
    fn folder(self, layout: Layout) -> Option<PathBuf> {
        let path = match layout {
            Layout::MacOs => self.macos,
            Layout::Linux => self.linux,
            Layout::Windows => self.windows.map(|(folder, _)| folder),
        }?;
        Some(path.split('/').collect())
    }

    /// The folder whose `NativeMessagingHosts` the browser reads on macOS and
    /// Linux, relative to the same folder as [`Browser::folder`].
    fn hosts_folder(self, layout: Layout) -> Option<PathBuf> {
        // brave-core's `BraveMainDelegate::PreSandboxStartup` points Brave on
        // macOS at `~/Library/Application Support/Google/Chrome`, whatever
        // `--user-data-dir` says, and creates its `NativeMessagingHosts` on
        // every start. On Linux Brave reads its own folder.
        if layout == Layout::MacOs && self == Self::BRAVE {
            return Self::CHROME.folder(layout);
        }
        self.folder(layout)
    }

    /// The keys setup writes for the browser under the registry root, on
    /// Windows.
    fn keys(self) -> &'static [&'static str] {
        self.windows.map(|(_, keys)| keys).unwrap_or_default()
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
    /// Windows: `%LOCALAPPDATA%`, where the browsers keep their user data.
    pub(crate) local_app_data: PathBuf,
    pub(crate) layout: Layout,
    /// The `HKCU` key the Windows browser keys sit under, `Software`.
    pub(crate) registry_root: String,
}

impl Location {
    /// This user's home and this OS's layout; `None` when the home is unknown.
    pub(crate) fn current() -> Option<Self> {
        user_home().map(|home| Self {
            // The default, for a session without the variable.
            local_app_data: std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .unwrap_or_else(|| home.join("AppData").join("Local")),
            home,
            layout: Layout::current(),
            registry_root: "Software".into(),
        })
    }

    /// The browsers that ship on this layout.
    fn browsers(&self) -> impl Iterator<Item = Browser> + '_ {
        Browser::ALL
            .into_iter()
            .filter(|browser| browser.folder(self.layout).is_some())
    }

    /// The folder the browsers keep their user data under.
    fn base(&self) -> PathBuf {
        match self.layout {
            Layout::MacOs => self.home.join("Library").join("Application Support"),
            Layout::Linux => self.home.join(".config"),
            Layout::Windows => self.local_app_data.clone(),
        }
    }

    /// `browser`'s user data folder.
    fn user_data(&self, browser: Browser) -> PathBuf {
        self.base()
            .join(browser.folder(self.layout).unwrap_or_default())
    }

    /// Whether `browser` is installed: its user data folder exists. Brave on
    /// macOS creates Chrome's folder too, so there Chrome counts only with
    /// `Local State`, which Chrome writes on its first start and Brave never
    /// writes there.
    fn installed(&self, browser: Browser) -> bool {
        let folder = self.user_data(browser);
        if self.layout == Layout::MacOs && browser == Browser::CHROME {
            return folder.join("Local State").is_file();
        }
        folder.is_dir()
    }

    /// The copy of the manifest setup keeps on every OS, for a browser that
    /// isn't on the list. On Windows every key names it.
    fn copy(&self) -> PathBuf {
        self.home
            .join(".devlanes")
            .join("browser")
            .join(manifest_name())
    }

    /// The registry key, under `HKCU`, that points the browsers reading
    /// `browser_key`, such as `Google/Chrome`, at a manifest.
    fn key(&self, browser_key: &str) -> String {
        format!(
            r"{}\{}\NativeMessagingHosts\{HOST_NAME}",
            self.registry_root,
            browser_key.replace('/', r"\")
        )
    }

    /// The manifest `browser` reads on macOS and Linux. Brave and Chrome share
    /// one on macOS.
    fn unix_manifest(&self, browser: Browser) -> PathBuf {
        self.base()
            .join(browser.hosts_folder(self.layout).unwrap_or_default())
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
    /// cache, which they may clear or replace at any time. A relative entry
    /// is refused too, because the browser can't start it.
    fn refusal(&self) -> Option<String> {
        if !self.entry.is_absolute() {
            return Some(format!(
                "devlanes can't find the full path of its own binary, only {}, so the browser extension can't start it. Run devlanes setup again as /full/path/to/devlanes setup",
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

/// One browser's registration. On Windows a browser has one per key it
/// gets, and it is registered only when all of them are current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registration {
    pub(crate) browser: Browser,
    /// The manifest file, or on Windows the registry key, as setup names it.
    pub(crate) place: String,
    /// Windows: the key under `HKCU` that `place` names.
    pub(crate) key: Option<String>,
    pub(crate) found: Found,
    /// Whether the browser is installed, by [`Location::installed`]. On
    /// Windows a key can outlive its browser, and `--remove` still deletes
    /// it when it's ours.
    pub(crate) installed: bool,
    /// macOS and Linux: the manifest is a symlink to [`Location::copy`],
    /// so writing the copy keeps it current.
    pub(crate) linked: bool,
}

impl Registration {
    /// Whether setup writes this registration: only for a browser that is
    /// installed.
    fn to_write(&self) -> bool {
        self.installed
            && !matches!(
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
        copy: ManifestCopy,
    },
}

/// The manifest copy at [`Location::copy`], as setup found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestCopy {
    pub(crate) path: PathBuf,
    /// [`Found::Kept`] when another working install holds it and setup runs
    /// from a `Source` or `Local` build, which then leaves it alone.
    pub(crate) found: Found,
    /// Windows: every key names this file, so while another install holds
    /// it a build can't register any browser without taking the others over.
    pub(crate) named_by_keys: bool,
}

impl ManifestCopy {
    /// Whether setup writes the copy.
    fn to_write(&self) -> bool {
        matches!(
            self.found,
            Found::Missing | Found::Stale { .. } | Found::Unreadable(_)
        )
    }

    /// The program another install's copy starts, when a build leaves it
    /// alone.
    fn held(&self) -> Option<&Path> {
        match &self.found {
            Found::Kept { runs } => Some(runs),
            _ => None,
        }
    }
}

/// The manifest copy at `location` for `install`. Blocking.
fn copy_of(location: &Location, install: &Install) -> ManifestCopy {
    let path = location.copy();
    let found = match read_manifest(&path, &install.manifest()) {
        Manifest::Missing => Found::Missing,
        manifest => found_of(manifest, install),
    };
    ManifestCopy {
        path,
        found,
        named_by_keys: location.layout == Layout::Windows,
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

/// Reads the registration of every browser that ships on this layout, for
/// `install`. Blocking.
pub(crate) fn inspect(location: &Location, install: &Install) -> Vec<Registration> {
    let expected = install.manifest();
    location
        .browsers()
        .flat_map(|browser| {
            let installed = location.installed(browser);
            if location.layout == Layout::Windows {
                return browser
                    .keys()
                    .iter()
                    .map(|key| inspect_key(location, install, &expected, browser, key, installed))
                    .collect();
            }
            let file = location.unix_manifest(browser);
            let linked = links_to(&file, &location.copy());
            let found = if !installed {
                Found::NoBrowser
            } else {
                match read_manifest(&file, &expected) {
                    // A link to the copy after the copy was deleted with
                    // `.devlanes`: devlanes' own to clean up, as on Windows.
                    Manifest::Missing if linked => Found::Stale {
                        runs: None,
                        ours: true,
                    },
                    Manifest::Missing => Found::Missing,
                    manifest => found_of(manifest, install),
                }
            };
            vec![Registration {
                browser,
                place: file.display().to_string(),
                key: None,
                found,
                installed,
                linked,
            }]
        })
        .collect()
}

/// Whether `file` is a symlink whose target is `copy`, whether or not the
/// target exists. Blocking.
fn links_to(file: &Path, copy: &Path) -> bool {
    let Ok(target) = std::fs::read_link(file) else {
        return false;
    };
    // A relative target is relative to the link's folder. It is resolved
    // without the filesystem, since the copy may be gone.
    let target = match file.parent() {
        Some(dir) => dir.join(target),
        None => target,
    };
    let mut resolved = PathBuf::new();
    for component in target.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    // A home reached through a symlink spells the same file differently.
    let canonical = |path: &Path| dunce::canonicalize(path).ok();
    same_path(&resolved, copy)
        || canonical(&resolved).is_some_and(|target| Some(target) == canonical(copy))
}

/// The Windows registration of `browser` under `browser_key`, one of its
/// [`Browser::keys`]: the key, and the manifest it names. The key is read
/// even when the browser isn't `installed`, since an earlier setup may have
/// written it.
fn inspect_key(
    location: &Location,
    install: &Install,
    expected: &Value,
    browser: Browser,
    browser_key: &str,
    installed: bool,
) -> Registration {
    let key = location.key(browser_key);
    let found = match registry::read_string(&key, "") {
        Ok(None) if !installed => Found::NoBrowser,
        Ok(None) => Found::Missing,
        Err(error) => Found::Unreadable(error.to_string()),
        Ok(Some(named)) => {
            let named = PathBuf::from(named);
            let own_file = same_path(&named, &location.copy());
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
        key: Some(key),
        found,
        installed,
        linked: false,
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
        copy: copy_of(location, &install),
        install,
    }
}

/// `(needed, done, reason)` of the step. `needed` means there is a browser to
/// register for or a manifest copy to keep, and the step never counts as
/// missing either way.
pub(crate) fn plan(facts: &Facts) -> (bool, bool, String) {
    let (install, registrations, copy) = match facts {
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
            copy,
        } => (install, registrations, copy),
    };
    // A key left behind for a browser that's gone is only `--remove`'s.
    let registrations: Vec<&Registration> = registrations
        .iter()
        .filter(|registration| registration.installed)
        .collect();
    let names = |keep: &dyn Fn(&Found) -> bool| join(&browser_names(&registrations, keep));
    let registered = join(&registered_names(&registrations));
    let is_stale = |found: &Found| matches!(found, Found::Stale { .. } | Found::Unreadable(_));
    let stale_names = browser_names(&registrations, &is_stale);
    // A browser with one missing key and one stale one is named once, as
    // refreshed, since it has a registration already.
    let mut missing = join(
        &browser_names(&registrations, &|found| *found == Found::Missing)
            .into_iter()
            .filter(|name| !stale_names.contains(name))
            .collect::<Vec<_>>(),
    );
    let mut stale = join(&stale_names);
    let mut kept: Vec<String> = Vec::new();
    for registration in &registrations {
        if let Found::Kept { runs } = &registration.found {
            let line = format!(
                "{} runs {}, another devlanes install, and a build doesn't replace it",
                registration.browser.name(),
                runs.display()
            );
            // Two keys of one browser can run the same install.
            if !kept.contains(&line) {
                kept.push(line);
            }
        }
    }
    if let Some(runs) = copy.held() {
        if copy.named_by_keys {
            // Writing any key would rewrite the shared manifest the kept
            // keys name.
            let blocked = names(&|found| {
                matches!(
                    found,
                    Found::Missing | Found::Stale { .. } | Found::Unreadable(_)
                )
            });
            (missing, stale) = (None, None);
            if let Some(blocked) = blocked {
                kept.push(format!(
                    "{blocked} can't be registered: the shared manifest {} runs {}, another devlanes install, and a build doesn't replace it",
                    copy.path.display(),
                    runs.display()
                ));
            }
        } else {
            kept.push(format!(
                "The manifest copy {} runs {}, another devlanes install, and a build doesn't replace it",
                copy.path.display(),
                runs.display()
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
    let host = "the browser extension host";
    let verb = if copy.found == Found::Missing {
        "writes"
    } else {
        "refreshes"
    };
    let copy_line = format!(
        "{verb} the manifest copy for other Chromium browsers at {}",
        copy.path.display()
    );
    let mut reason = match (missing, stale) {
        (Some(missing), None) => format!("setup registers {host} for {missing}"),
        (None, Some(stale)) => format!("setup refreshes {host} for {stale}"),
        (Some(missing), Some(stale)) => {
            format!("setup registers {host} for {missing} and refreshes it for {stale}")
        }
        // The copy is named only when it is all setup writes.
        (None, None) if copy.to_write() => format!("setup {copy_line}"),
        (None, None) => {
            let reason = match (&registered, &kept) {
                (Some(registered), _) => format!("{host} is registered for {registered}{note}"),
                (None, Some(kept)) => return (false, false, kept.clone()),
                (None, None) => format!(
                    "none of the browsers devlanes registers is installed. The manifest copy for other Chromium browsers is at {}{note}",
                    copy.path.display()
                ),
            };
            return (true, true, reason);
        }
    };
    if let Some(registered) = registered {
        let subject = if reason.contains(host) { "it" } else { host };
        reason.push_str(&format!(", and {subject} is registered for {registered}"));
    }
    (true, false, format!("{reason}{note}"))
}

/// The browsers with a registration `keep` matches, each named once, in the
/// order of `registrations`.
fn browser_names(
    registrations: &[&Registration],
    keep: &dyn Fn(&Found) -> bool,
) -> Vec<&'static str> {
    let mut names = Vec::new();
    for registration in registrations {
        let name = registration.browser.name();
        if keep(&registration.found) && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The browsers whose registrations are all current. On Windows a browser
/// with one current key and another one stale may still find another host.
fn registered_names(registrations: &[&Registration]) -> Vec<&'static str> {
    let unfinished = browser_names(registrations, &|found| *found != Found::Current);
    browser_names(registrations, &|found| *found == Found::Current)
        .into_iter()
        .filter(|name| !unfinished.contains(name))
        .collect()
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

/// Creates `~/.devlanes` private when it is missing, as the client creates
/// the devlanes home, since the manifest copy goes inside it.
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)
            .map_err(|source| BrowserError::Write {
                path: folder.clone(),
                source,
            })
    }
    #[cfg(not(any(unix, windows)))]
    std::fs::create_dir_all(&folder).map_err(|source| BrowserError::Write {
        path: folder.clone(),
        source,
    })
}

/// Writes the manifest copy and the registrations of `registrations` that
/// aren't current, and returns a line for each. Blocking.
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
    let copy = copy_of(location, install);
    // On Windows every key names the copy, so with another install holding
    // it a build writes neither the file nor any key.
    if copy.held().is_some() && copy.named_by_keys {
        return Ok(lines);
    }
    if copy.to_write() {
        devlanes_folder(location)?;
        write_manifest(&copy.path, &manifest)?;
        lines.push(format!("wrote {}", copy.path.display()));
    }
    if location.layout == Layout::Windows {
        let value = copy.path.to_string_lossy();
        // A browser's channels share one key, and Brave and Vivaldi share
        // Chromium's and Chrome's, so each key is written once.
        let mut written: Vec<String> = Vec::new();
        for registration in todo {
            let Some(key) = registration.key.clone() else {
                continue;
            };
            if !written.contains(&key) {
                registry::write_string(&key, "", &value).map_err(|source| {
                    BrowserError::Registry {
                        key: key.clone(),
                        source,
                    }
                })?;
                written.push(key);
            }
            lines.push(registered(registration));
        }
        return Ok(lines);
    }
    // Brave and Chrome share a manifest on macOS, so each file is written once.
    let mut written: Vec<PathBuf> = Vec::new();
    for registration in todo {
        let file = location.unix_manifest(registration.browser);
        // A link to the copy is current once the copy is, and replacing it
        // with a file would undo the user's link.
        if !registration.linked && !written.contains(&file) {
            write_manifest(&file, &manifest)?;
            written.push(file);
        }
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
            let target = match registration.key {
                Some(key) => Target::Key(key),
                None => Target::File(location.unix_manifest(registration.browser)),
            };
            // Browsers share keys on Windows.
            if !removal.delete.contains(&target) {
                removal.delete.push(target);
            }
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
        let line = format!("left {} in place: {why}", registration.place);
        if !removal.keep.contains(&line) {
            removal.keep.push(line);
        }
    }
    // The copy goes last, after the links and keys that name it. A link or
    // key the user made for a browser outside the list is theirs to delete.
    let file = location.copy();
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
/// gone, error, kept, ours}], found: [names]}`, where `found` lists the
/// browsers the step looked at, `kept` marks another install's registration
/// that this build leaves alone, and `ours` marks this install's own
/// registration that is out of date. Read-only.
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
    let inspected = inspect(location, install);
    let registrations: Vec<&Registration> = inspected
        .iter()
        .filter(|registration| registration.installed)
        .collect();
    let bad: Vec<&Registration> = registrations
        .iter()
        .copied()
        .filter(|registration| {
            matches!(
                registration.found,
                Found::Stale { .. } | Found::Kept { .. } | Found::Unreadable(_)
            )
        })
        .collect();
    // Doctor says what is wrong with a browser, so a browser with two bad
    // keys gets one entry. It is kept only when every bad key is, since
    // setup rewrites any other one.
    let stale: Vec<Value> = browser_names(&bad, &|_| true)
        .into_iter()
        .filter_map(|name| {
            let mut of_browser = bad.iter().filter(|r| r.browser.name() == name);
            let first = of_browser.clone().next()?;
            Some(
                *of_browser
                    .find(|r| !matches!(r.found, Found::Kept { .. }))
                    .unwrap_or(first),
            )
        })
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
                // A build leaves it alone, so `devlanes setup` wouldn't fix it.
                "kept": matches!(registration.found, Found::Kept { .. }),
                // This install's own registration, only out of date.
                "ours": matches!(registration.found, Found::Stale { ours: true, .. }),
            }))
        })
        .collect();
    json!({
        "registered": registered_names(&registrations),
        "stale": stale,
        "found": browser_names(&registrations, &|found| *found != Found::NoBrowser),
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

    /// The browsers most tests set up, in [`Browser::ALL`]'s order.
    const FOUR: [Browser; 4] = [
        Browser::CHROME,
        Browser::CHROMIUM,
        Browser::EDGE,
        Browser::BRAVE,
    ];

    /// The `found` of each registration of [`FOUR`], in that order. On
    /// Windows Brave has two, for Chromium's key and Chrome's.
    fn founds(location: &Location, install: &Install) -> Vec<Found> {
        inspect(location, install)
            .into_iter()
            .filter(|registration| FOUR.contains(&registration.browser))
            .map(|registration| registration.found)
            .collect()
    }

    /// Browsers under a temporary `home` with `layout`. The registry root is
    /// a test key, so nothing touches `HKCU\Software` itself.
    fn location(home: &Path, layout: Layout, registry_root: String) -> Location {
        Location {
            home: home.to_path_buf(),
            local_app_data: home.join("AppData").join("Local"),
            layout,
            registry_root,
        }
    }

    /// Creates the user data folder of each of `browsers` with its
    /// `Local State`, as starting them once would.
    fn install_browsers(location: &Location, browsers: &[Browser]) {
        for browser in browsers {
            let folder = location.user_data(*browser);
            std::fs::create_dir_all(&folder).expect("user data folder");
            std::fs::write(folder.join("Local State"), "{}").expect("local state");
        }
    }

    /// On the Linux layout, setup writes a manifest into each browser folder
    /// that exists and refreshes a stale one. `--remove` deletes the
    /// manifests that run this install, including an outdated one, and
    /// leaves another install's in place. The macOS tests below cover Brave
    /// sharing Chrome's manifest there.
    #[test]
    fn linux_registers_refreshes_and_removes_only_this_install() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(&temp.path().join("user"), Layout::Linux, String::new());
        let install = script(&temp.path().join("bin").join("devlanes"));
        let manifest = |browser: Browser| location.unix_manifest(browser);
        install_browsers(&location, &[Browser::CHROME, Browser::BRAVE]);
        let gone = temp.path().join("gone").join("devlanes");
        std::fs::create_dir_all(manifest(Browser::BRAVE).parent().expect("parent"))
            .expect("hosts folder");
        std::fs::write(manifest(Browser::BRAVE), other_manifest(&gone)).expect("write");

        let registrations = inspect(&location, &install);
        assert_eq!(
            founds(&location, &install),
            [
                Found::Missing,
                Found::NoBrowser,
                Found::NoBrowser,
                Found::Stale {
                    runs: Some(gone.clone()),
                    ours: false
                },
            ]
        );
        // Every other browser on the list isn't installed.
        assert!(
            registrations
                .iter()
                .filter(|registration| !FOUR.contains(&registration.browser))
                .all(|registration| registration.found == Found::NoBrowser),
            "{registrations:?}"
        );
        let facts = Facts::Found {
            install: install.clone(),
            registrations: registrations.clone(),
            copy: copy_of(&location, &install),
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
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            lines[0],
            format!("wrote {}", location.copy().display()),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("registered the browser extension host for Chrome: "),
            "{lines:?}"
        );
        assert_eq!(
            std::fs::read_to_string(location.copy()).expect("copy"),
            std::fs::read_to_string(manifest(Browser::CHROME)).expect("manifest")
        );
        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(manifest(Browser::CHROME)).expect("read"),
        )
        .expect("json");
        assert_eq!(written, install.manifest());
        assert_eq!(
            written["allowed_origins"],
            json!(["chrome-extension://konofbnhgejkgkonkleipkkpkphbfgee/"])
        );
        assert_eq!(written["path"], json!(install.entry));
        assert_eq!(
            founds(&location, &install),
            [
                Found::Current,
                Found::NoBrowser,
                Found::NoBrowser,
                Found::Current
            ]
        );
        // Setup creates no folder for a browser that isn't installed.
        assert!(!location.user_data(Browser::EDGE).exists());
        assert!(
            plan(&Facts::Found {
                install: install.clone(),
                registrations: inspect(&location, &install),
                copy: copy_of(&location, &install),
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
            (Browser::EDGE, other_manifest(&other)),
            (Browser::CHROMIUM, other_manifest(&install.entry)),
        ] {
            std::fs::create_dir_all(manifest(browser).parent().expect("parent"))
                .expect("hosts folder");
            std::fs::write(manifest(browser), text).expect("write");
        }
        let report = report(&location, &install);
        assert_eq!(report["registered"], json!(["Chrome", "Brave"]), "{report}");
        assert_eq!(
            report["found"],
            json!(["Chrome", "Chromium", "Edge", "Brave"])
        );
        assert_eq!(report["stale"][1]["browser"], "Edge", "{report}");
        assert_eq!(report["stale"][1]["gone"], false, "{report}");
        assert_eq!(report["stale"][0]["ours"], true, "{report}");
        assert_eq!(report["stale"][1]["ours"], false, "{report}");
        // Doctor words this install's outdated manifest as its own, and
        // only Edge as another install.
        let lines: Vec<String> =
            devlanes_cli::doctor_checks_of(&json!({"browser": report.clone()}))
                .into_iter()
                .map(|check| check.text)
                .collect();
        assert!(
            lines.contains(&"Chromium's extension host is out of date".to_owned()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&format!(
                "Edge's extension host runs {}, another devlanes install",
                other.display()
            )),
            "{lines:?}"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("another devlanes install"))
                .count(),
            1,
            "{lines:?}"
        );

        let removal = removal(&location, &install);
        assert_eq!(
            removal.delete,
            [
                Target::File(manifest(Browser::CHROME)),
                Target::File(manifest(Browser::CHROMIUM)),
                Target::File(manifest(Browser::BRAVE)),
                Target::File(location.copy()),
            ]
        );
        assert_eq!(removal.keep.len(), 1, "{removal:?}");
        assert!(
            removal.keep[0].ends_with(&format!(
                "in place: it runs {}, which is another devlanes install. Run devlanes setup --remove with that install to delete it",
                other.display()
            )),
            "{removal:?}"
        );
        assert_eq!(delete(&removal).expect("delete").len(), 4);
        for browser in [Browser::CHROME, Browser::CHROMIUM, Browser::BRAVE] {
            assert!(!manifest(browser).exists(), "{browser:?}");
        }
        assert!(!location.copy().exists());
        assert_eq!(
            std::fs::read_to_string(manifest(Browser::EDGE)).expect("read"),
            other_manifest(&other)
        );
        assert!(super::removal(&location, &install).delete.is_empty());
    }

    /// devlanes 0.16.x wrote manifests that allow the extension's old id. A
    /// manifest of this install that allows another origin is stale.
    /// `--check` and doctor report it, and setup rewrites it.
    #[test]
    fn another_allowed_origin_is_refreshed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(&temp.path().join("user"), Layout::Linux, String::new());
        let install = script(&temp.path().join("bin").join("devlanes"));
        install_browsers(&location, &[Browser::CHROME]);
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        let mut old = install.manifest();
        // The only origin 0.16.x allowed.
        old["allowed_origins"] = json!(["chrome-extension://pggffblellmckocpflaaeligadjpkahj/"]);
        for file in [location.unix_manifest(Browser::CHROME), location.copy()] {
            std::fs::write(file, old.to_string()).expect("write");
        }

        let stale = Found::Stale {
            runs: Some(install.entry.clone()),
            ours: true,
        };
        assert_eq!(founds(&location, &install)[0], stale);
        assert_eq!(copy_of(&location, &install).found, stale);
        assert_eq!(
            facts_plan(&location, &install),
            (
                true,
                false,
                "setup refreshes the browser extension host for Chrome".into()
            )
        );
        let report = report(&location, &install);
        let lines: Vec<String> = devlanes_cli::doctor_checks_of(&json!({"browser": report}))
            .into_iter()
            .map(|check| check.text)
            .collect();
        assert!(
            lines.contains(&"Chrome's extension host is out of date".to_owned()),
            "{lines:?}"
        );
        assert_eq!(
            removal(&location, &install).delete,
            [
                Target::File(location.unix_manifest(Browser::CHROME)),
                Target::File(location.copy()),
            ]
        );

        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(founds(&location, &install)[0], Found::Current);
        assert_eq!(copy_of(&location, &install).found, Found::Current);
    }

    /// The manifest Brave reads on macOS, in Chrome's folder.
    fn chrome_folder_manifest(location: &Location) -> PathBuf {
        location
            .home
            .join("Library")
            .join("Application Support")
            .join("Google")
            .join("Chrome")
            .join("NativeMessagingHosts")
            .join(manifest_name())
    }

    /// Brave alone on macOS: Brave's folder, and the
    /// `Google/Chrome/NativeMessagingHosts` it creates on every start, or
    /// nothing in Chrome's folder when setup runs before Brave's first start.
    /// Setup writes the manifest into Chrome's folder, never into Brave's,
    /// and doesn't take Chrome for installed. `--remove` deletes it.
    #[test]
    fn macos_brave_alone_gets_chromes_manifest() {
        for brave_started in [true, false] {
            let temp = tempfile::tempdir().expect("tempdir");
            let location = location(&temp.path().join("user"), Layout::MacOs, String::new());
            install_browsers(&location, &[Browser::BRAVE]);
            let shared = chrome_folder_manifest(&location);
            if brave_started {
                std::fs::create_dir_all(shared.parent().expect("parent")).expect("brave's folder");
            }
            let install = script(&temp.path().join("bin").join("devlanes"));
            assert_eq!(location.unix_manifest(Browser::BRAVE), shared);
            assert_eq!(
                founds(&location, &install),
                [
                    Found::NoBrowser,
                    Found::NoBrowser,
                    Found::NoBrowser,
                    Found::Missing
                ],
                "{brave_started}"
            );
            assert_eq!(
                facts_plan(&location, &install).2,
                "setup registers the browser extension host for Brave"
            );

            let lines = apply(&location, &install, &inspect(&location, &install)).expect("apply");
            assert_eq!(
                lines[1],
                format!(
                    "registered the browser extension host for Brave: {}",
                    shared.display()
                )
            );
            assert_eq!(lines.len(), 2, "{lines:?}");
            let written: Value =
                serde_json::from_str(&std::fs::read_to_string(&shared).expect("read"))
                    .expect("json");
            assert_eq!(written, install.manifest());
            assert!(
                !location
                    .user_data(Browser::BRAVE)
                    .join("NativeMessagingHosts")
                    .exists()
            );
            // Chrome's folder now exists, and Chrome still isn't installed.
            assert_eq!(
                facts_plan(&location, &install),
                (
                    true,
                    true,
                    "the browser extension host is registered for Brave".into()
                )
            );
            let report = report(&location, &install);
            assert_eq!(report["registered"], json!(["Brave"]), "{report}");
            assert_eq!(report["found"], json!(["Brave"]), "{report}");

            // Another install's manifest there is what Brave starts.
            std::fs::write(&shared, other_manifest(&temp.path().join("gone"))).expect("write");
            assert_eq!(
                facts_plan(&location, &install).2,
                "setup refreshes the browser extension host for Brave"
            );
            apply(&location, &install, &inspect(&location, &install)).expect("apply");
            assert_eq!(
                removal(&location, &install).delete,
                [Target::File(shared.clone()), Target::File(location.copy())]
            );
            delete(&removal(&location, &install)).expect("delete");
            assert!(!shared.exists());
        }
    }

    /// Chrome and Brave on macOS read the same manifest: setup writes it once
    /// and names both, a stale one is stale for both, and `--remove` deletes
    /// it once.
    #[test]
    fn macos_chrome_and_brave_share_one_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(&temp.path().join("user"), Layout::MacOs, String::new());
        install_browsers(&location, &[Browser::CHROME, Browser::BRAVE]);
        let install = script(&temp.path().join("bin").join("devlanes"));
        let shared = chrome_folder_manifest(&location);
        assert_eq!(location.unix_manifest(Browser::CHROME), shared);
        assert_eq!(
            facts_plan(&location, &install).2,
            "setup registers the browser extension host for Chrome and Brave"
        );
        let lines = apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(
            lines[2].ends_with(&shared.display().to_string()),
            "{lines:?}"
        );
        assert_eq!(
            facts_plan(&location, &install).2,
            "the browser extension host is registered for Chrome and Brave"
        );

        let gone = temp.path().join("gone").join("devlanes");
        std::fs::write(&shared, other_manifest(&gone)).expect("write");
        let stale = Found::Stale {
            runs: Some(gone),
            ours: false,
        };
        assert_eq!(
            founds(&location, &install),
            [stale.clone(), Found::NoBrowser, Found::NoBrowser, stale]
        );
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(
            founds(&location, &install),
            [
                Found::Current,
                Found::NoBrowser,
                Found::NoBrowser,
                Found::Current
            ]
        );
        let removal = removal(&location, &install);
        assert_eq!(
            removal.delete,
            [Target::File(shared.clone()), Target::File(location.copy())]
        );
        assert_eq!(delete(&removal).expect("delete").len(), 2);
        assert!(!shared.exists());
    }

    /// Chrome alone on macOS is registered in its own folder, and Brave,
    /// which isn't installed, isn't named.
    #[test]
    fn macos_chrome_alone_is_registered_for_chrome() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(&temp.path().join("user"), Layout::MacOs, String::new());
        install_browsers(&location, &[Browser::CHROME]);
        let install = script(&temp.path().join("bin").join("devlanes"));
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert!(chrome_folder_manifest(&location).is_file());
        assert_eq!(
            facts_plan(&location, &install),
            (
                true,
                true,
                "the browser extension host is registered for Chrome".into()
            )
        );
        assert!(!location.user_data(Browser::BRAVE).exists());
    }

    /// Setup registers each installed browser on the list, channels
    /// included, and nothing for the others. Arc is on the list only on
    /// macOS, and Edge Canary has no Linux build.
    #[test]
    fn only_installed_browsers_on_the_list_are_registered() {
        for layout in [Layout::MacOs, Layout::Linux] {
            let temp = tempfile::tempdir().expect("tempdir");
            let location = location(&temp.path().join("user"), layout, String::new());
            let installed = [Browser::CHROME_CANARY, Browser::EDGE_DEV, Browser::VIVALDI];
            install_browsers(&location, &installed);
            let install = script(&temp.path().join("bin").join("devlanes"));
            let names: Vec<&str> = inspect(&location, &install)
                .iter()
                .map(|registration| registration.browser.name())
                .collect();
            assert_eq!(names.contains(&"Arc"), layout == Layout::MacOs, "{names:?}");
            assert_eq!(
                names.contains(&"Edge Canary"),
                layout == Layout::MacOs,
                "{names:?}"
            );
            let (needed, done, reason) = facts_plan(&location, &install);
            assert!(needed && !done);
            assert_eq!(
                reason,
                "setup registers the browser extension host for Chrome Canary, Edge Dev and Vivaldi"
            );
            apply(&location, &install, &inspect(&location, &install)).expect("apply");
            for browser in Browser::ALL {
                if browser.folder(layout).is_none() {
                    continue;
                }
                assert_eq!(
                    location.unix_manifest(browser).is_file(),
                    installed.contains(&browser),
                    "{browser:?}"
                );
                assert_eq!(
                    location.user_data(browser).exists(),
                    installed.contains(&browser),
                    "{browser:?}"
                );
            }
        }
        let folder = |browser: Browser, layout| {
            browser
                .folder(layout)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
        };
        assert_eq!(
            folder(Browser::CHROME_DEV, Layout::Linux).as_deref(),
            Some("google-chrome-unstable")
        );
        assert_eq!(
            folder(Browser::EDGE_CANARY, Layout::Windows).as_deref(),
            Some("Microsoft/Edge SxS/User Data")
        );
        assert_eq!(Browser::EDGE_CANARY.keys(), ["Microsoft/Edge"]);
        assert_eq!(folder(Browser::ARC, Layout::Windows), None);
    }

    /// A link to the manifest copy is this install's registration: setup
    /// keeps it current by writing the copy and leaves the link in place,
    /// `--remove` deletes the link and the copy, and a link whose copy is
    /// gone is reported and removed like a Windows key whose manifest is
    /// gone. A link the user made for a browser outside the list stays.
    #[cfg(unix)]
    #[test]
    fn links_to_the_copy_follow_it() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(&temp.path().join("user"), Layout::Linux, String::new());
        install_browsers(
            &location,
            &[Browser::CHROME, Browser::CHROMIUM, Browser::BRAVE],
        );
        let install = script(&temp.path().join("bin").join("devlanes"));
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        let copy = location.copy();
        let is_link = |path: &Path| {
            std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
        };
        // Brave links to the copy by its full path, Chromium relatively.
        let brave = location.unix_manifest(Browser::BRAVE);
        std::fs::remove_file(&brave).expect("remove");
        symlink(&copy, &brave).expect("link");
        let chromium = location.unix_manifest(Browser::CHROMIUM);
        std::fs::remove_file(&chromium).expect("remove");
        let relative: PathBuf = ["..", "..", "..", ".devlanes", "browser"]
            .iter()
            .collect::<PathBuf>()
            .join(manifest_name());
        symlink(&relative, &chromium).expect("link");
        // A browser outside the list, linked by hand.
        let other = location
            .home
            .join(".config")
            .join("thorium")
            .join("NativeMessagingHosts")
            .join(manifest_name());
        std::fs::create_dir_all(other.parent().expect("parent")).expect("folder");
        symlink(&copy, &other).expect("link");

        // An outdated copy makes both links outdated, and setup rewrites
        // only the copy.
        std::fs::write(&copy, other_manifest(&install.entry)).expect("outdated copy");
        let registrations = inspect(&location, &install);
        for registration in registrations.iter().filter(|r| r.linked) {
            assert_eq!(
                registration.found,
                Found::Stale {
                    runs: Some(install.entry.clone()),
                    ours: true
                }
            );
        }
        assert_eq!(registrations.iter().filter(|r| r.linked).count(), 2);
        apply(&location, &install, &registrations).expect("apply");
        assert!(is_link(&brave) && is_link(&chromium));
        assert_eq!(
            founds(&location, &install),
            [
                Found::Current,
                Found::Current,
                Found::NoBrowser,
                Found::Current
            ]
        );

        // The copy is gone: the links are devlanes' own to clean up.
        std::fs::remove_file(&copy).expect("delete copy");
        let report = report(&location, &install);
        assert_eq!(report["stale"][0]["browser"], "Chromium", "{report}");
        assert_eq!(report["stale"][0]["runs"], Value::Null, "{report}");
        assert_eq!(report["stale"][0]["ours"], true, "{report}");
        let removal = removal(&location, &install);
        assert_eq!(
            removal.delete,
            [Browser::CHROME, Browser::CHROMIUM, Browser::BRAVE]
                .map(|browser| Target::File(location.unix_manifest(browser)))
        );
        delete(&removal).expect("delete");
        assert!(!is_link(&brave) && !is_link(&chromium));
        assert!(is_link(&other), "a hand-made link is the user's");
    }

    /// When setup creates `~/.devlanes` for the manifest copy, only this user
    /// can reach it, as when the client creates the devlanes home.
    #[cfg(unix)]
    #[test]
    fn setup_creates_the_devlanes_folder_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(&temp.path().join("user"), Layout::Linux, String::new());
        std::fs::create_dir_all(&location.home).expect("home");
        let install = script(&temp.path().join("bin").join("devlanes"));
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert!(location.copy().is_file());
        let mode = std::fs::metadata(location.home.join(".devlanes"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }

    /// The plan of the facts at `location` for `install`.
    fn facts_plan(location: &Location, install: &Install) -> (bool, bool, String) {
        plan(&facts(Some(location), install.clone()))
    }

    /// uvx and pipx run installs are refused, and so is an entry that isn't
    /// an absolute path. Builds from a checkout get a note that the host
    /// runs that build.
    #[test]
    fn caches_are_refused_and_builds_are_named() {
        let temp = tempfile::tempdir().expect("tempdir");
        let location = location(temp.path(), Layout::Linux, String::new());
        install_browsers(&location, &[Browser::CHROMIUM]);
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
        assert!(
            reason.contains("as /full/path/to/devlanes setup"),
            "{reason}"
        );
        assert!(matches!(
            apply(&location, &relative, &inspect(&location, &relative)),
            Err(BrowserError::Refused(_))
        ));
        assert!(!location.unix_manifest(Browser::CHROMIUM).exists());
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
        let location = location(&temp.path().join("user"), Layout::Linux, String::new());
        let other = temp.path().join("opt").join("devlanes");
        std::fs::create_dir_all(other.parent().expect("parent")).expect("opt");
        std::fs::write(&other, "").expect("other binary");
        let gone = temp.path().join("gone").join("devlanes");
        for (browser, runs) in [(Browser::CHROME, &other), (Browser::BRAVE, &gone)] {
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
                copy: copy_of(&location, &install),
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
            let report = report(&location, &install);
            assert_eq!(report["stale"][0]["browser"], "Chrome", "{report}");
            assert_eq!(report["stale"][0]["kept"], true, "{report}");
            assert_eq!(report["stale"][1]["kept"], false, "{report}");
            let lines = apply(&location, &install, &registrations).expect("apply");
            let registered = lines
                .iter()
                .filter(|line| line.starts_with("registered"))
                .count();
            assert_eq!(registered, 1, "{lines:?}");
            assert_eq!(
                std::fs::read_to_string(location.unix_manifest(Browser::CHROME)).expect("read"),
                other_manifest(&other)
            );
            // Put Brave back for the next install.
            std::fs::write(
                location.unix_manifest(Browser::BRAVE),
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

    /// The keys under the test root that name a manifest, as `Google\Chrome`.
    #[cfg(windows)]
    fn written_keys(location: &Location) -> Vec<&'static str> {
        [
            "Google/Chrome",
            "Chromium",
            "Microsoft/Edge",
            "Vivaldi",
            "BraveSoftware/Brave-Browser",
        ]
        .into_iter()
        .filter(|key| {
            registry::read_string(&location.key(key), "")
                .expect("read")
                .is_some()
        })
        .collect()
    }

    /// On Windows, one manifest under `.devlanes\browser` and the keys the
    /// installed browsers read pointing at it, under a test key instead of
    /// `HKCU\Software`. Chrome Beta and Brave share Chrome's key, and
    /// Vivaldi, which isn't installed, gets none. `--remove` deletes this
    /// install's keys and manifest, and leaves a key that names another
    /// install's manifest.
    #[cfg(windows)]
    #[test]
    fn windows_registers_installed_browsers_and_removes_only_this_install() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-{}",
            std::process::id()
        ));
        let location = location(&temp.path().join("user"), Layout::Windows, root.0.clone());
        install_browsers(&location, &FOUR);
        install_browsers(&location, &[Browser::CHROME_BETA]);
        let install = script(&temp.path().join("bin").join("devlanes.exe"));
        // Brave has two keys.
        assert_eq!(founds(&location, &install), vec![Found::Missing; 5]);

        let lines = apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(lines.len(), 7, "{lines:?}");
        let file = location
            .home
            .join(".devlanes")
            .join("browser")
            .join("run.devlanes.browser.json");
        assert!(file.is_file());
        assert_eq!(
            written_keys(&location),
            ["Google/Chrome", "Chromium", "Microsoft/Edge"]
        );
        for key in written_keys(&location) {
            assert_eq!(
                registry::read_string(&location.key(key), "").expect("read"),
                Some(file.display().to_string())
            );
        }
        assert!(
            location
                .key("Google/Chrome")
                .ends_with(r"\Google\Chrome\NativeMessagingHosts\run.devlanes.browser")
        );
        assert_eq!(founds(&location, &install), vec![Found::Current; 5]);

        let other = temp.path().join("other.json");
        let other_exe = temp.path().join("other").join("devlanes.exe");
        std::fs::write(&other, other_manifest(&other_exe)).expect("other manifest");
        let edge = location.key("Microsoft/Edge");
        registry::write_string(&edge, "", &other.display().to_string()).expect("edge key");
        assert_eq!(
            founds(&location, &install)[2],
            Found::Stale {
                runs: Some(other_exe.clone()),
                ours: false
            }
        );
        let report = report(&location, &install);
        assert_eq!(report["stale"][0]["gone"], true, "{report}");

        // Chrome's and Chromium's keys once each, for every browser that
        // reads them, then the manifest.
        let removal = removal(&location, &install);
        assert_eq!(
            removal.delete,
            [
                Target::Key(location.key("Google/Chrome")),
                Target::Key(location.key("Chromium")),
                Target::File(file.clone()),
            ]
        );
        assert_eq!(removal.keep.len(), 1, "{removal:?}");
        delete(&removal).expect("delete");
        assert!(!file.exists());
        assert_eq!(written_keys(&location), ["Microsoft/Edge"]);
        assert_eq!(
            registry::read_string(&edge, "").expect("read"),
            Some(other.display().to_string())
        );
    }

    /// Brave on Windows reads Chromium's and Chrome's keys and never its own,
    /// and Vivaldi gets its own key next to those two. With only Brave
    /// installed, setup writes the two keys Brave reads and none under
    /// `BraveSoftware`. A Brave whose first key runs another install isn't
    /// registered, and `--remove` deletes every key setup wrote.
    #[cfg(windows)]
    #[test]
    fn windows_brave_and_vivaldi_get_the_keys_they_read() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-brave-{}",
            std::process::id()
        ));
        let location = location(&temp.path().join("user"), Layout::Windows, root.0.clone());
        install_browsers(&location, &[Browser::BRAVE]);
        let install = script(&temp.path().join("bin").join("devlanes.exe"));
        let check = || facts_plan(&location, &install);
        assert_eq!(
            check().2,
            "setup registers the browser extension host for Brave"
        );

        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(written_keys(&location), ["Google/Chrome", "Chromium"]);
        assert_eq!(
            check(),
            (
                true,
                true,
                "the browser extension host is registered for Brave".into()
            )
        );
        assert_eq!(report(&location, &install)["registered"], json!(["Brave"]));

        // Another install's manifest under Chromium's key, which Brave reads
        // first: Brave would start that one.
        let other = temp.path().join("other.json");
        let other_exe = temp.path().join("other").join("devlanes.exe");
        std::fs::write(&other, other_manifest(&other_exe)).expect("other manifest");
        registry::write_string(&location.key("Chromium"), "", &other.display().to_string())
            .expect("chromium key");
        assert_eq!(
            check().2,
            "setup refreshes the browser extension host for Brave"
        );
        let report = report(&location, &install);
        assert_eq!(report["registered"], json!([]), "{report}");
        assert_eq!(
            report["stale"].as_array().map(Vec::len),
            Some(1),
            "{report}"
        );
        // With Chrome's key gone too, Brave is still named once.
        registry::delete_tree(&location.key("Google/Chrome")).expect("delete chrome key");
        assert_eq!(
            check().2,
            "setup refreshes the browser extension host for Brave"
        );
        apply(&location, &install, &inspect(&location, &install)).expect("apply");

        install_browsers(&location, &[Browser::VIVALDI]);
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        assert_eq!(
            written_keys(&location),
            ["Google/Chrome", "Chromium", "Vivaldi"]
        );
        assert_eq!(
            check().2,
            "the browser extension host is registered for Brave and Vivaldi"
        );

        delete(&removal(&location, &install)).expect("delete");
        assert_eq!(written_keys(&location), Vec::<&str>::new());
        assert!(!location.copy().exists());
    }

    /// Doctor names Brave once when both its keys are wrong, and says a build
    /// leaves it alone only when it would leave both: here Chromium's key
    /// runs another working install, which a build keeps, but Chrome's runs
    /// a program that is gone, which setup rewrites.
    #[cfg(windows)]
    #[test]
    fn doctor_reports_a_browser_as_kept_only_when_every_bad_key_is() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-kept-{}",
            std::process::id()
        ));
        let location = location(&temp.path().join("user"), Layout::Windows, root.0.clone());
        install_browsers(&location, &[Browser::BRAVE]);
        let other = temp.path().join("other").join("devlanes.exe");
        std::fs::create_dir_all(other.parent().expect("parent")).expect("other");
        std::fs::write(&other, "").expect("other binary");
        let gone = temp.path().join("gone").join("devlanes.exe");
        for (key, runs, file) in [
            ("Chromium", &other, temp.path().join("other.json")),
            ("Google/Chrome", &gone, temp.path().join("gone.json")),
        ] {
            std::fs::write(&file, other_manifest(runs)).expect("manifest");
            registry::write_string(&location.key(key), "", &file.display().to_string())
                .expect("key");
        }
        let build = temp.path().join("target").join("devlanes.exe");
        let install = Install::new(InstallMethod::Source, build);
        let report = report(&location, &install);
        let stale = report["stale"].as_array().expect("stale");
        assert_eq!(stale.len(), 1, "{report}");
        assert_eq!(stale[0]["browser"], "Brave", "{report}");
        assert_eq!(stale[0]["kept"], false, "{report}");
        assert_eq!(stale[0]["gone"], true, "{report}");
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
        let location = location(&temp.path().join("user"), Layout::Windows, root.0.clone());
        install_browsers(&location, &FOUR);
        let other = temp.path().join("other").join("devlanes.exe");
        std::fs::create_dir_all(other.parent().expect("parent")).expect("other");
        std::fs::write(&other, "").expect("other binary");
        let shared = location.copy();
        std::fs::create_dir_all(shared.parent().expect("parent")).expect("browser folder");
        std::fs::write(&shared, other_manifest(&other)).expect("shared manifest");
        // Chrome's, Chromium's and so Brave's.
        for key in ["Google/Chrome", "Chromium"] {
            registry::write_string(&location.key(key), "", &shared.display().to_string())
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
            copy,
            ..
        } = &facts
        else {
            panic!("{facts:?}");
        };
        assert_eq!(copy.path, shared);
        assert_eq!(copy.held(), Some(other.as_path()));
        let (needed, done, reason) = plan(&facts);
        assert!(!needed && !done, "{reason}");
        assert!(
            reason.contains(&format!(
                "Edge can't be registered: the shared manifest {} runs {}, another devlanes install, and a build doesn't replace it. To register this build",
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
            registry::read_string(&location.key("Microsoft/Edge"), "").expect("read"),
            None
        );
    }

    /// After `.devlanes` was deleted, the keys still name the file setup
    /// writes there. They are devlanes' own, so `--remove` deletes them, also
    /// one for a browser that is no longer installed. A key that names a
    /// missing file elsewhere stays.
    #[cfg(windows)]
    #[test]
    fn windows_removes_its_keys_once_the_manifest_is_gone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = TestKey(format!(
            r"Software\devlanes-test-browser-gone-{}",
            std::process::id()
        ));
        let location = location(&temp.path().join("user"), Layout::Windows, root.0.clone());
        install_browsers(&location, &FOUR);
        let install = script(&temp.path().join("bin").join("devlanes.exe"));
        apply(&location, &install, &inspect(&location, &install)).expect("apply");
        // As if Vivaldi was registered, then uninstalled.
        let vivaldi = location.key("Vivaldi");
        registry::write_string(&vivaldi, "", &location.copy().display().to_string())
            .expect("vivaldi key");
        std::fs::remove_dir_all(location.home.join(".devlanes")).expect("delete .devlanes");
        let elsewhere = temp.path().join("missing.json");
        let edge = location.key("Microsoft/Edge");
        registry::write_string(&edge, "", &elsewhere.display().to_string()).expect("edge key");
        let ours = Found::Stale {
            runs: None,
            ours: true,
        };
        let found = founds(&location, &install);
        assert_eq!(
            [&found[0], &found[1], &found[3], &found[4]],
            [&ours; 4],
            "{found:?}"
        );
        let removal = removal(&location, &install);
        assert_eq!(
            removal.delete,
            ["Google/Chrome", "Chromium", "Vivaldi"].map(|key| Target::Key(location.key(key)))
        );
        assert_eq!(removal.keep.len(), 1, "{removal:?}");
        assert!(
            removal.keep[0].contains("the manifest it names is missing"),
            "{removal:?}"
        );
        delete(&removal).expect("delete");
        assert_eq!(written_keys(&location), ["Microsoft/Edge"]);
    }
}
