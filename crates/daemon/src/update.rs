//! Release checks and background installs. The daemon reads the latest
//! version from the registry it was installed from (`devlanes_update::check`)
//! 30 seconds after it starts, then every 4 hours and when a TUI opens
//! (`update.check`), and caches the answer with the release notes in
//! `$DEVLANES_HOME/update-check.json`. `daemon.status` and `status` report a
//! newer release. With `updates.mode: auto`, a patch release is installed in
//! the background: script installs download and stage it for the next
//! interactive `devlanes`, package managers install it. Nothing restarts.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use devlanes_config::{UpdateChannel, UpdateMode};
use devlanes_proto::{
    RpcError,
    release::{ReleaseCheck, UpdateCheckParams, UpdateInfo, UpdateNote},
};
use devlanes_update::{
    InstallMethod,
    check::{Channel, Source, Urls},
    install::{self, InstallError, Lock, Plan, Release, Staged},
    notes,
    version::{Bump, Version, bump, is_newer},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::Inner;

/// The cache file under `$DEVLANES_HOME`.
const CACHE_FILE: &str = "update-check.json";

/// When and where the daemon looks for releases.
#[derive(Debug, Clone)]
pub struct UpdateCheckOptions {
    /// The registries and the changelog; tests point them at a local server.
    pub urls: Urls,
    /// How devlanes was installed, which picks the registry; `None` reads it
    /// from the daemon's executable. Tests set one, since a test binary counts
    /// as a source build, which has nothing to check.
    pub method: Option<InstallMethod>,
    /// Why checks are off for this daemon (`debug_build`, `ci`, `env`), or `None`.
    pub off: Option<&'static str>,
    /// Wait after start before the first check, so it never competes with startup.
    pub first_delay: Duration,
    /// Time between background checks.
    pub interval: Duration,
    /// How old the last check must be before `update.check` without `force`
    /// checks again.
    pub recheck_after: Duration,
    /// Limit for one request, redirects included.
    pub timeout: Duration,
    /// Whether a package manager may replace the daemon's executable while
    /// it runs ([`devlanes_update::own_exe_retained`]); without it, Windows
    /// package-manager installs are left to a person.
    pub exe_retained: bool,
}

impl UpdateCheckOptions {
    /// Production timing and registries; off in debug builds, under CI markers
    /// and with `DEVLANES_NO_UPDATE_CHECK` set to a true value. A debug build
    /// with `DEVLANES_UPDATE_BASE_URL` set checks the releases there.
    pub fn from_env() -> Self {
        let off = if cfg!(debug_assertions) && devlanes_update::check::base_override().is_none() {
            Some("debug_build")
        } else if ci_detected(|name| std::env::var(name).ok()) {
            Some("ci")
        } else if std::env::var("DEVLANES_NO_UPDATE_CHECK")
            .ok()
            .as_deref()
            .is_some_and(truthy)
        {
            Some("env")
        } else {
            None
        };
        Self {
            urls: Urls::from_env(),
            method: None,
            off,
            first_delay: Duration::from_secs(30),
            interval: Duration::from_secs(4 * 3600),
            recheck_after: Duration::from_secs(3600),
            timeout: Duration::from_secs(10),
            exe_retained: devlanes_update::own_exe_retained(),
        }
    }
}

/// The CI markers the CLI's output-mode detection uses. Under CI the daemon
/// skips release checks, and the binary sets [`crate::EngineOptions::ci`] so a
/// checkout takes lane 0 on any branch.
pub fn ci_detected(lookup: impl Fn(&str) -> Option<String>) -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "BUILDKITE",
        "CIRCLECI",
        "TF_BUILD",
        "JENKINS_URL",
        "TEAMCITY_VERSION",
        "BITBUCKET_BUILD_NUMBER",
    ]
    .into_iter()
    .any(|name| lookup(name).as_deref().is_some_and(truthy))
}

/// Empty, `0` and `false` count as unset.
fn truthy(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
}

