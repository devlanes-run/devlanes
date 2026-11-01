//! Release checks. The daemon reads the latest version from the registry it
//! was installed from (`devlanes_update::check`) 30 seconds after it starts,
//! then every 4 hours and when a TUI opens (`update.check`), and caches the
//! answer with the release notes in `$DEVLANES_HOME/update-check.json`.
//! `daemon.status` and `status` report a newer release. Nothing is downloaded
//! or installed, and nothing restarts.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    notes,
    version::{Version, bump, is_newer},
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
}

impl UpdateCheckOptions {
    /// Production timing and registries; off in debug builds, under CI markers
    /// and with `DEVLANES_NO_UPDATE_CHECK` set to a true value.
    pub fn from_env() -> Self {
        let off = if cfg!(debug_assertions) {
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
            urls: Urls::default(),
            method: None,
            off,
            first_delay: Duration::from_secs(30),
            interval: Duration::from_secs(4 * 3600),
            recheck_after: Duration::from_secs(3600),
            timeout: Duration::from_secs(10),
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

/// The daemon's view of the latest release.
#[derive(Debug, Default)]
pub(crate) struct UpdateState {
    /// The source for this install, or why it has none; read once.
    source: tokio::sync::OnceCell<Result<Source, &'static str>>,
    cached: Mutex<Option<Cached>>,
    /// One check at a time: the background loop and `update.check` share it.
    checking: tokio::sync::Mutex<()>,
}

impl UpdateState {
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

    /// Where this install reads the latest version, or why checks are off.
    /// The install method is read from the filesystem on first use.
    async fn update_source(&self) -> Result<Source, &'static str> {
        if let Some(reason) = self.update_off() {
            return Err(reason);
        }
        let channel = self.update_channel();
        let configured = self.opts.update_check.method.clone();
        *self
            .update
            .source
            .get_or_init(|| async move {
                let method = match configured {
                    Some(method) => method,
                    None => tokio::task::spawn_blocking(InstallMethod::current)
                        .await
                        .unwrap_or(InstallMethod::Source),
                };
                Source::for_method(&method, channel)
            })
            .await
    }

    /// The newer release to report, if any.
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
        Some(UpdateInfo {
            latest: cached.latest,
            channel: cached.channel,
            bump: bump.as_str().into(),
            state: "available".into(),
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
    /// release, or `null`.
    pub(crate) async fn update_check_method(&self, params: Value) -> Result<Value, RpcError> {
        let UpdateCheckParams { force } = crate::engine::params(params)?;
        if let Ok(source) = self.update_source().await {
            let recheck = self.opts.update_check.recheck_after;
            self.check_release(source, (!force).then_some(recheck))
                .await;
        }
        Ok(self
            .update_report()
            .and_then(|update| serde_json::to_value(update).ok())
            .unwrap_or(Value::Null))
    }

    /// Loads the cache, then checks after the first delay and every interval
    /// after the last check. Returns at once when checks are off.
    pub(crate) async fn update_check_loop(self: Arc<Self>) {
        let Ok(source) = self.update_source().await else {
            return;
        };
        let path = self.opts.home.join(CACHE_FILE);
        let cached = tokio::task::spawn_blocking(move || read_cache(&path, source))
            .await
            .ok()
            .flatten();
        if let Some(cached) = cached {
            self.update.load(cached);
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
            }
            if !self.until_due(options.interval).is_zero() {
                wait = Duration::ZERO;
                continue;
            }
            self.check_release(source, None).await;
            wait = options.interval;
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
    /// new release, then caches both on disk and in memory. With `fresh`, a
    /// cached answer younger than that is kept instead. A failure keeps the
    /// cached answer.
    async fn check_release(&self, source: Source, fresh: Option<Duration>) {
        let _checking = self.update.checking.lock().await;
        if let Some(fresh) = fresh
            && !self.until_due(fresh).is_zero()
        {
            return;
        }
        let options = &self.opts.update_check;
        let latest =
            match devlanes_update::check::latest(source, &options.urls, options.timeout).await {
                Ok(latest) => latest,
                Err(error) => {
                    tracing::debug!(%error, source = source.name(), "release check failed");
                    return;
                }
            };
        let previous = self
            .update
            .get()
            .filter(|previous| previous.notes_for.as_deref() == Some(latest.as_str()));
        let (notes_for, notes) = match previous {
            Some(previous) => (previous.notes_for, previous.notes),
            None if is_newer(&latest, devlanes_version::VERSION) => {
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