/// What the last successful check found, as stored in the cache file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cached {
    /// Milliseconds since the epoch.
    pub checked_at: u64,
    pub latest: String,
    /// The channel and source the check read; a cache of another one is
    /// ignored after the config or the install changed.
    pub channel: String,
    pub source: String,
    /// The `latest` that `notes` were read for, so the changelog is fetched
    /// once per release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_for: Option<String>,
    /// Changelog entries after the version that read them, up to `notes_for`.
    #[serde(default)]
    pub notes: Vec<UpdateNote>,
}

/// What the background install is at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Background {
    /// Nothing ran, or it gave way to a `devlanes update`.
    #[default]
    Idle,
    /// Installing this version now.
    Installing(String),
    /// This version is installed, or staged for the next interactive start.
    Ready(String),
    /// Installing `version` failed with `error`; the next scheduled or
    /// forced check tries again.
    Failed { version: String, error: String },
}

impl Background {
    /// The state a background install task ended in for `version`: `Ready`
    /// with what it installed, `Idle` when a `devlanes update` held the lock
    /// (it installs the release itself), `Failed` otherwise, a panic too.
    fn after(
        version: &str,
        ended: Result<Result<String, InstallError>, tokio::task::JoinError>,
    ) -> Self {
        let error = match ended {
            Ok(Ok(installed)) => return Self::Ready(installed),
            Ok(Err(InstallError::Busy)) => return Self::Idle,
            Ok(Err(error)) => error.to_string(),
            Err(error) => format!("the install stopped: {error}"),
        };
        tracing::info!(%error, version, "background update failed");
        Self::Failed {
            version: version.to_owned(),
            error,
        }
    }
}

/// The daemon's view of the latest release.
#[derive(Debug, Default)]
pub(crate) struct UpdateState {
    /// How this daemon's executable was installed, and its path; read once.
    method: tokio::sync::OnceCell<(InstallMethod, PathBuf)>,
    /// The source for this install, or why it has none; read once.
    source: tokio::sync::OnceCell<Result<Source, &'static str>>,
    cached: Mutex<Option<Cached>>,
    /// One check at a time: the background loop and `update.check` share it.
    checking: tokio::sync::Mutex<()>,
    /// Told after an `update.check` with `install` read the registry, so the
    /// loop can install what it found.
    checked: tokio::sync::Notify,
    /// Set with `checked` by a forced `update.check`, which tries a failed
    /// install of the same release again.
    retry: AtomicBool,
    background: Mutex<Background>,
}

impl UpdateState {
    /// The background install's state.
    fn background(&self) -> Background {
        self.background
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replaces the background install's state.
    fn set_background(&self, state: Background) {
        *self.background.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }

    /// The cached result.
    fn get(&self) -> Option<Cached> {
        self.cached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replaces the cached result.
    fn set(&self, cached: Cached) {
        *self.cached.lock().unwrap_or_else(|e| e.into_inner()) = Some(cached);
    }

    /// Takes the cache file's result unless a check (an `update.check` that
    /// came before the file was read) already gave a newer one.
    fn load(&self, cached: Cached) {
        self.cached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(cached);
    }
}

impl Inner {
    /// Why checks are off from the daemon's environment, the user config or
    /// the install (once known), or `None`.
    fn update_off(&self) -> Option<&'static str> {
        self.opts
            .update_check
            .off
            .or((self.opts.user.updates.mode == UpdateMode::Disable).then_some("config"))
            .or_else(|| match self.update.source.get() {
                Some(Err(reason)) => Some(*reason),
                _ => None,
            })
    }

    /// The channel the config asks for, else the running build's.
    fn update_channel(&self) -> Channel {
        match self.opts.user.updates.channel {
            Some(UpdateChannel::Stable) => Channel::Stable,
            Some(UpdateChannel::Next) => Channel::Next,
            None => Channel::of_version(devlanes_version::VERSION),
        }
    }

    /// How this daemon was installed and the executable it runs, read from
    /// the filesystem on first use.
    async fn install_method(&self) -> &(InstallMethod, PathBuf) {
        let configured = self.opts.update_check.method.clone();
        self.update
            .method
            .get_or_init(|| async move {
                tokio::task::spawn_blocking(move || {
                    let exe = devlanes_update::running_exe()
                        .unwrap_or_else(|_| PathBuf::from("devlanes"));
                    (configured.unwrap_or_else(InstallMethod::current), exe)
                })
                .await
                .unwrap_or((InstallMethod::Source, PathBuf::from("devlanes")))
            })
            .await
    }

    /// Where this install reads the latest version, or why checks are off.
    async fn update_source(&self) -> Result<Source, &'static str> {
        if let Some(reason) = self.update_off() {
            return Err(reason);
        }
        let channel = self.update_channel();
        let (method, _) = self.install_method().await;
        *self
            .update
            .source
            .get_or_init(|| async move { Source::for_method(method, channel) })
            .await
    }

    /// The newer release to report, if any, with the background install's
    /// state for it.
    pub(crate) fn update_report(&self) -> Option<UpdateInfo> {
        if self.update_off().is_some() {
            return None;
        }
        let cached = self.update.get()?;
        let running = devlanes_version::VERSION;
        let bump = bump(running, &cached.latest)?;
        let notes = cached
            .notes
            .into_iter()
            .filter(|note| {
                is_newer(&note.version, running) && !is_newer(&note.version, &cached.latest)
            })
            .collect();
        let (state, installed, error) = match self.update.background() {
            Background::Installing(version) if version == cached.latest => {
                ("installing", None, None)
            }
            Background::Ready(version) if is_newer(&version, running) => {
                ("ready", Some(version), None)
            }
            Background::Failed { version, error } if version == cached.latest => {
                ("failed", None, Some(error))
            }
            _ => ("available", None, None),
        };
        Some(UpdateInfo {
            latest: cached.latest,
            channel: cached.channel,
            bump: bump.as_str().into(),
            state: state.into(),
            installed,
            error,
            checked_at: cached.checked_at,
            notes,
        })
    }

    /// The check's state for `doctor`.
    pub(crate) fn release_check(&self) -> ReleaseCheck {
        let mode = self.opts.user.updates.mode.as_str().to_owned();
        if let Some(reason) = self.update_off() {
            return ReleaseCheck {
                state: "off".into(),
                reason: Some(reason.into()),
                mode,
                ..ReleaseCheck::default()
            };
        }
        let Some(Ok(source)) = self.update.source.get() else {
            return ReleaseCheck {
                state: "pending".into(),
                mode,
                ..ReleaseCheck::default()
            };
        };
        let base = ReleaseCheck {
            state: "pending".into(),
            mode,
            channel: Some(source.channel().as_str().into()),
            source: Some(source.name().into()),
            ..ReleaseCheck::default()
        };
        match self.update.get() {
            Some(cached) => ReleaseCheck {
                state: "ok".into(),
                bump: bump(devlanes_version::VERSION, &cached.latest).map(|b| b.as_str().into()),
                latest: Some(cached.latest),
                checked_at: Some(cached.checked_at),
                ..base
            },
            None => base,
        }
    }

    /// Adds `update` to an object result when a newer release is reported.
    pub(crate) fn with_update(&self, mut result: Value) -> Value {
        if let (Some(update), Some(map)) = (self.update_report(), result.as_object_mut())
            && let Ok(update) = serde_json::to_value(update)
        {
            map.insert("update".into(), update);
        }
        result
    }

    /// `update.check`: checks now when forced or when the last check is older
    /// than [`UpdateCheckOptions::recheck_after`], then replies with the newer
    /// release, or `null`. A forced check that cannot read the registry fails
    /// with `update_failed`, so `devlanes update` never takes a stale answer
    /// for the latest. Only a check that read the registry, with `install`,
    /// wakes the background install, and only a forced one retries a failed
    /// install of the same release, so opening a TUI never repeats a failure.
    pub(crate) async fn update_check_method(&self, params: Value) -> Result<Value, RpcError> {
        let UpdateCheckParams { force, install } = crate::engine::params(params)?;
        if let Ok(source) = self.update_source().await {
            let recheck = self.opts.update_check.recheck_after;
            match self
                .check_release(source, (!force).then_some(recheck))
                .await
            {
                Checked::Read if install => {
                    if force {
                        self.update.retry.store(true, Ordering::Relaxed);
                    }
                    self.update.checked.notify_one();
                }
                Checked::Failed(error) if force => {
                    return Err(RpcError::new(
                        "update_failed",
                        serde_json::json!({
                            "title": format!(
                                "cannot read the latest {} release: {error}",
                                source.channel().as_str()
                            ),
                            "hint": "retry with devlanes update",
                        }),
                        1,
                    ));
                }
                _ => {}
            }
        }
        Ok(self
            .update_report()
            .and_then(|update| serde_json::to_value(update).ok())
            .unwrap_or(Value::Null))
    }

    /// Loads the cache and a release staged before a restart, then checks
    /// after the first delay and every interval after the last check, and
    /// installs what a check found when the mode says so. Returns at once
    /// when checks are off.
    pub(crate) async fn update_check_loop(self: Arc<Self>) {
        let Ok(source) = self.update_source().await else {
            return;
        };
        let (path, home) = (self.opts.home.join(CACHE_FILE), self.opts.home.clone());
        let (cached, staged) =
            tokio::task::spawn_blocking(move || (read_cache(&path, source), Staged::load(&home)))
                .await
                .unwrap_or_default();
        if let Some(cached) = cached {
            self.update.load(cached);
        }
        if let Some(staged) = staged.filter(|s| is_newer(&s.version, devlanes_version::VERSION)) {
            self.update
                .set_background(Background::Ready(staged.version));
        }
        let options = self.opts.update_check.clone();
        let mut wait = options.first_delay;
        loop {
            // A recent answer, from before a restart or from `update.check`,
            // moves the next check.
            wait = wait.max(self.until_due(options.interval));
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
                () = self.update.checked.notified() => {
                    let retry = self.update.retry.swap(false, Ordering::Relaxed);
                    self.clone().install_in_background(retry);
                    wait = Duration::ZERO;
                    continue;
                }
            }
            if !self.until_due(options.interval).is_zero() {
                wait = Duration::ZERO;
                continue;
            }
            if self.check_release(source, None).await == Checked::Read {
                self.clone().install_in_background(true);
            }
            wait = options.interval;
        }
    }

    /// Starts installing the latest release in the background when
    /// [`installs_in_background`] says so and nothing installed it yet. A
    /// failed install of the same release runs again only with `retry`: at
    /// a scheduled check or a forced `update.check`.
    fn install_in_background(self: Arc<Self>, retry: bool) {
        let Some(cached) = self.update.get() else {
            return;
        };
        let latest = cached.latest;
        let Some(bump) = bump(devlanes_version::VERSION, &latest) else {
            return;
        };
        let mode = self.opts.user.updates.mode;
        let skip = match self.update.background() {
            Background::Installing(_) => true,
            Background::Ready(version) => !is_newer(&latest, &version),
            Background::Failed { version, .. } => version == latest && !retry,
            Background::Idle => false,
        };
        if skip || mode != UpdateMode::Auto || bump != Bump::Patch {
            return;
        }
        let channel = source_channel(&cached.channel);
        let retained = self.opts.update_check.exe_retained;
        tokio::spawn(async move {
            let (method, exe) = self.install_method().await.clone();
            let version = latest.clone();
            let plan =
                tokio::task::spawn_blocking(move || Plan::new(&method, &exe, &version, retained))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|plan| plan.map_err(|error| error.to_string()));
            let Ok(plan) = plan else {
                return;
            };
            if !installs_in_background(mode, bump, &plan) {
                return;
            }
            self.update
                .set_background(Background::Installing(latest.clone()));
            // Its own task, so a panic ends as `Failed` instead of leaving
            // `Installing` behind.
            let task = {
                let (this, latest) = (self.clone(), latest.clone());
                tokio::spawn(async move { this.install_release(&plan, &latest, channel).await })
            };
            self.update
                .set_background(Background::after(&latest, task.await));
        });
    }

    /// Runs `plan` for `version` under the update lock: stages the release
    /// archive, or runs the package manager with its output in
    /// `logs/update.log`. Returns the version now installed or staged.
    async fn install_release(
        &self,
        plan: &Plan,
        version: &str,
        channel: Channel,
    ) -> Result<String, InstallError> {
        let home = self.opts.home.clone();
        let _lock = tokio::task::spawn_blocking(move || Lock::acquire(&home, Duration::ZERO))
            .await
            .map_err(|error| InstallError::Io {
                what: "update lock".into(),
                source: std::io::Error::other(error),
            })??;
        match plan {
            Plan::Archive { dir } => {
                let release = match channel {
                    Channel::Next => Release::Next,
                    Channel::Stable => Release::Version(version.to_owned()),
                };
                let options = &self.opts.update_check;
                let staged = install::stage(
                    &self.opts.home,
                    &options.urls,
                    &release,
                    dir,
                    options.timeout,
                )
                .await?;
                Ok(staged.version)
            }
            Plan::PackageManager(commands) => {
                install::run_package_manager(commands, &self.opts.logs.join("update.log")).await?;
                Ok(version.to_owned())
            }
            Plan::Manual { command } => Err(InstallError::Manual {
                command: command.clone(),
            }),
        }
    }

    /// The time until the cached answer is `interval` old; zero without one.
    fn until_due(&self, interval: Duration) -> Duration {
        let Some(cached) = self.update.get() else {
            return Duration::ZERO;
        };
        let due = cached
            .checked_at
            .saturating_add(u64::try_from(interval.as_millis()).unwrap_or(u64::MAX));
        Duration::from_millis(due.saturating_sub(crate::units::now_ms()))
    }

    /// Reads the latest version from `source`, and the release notes once per
    /// new stable release, then caches both on disk and in memory. With
    /// `fresh`, a cached answer younger than that is kept instead. A failure
    /// keeps the cached answer.
    async fn check_release(&self, source: Source, fresh: Option<Duration>) -> Checked {
        let _checking = self.update.checking.lock().await;
        if let Some(fresh) = fresh
            && !self.until_due(fresh).is_zero()
        {
            return Checked::Fresh;
        }
        let options = &self.opts.update_check;
        let latest =
            match devlanes_update::check::latest(source, &options.urls, options.timeout).await {
                Ok(latest) => latest,
                Err(error) => {
                    tracing::debug!(%error, source = source.name(), "release check failed");
                    return Checked::Failed(error.to_string());
                }
            };
        let previous = self
            .update
            .get()
            .filter(|previous| previous.notes_for.as_deref() == Some(latest.as_str()));
        // A `next` build has no tag, so no changelog to read at it.
        let tagged = Version::parse(&latest).is_some_and(|version| !version.is_prerelease());
        let (notes_for, notes) = match previous {
            Some(previous) => (previous.notes_for, previous.notes),
            None if tagged && is_newer(&latest, devlanes_version::VERSION) => {
                match devlanes_update::check::release_notes(&options.urls, &latest, options.timeout)
                    .await
                {
                    Ok(releases) => (Some(latest.clone()), note_lines(&releases, &latest)),
                    // Read again at the next check.
                    Err(error) => {
                        tracing::debug!(%error, "release notes not read");
                        (None, Vec::new())
                    }
                }
            }
            None => (None, Vec::new()),
        };
        let cached = Cached {
            checked_at: crate::units::now_ms(),
            latest,
            channel: source.channel().as_str().into(),
            source: source.name().into(),
            notes_for,
            notes,
        };
        // Cached on disk before it is reported, so a status that shows the
        // release also survives a restart.
        let path = self.opts.home.join(CACHE_FILE);
        let written_cached = cached.clone();
        let written = tokio::task::spawn_blocking(move || write_cache(&path, &written_cached))
            .await
            .map_err(std::io::Error::other)
            .and_then(|result| result);
        if let Err(error) = written {
            tracing::debug!(%error, "release check cache not written");
        }
        self.update.set(cached);
        Checked::Read
    }
}

/// How [`Inner::check_release`] ended.
#[derive(Debug, PartialEq, Eq)]
enum Checked {
    /// It read the registry and cached the answer.
    Read,
    /// The cached answer was recent enough, so it read nothing.
    Fresh,
    /// The registry could not be read, for this reason.
    Failed(String),
}

/// Whether the daemon installs a release `bump` away in the background under
/// `mode` with `plan`: only `auto` mode, only patch releases (a minor one may
/// break what worked, so a person decides), and only what devlanes can
/// install itself.
fn installs_in_background(mode: UpdateMode, bump: Bump, plan: &Plan) -> bool {
    mode == UpdateMode::Auto && bump == Bump::Patch && !matches!(plan, Plan::Manual { .. })
}

/// The channel a cache entry names.
fn source_channel(channel: &str) -> Channel {
    if channel == Channel::Next.as_str() {
        Channel::Next
    } else {
        Channel::Stable
    }
}

/// The entries of the releases after this daemon's version up to `latest`,
/// newest release first.
fn note_lines(releases: &[notes::Release], latest: &str) -> Vec<UpdateNote> {
    notes::between(releases, devlanes_version::VERSION, latest)
        .into_iter()
        .flat_map(|release| {
            release.entries.iter().map(|entry| UpdateNote {
                version: release.version.clone(),
                level: entry.level.as_str().into(),
                line: entry.line.clone(),
            })
        })
        .collect()
}

/// Reads the cache file; a missing or unreadable file, or one written for
/// another source or channel, is no cache.
fn read_cache(path: &Path, source: Source) -> Option<Cached> {
    let text = std::fs::read_to_string(path).ok()?;
    let cached: Cached = serde_json::from_str(&text).ok()?;
    let matches = cached.source == source.name() && cached.channel == source.channel().as_str();
    (matches && Version::parse(&cached.latest).is_some()).then_some(cached)
}

/// Writes the cache through a temporary file and a rename.
fn write_cache(path: &Path, cached: &Cached) -> std::io::Result<()> {
    let text = serde_json::to_string(cached).map_err(std::io::Error::other)?;
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&temporary, text)?;
    crate::files::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mode x bump x method: only `auto` installs, only patch releases, only
    /// for methods devlanes installs itself.
    #[test]
    fn background_installs_decision_table() {
        let archive = Plan::Archive {
            dir: PathBuf::from("/opt/acme/bin"),
        };
        let npm = Plan::PackageManager(vec![install::Argv {
            program: PathBuf::from("npm"),
            args: vec!["install".into(), "-g".into(), "devlanes@0.9.1".into()],
        }]);
        let manual = Plan::Manual {
            command: "cargo install devlanes".into(),
        };
        for (mode, bump, plan, expected) in [
            (UpdateMode::Auto, Bump::Patch, &archive, true),
            (UpdateMode::Auto, Bump::Patch, &npm, true),
            (UpdateMode::Auto, Bump::Patch, &manual, false),
            (UpdateMode::Auto, Bump::Minor, &archive, false),
            (UpdateMode::Auto, Bump::Minor, &npm, false),
            (UpdateMode::Notify, Bump::Patch, &archive, false),
            (UpdateMode::Notify, Bump::Patch, &npm, false),
            (UpdateMode::Disable, Bump::Patch, &archive, false),
        ] {
            assert_eq!(
                installs_in_background(mode, bump, plan),
                expected,
                "{mode:?} {bump:?} {plan:?}"
            );
        }
        assert_eq!(source_channel("next"), Channel::Next);
        assert_eq!(source_channel("stable"), Channel::Stable);
    }

    /// A background install ends ready, idle when a `devlanes update` took
    /// over, or failed, a panic included, so `installing` never stays.
    #[tokio::test]
    async fn a_background_install_always_ends() {
        let ended = |result: Result<String, InstallError>| Ok(result);
        assert_eq!(
            Background::after("0.9.1", ended(Ok("0.9.1".into()))),
            Background::Ready("0.9.1".into())
        );
        assert_eq!(
            Background::after("0.9.1", ended(Err(InstallError::Busy))),
            Background::Idle
        );
        assert!(matches!(
            Background::after("0.9.1", ended(Err(InstallError::Release("bad".into())))),
            Background::Failed { version, error } if version == "0.9.1" && error == "bad"
        ));
        let panicked = tokio::spawn(async {
            if true {
                panic!("install task");
            }
            Ok::<String, InstallError>(String::new())
        })
        .await;
        assert!(matches!(
            Background::after("0.9.1", panicked),
            Background::Failed { version, error } if version == "0.9.1" && error.contains("panic")
        ));
    }

    #[test]
    fn ci_markers() {
        assert!(ci_detected(|name| (name == "CI").then(|| "true".into())));
        assert!(ci_detected(
            |name| (name == "GITHUB_ACTIONS").then(|| "true".into())
        ));
        assert!(!ci_detected(|name| (name == "CI").then(|| "false".into())));
        assert!(!ci_detected(|_| None));
    }

    #[test]
    fn cache_round_trip_and_rejects_junk_and_other_sources() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(CACHE_FILE);
        let cached = Cached {
            checked_at: 5,
            latest: "0.3.0".into(),
            channel: "stable".into(),
            source: "npm".into(),
            notes_for: Some("0.3.0".into()),
            notes: vec![UpdateNote {
                version: "0.3.0".into(),
                level: "patch".into(),
                line: "Fixed a thing.".into(),
            }],
        };
        write_cache(&path, &cached)?;
        assert_eq!(read_cache(&path, Source::Npm), Some(cached));
        assert_eq!(read_cache(&path, Source::PyPi), None);
        assert_eq!(read_cache(&path, Source::GitHub(Channel::Stable)), None);
        std::fs::write(
            &path,
            r#"{"checked_at":1,"latest":"junk","channel":"stable","source":"npm"}"#,
        )?;
        assert_eq!(read_cache(&path, Source::Npm), None);
        std::fs::write(&path, "not json")?;
        assert_eq!(read_cache(&path, Source::Npm), None);
        Ok(())
    }

    #[test]
    fn notes_cover_releases_after_the_running_version() {
        let releases = vec![
            notes::Release {
                version: "99.1.0".into(),
                entries: vec![notes::Entry {
                    level: notes::Level::Minor,
                    line: "Removed a flag.".into(),
                    body: String::new(),
                }],
            },
            notes::Release {
                version: "99.0.0".into(),
                entries: vec![notes::Entry {
                    level: notes::Level::Patch,
                    line: "Fixed a thing.".into(),
                    body: "More.".into(),
                }],
            },
            notes::Release {
                version: "0.0.1".into(),
                entries: vec![notes::Entry {
                    level: notes::Level::Patch,
                    line: "Old.".into(),
                    body: String::new(),
                }],
            },
        ];
        let lines = note_lines(&releases, "99.0.0");
        assert_eq!(
            lines,
            [UpdateNote {
                version: "99.0.0".into(),
                level: "patch".into(),
                line: "Fixed a thing.".into(),
            }]
        );
        assert_eq!(note_lines(&releases, "99.1.0").len(), 2);
    }
}
