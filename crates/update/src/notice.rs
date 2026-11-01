//! Release notices on the client side: their text, their keys, and when each
//! was last shown or dismissed.

use std::{collections::BTreeMap, path::Path, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    InstallMethod,
    notes::{Release, WhatsNew},
    version::{Bump, Version},
};

/// The notice log under `$DEVLANES_HOME`.
const NOTICES_FILE: &str = "notices.json";
/// A notice repeats at most once per day.
const REPEAT_MS: u64 = 24 * 3600 * 1000;

/// The command notices name for installing `latest`: `devlanes update` where
/// devlanes installs releases itself, else the upgrade command of how this
/// binary was installed.
pub fn update_command(method: &InstallMethod, latest: &str) -> String {
    if crate::install::installs_itself(method) {
        "devlanes update".into()
    } else {
        method.upgrade_command(latest)
    }
}

/// The notice for a newer release `bump` away from `current`, with the command
/// that installs it. A minor release says it may break what worked.
pub fn update_line(latest: &str, current: &str, bump: Bump, command: &str) -> String {
    let breaking = match bump {
        Bump::Minor => " with breaking changes",
        Bump::Patch => "",
    };
    format!("devlanes {latest} is available{breaking} (this is {current}): {command}")
}

/// The notice for a release the daemon installed or downloaded in the
/// background, which the next interactive `devlanes` switches to.
pub fn ready_line(installed: &str, current: &str) -> String {
    format!("devlanes {installed} is ready (this is {current}): run devlanes to switch to it")
}

/// Notice key for a release ready to switch to.
pub fn ready_key(installed: &str) -> String {
    format!("ready:{installed}")
}

/// The notice for a daemon that runs another version than this client.
pub fn skew_line(daemon: &str, client: &str) -> String {
    format!(
        "daemon runs {daemon}, this CLI is {client}; run devlanes daemon restart when convenient (it stops services)"
    )
}

/// Notice key for a release.
pub fn update_key(latest: &str) -> String {
    format!("update:{latest}")
}

/// Notice key for a version pair.
pub fn skew_key(daemon: &str, client: &str) -> String {
    format!("skew:{daemon}:{client}")
}

/// When each notice was last shown, and which ones the TUI dismissed, in
/// `$DEVLANES_HOME/notices.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeLog {
    /// Notice key to milliseconds since the epoch.
    #[serde(default)]
    pub shown: BTreeMap<String, u64>,
    /// Keys the user dismissed in the TUI.
    #[serde(default)]
    pub dismissed: Vec<String>,
    /// The newest release whose "What's new" was shown, or that a fresh home
    /// started on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_version: Option<String>,
}

impl NoticeLog {
    /// Reads the log under `home`; a missing or broken file is an empty log.
    pub fn load(home: &Path) -> Self {
        Self::read(home).unwrap_or_default()
    }

    /// Reads the log under `home`; `None` when there is no file, and an
    /// empty log when it is broken.
    fn read(home: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(home.join(NOTICES_FILE)).ok()?;
        Some(serde_json::from_str(&text).unwrap_or_default())
    }

    /// Writes the log through a temporary file and a rename.
    pub fn save(&self, home: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        let path = home.join(NOTICES_FILE);
        let temporary = home.join(format!(".{NOTICES_FILE}.{}", std::process::id()));
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)
    }

    /// Whether `key` may be shown at `now`: never shown, or shown a day ago.
    pub fn due(&self, key: &str, now: u64) -> bool {
        self.shown
            .get(key)
            .is_none_or(|shown| now.saturating_sub(*shown) >= REPEAT_MS)
    }

    /// Records `key` as shown at `now`, keeping only the newest entries.
    pub fn mark_shown(&mut self, key: &str, now: u64) {
        self.shown.insert(key.into(), now);
        while self.shown.len() > 32 {
            let oldest = self
                .shown
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => self.shown.remove(&oldest),
                None => break,
            };
        }
    }

    /// Whether the TUI user dismissed `key`.
    pub fn is_dismissed(&self, key: &str) -> bool {
        self.dismissed.iter().any(|dismissed| dismissed == key)
    }

    /// Dismisses `key`, keeping the most recent dismissals.
    pub fn dismiss(&mut self, key: &str) {
        if !self.is_dismissed(key) {
            self.dismissed.push(key.into());
        }
        let excess = self.dismissed.len().saturating_sub(32);
        self.dismissed.drain(..excess);
    }
}

/// What opening devlanes on a version does about "What's new".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhatsNewStart {
    /// Show these notes and record the version as seen.
    Show(WhatsNew),
    /// Record the version as seen without showing anything.
    Record,
    /// Leave the log alone.
    Nothing,
}

/// The "What's new" rule for opening devlanes `running`, given the notice
/// log (`None` when the home has none yet) and the `releases` this build
/// carries. A stable release newer than the one seen shows every release
/// after it up to `running`; a log that never saw one shows `running`'s own
/// notes. A home without a log is a fresh install: it records `running`
/// silently. Prereleases, `local` builds and downgrades never show.
pub fn whats_new_on_start(
    log: Option<&NoticeLog>,
    running: &str,
    local: bool,
    releases: &[Release],
) -> WhatsNewStart {
    let Some(version) = Version::parse(running) else {
        return WhatsNewStart::Nothing;
    };
    if local || version.is_prerelease() {
        return WhatsNewStart::Nothing;
    }
    let Some(log) = log else {
        return WhatsNewStart::Record;
    };
    let seen = log
        .seen_version
        .as_deref()
        .and_then(|seen| Some((seen, Version::parse(seen)?)));
    let shown = match seen {
        Some((_, seen)) if seen >= version => return WhatsNewStart::Nothing,
        Some((seen, _)) => WhatsNew::since(releases, seen, running),
        None => WhatsNew::of(releases, running),
    };
    if shown.is_empty() {
        WhatsNewStart::Record
    } else {
        WhatsNewStart::Show(shown)
    }
}

/// Applies [`whats_new_on_start`] to the log under `home` and records what it
/// says, returning the notes to show. Runs under the update lock, so of
/// several TUIs opening at once only one shows them: the others find the
/// lock taken and show nothing, as does a TUI opening while an update
/// installs; the next one shows them then. Blocking.
pub fn claim_whats_new(
    home: &Path,
    running: &str,
    local: bool,
    releases: &[Release],
) -> Option<WhatsNew> {
    let _lock = crate::install::Lock::acquire(home, Duration::ZERO).ok()?;
    let log = NoticeLog::read(home);
    let shown = match whats_new_on_start(log.as_ref(), running, local, releases) {
        WhatsNewStart::Nothing => return None,
        WhatsNewStart::Record => None,
        WhatsNewStart::Show(shown) => Some(shown),
    };
    let mut log = log.unwrap_or_default();
    log.seen_version = Some(running.to_owned());
    // Losing the record only shows the notes again next time.
    let _ = log.save(home);
    shown
}

/// Milliseconds since the epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_lines_mark_breaking_releases() {
        assert_eq!(
            update_line("0.9.1", "0.9.0", Bump::Patch, "devlanes update"),
            "devlanes 0.9.1 is available (this is 0.9.0): devlanes update"
        );
        assert_eq!(
            update_line("0.10.0", "0.9.0", Bump::Minor, "devlanes update"),
            "devlanes 0.10.0 is available with breaking changes (this is 0.9.0): devlanes update"
        );
    }

    #[test]
    fn notices_name_devlanes_update_unless_it_cannot_install() {
        for method in [
            InstallMethod::Homebrew,
            InstallMethod::Npm,
            InstallMethod::Pipx,
            InstallMethod::Script(std::path::PathBuf::from("/opt/acme/bin")),
        ] {
            assert_eq!(update_command(&method, "0.9.1"), "devlanes update");
        }
        for method in [InstallMethod::CargoInstall, InstallMethod::Uvx] {
            assert_eq!(
                update_command(&method, "0.9.1"),
                method.upgrade_command("0.9.1")
            );
        }
        assert_eq!(
            ready_line("0.9.1", "0.9.0"),
            "devlanes 0.9.1 is ready (this is 0.9.0): run devlanes to switch to it"
        );
    }

    /// Releases 0.10.0, 0.9.1 and 0.9.0, newest first, each with a patch and
    /// then a minor entry.
    fn releases() -> Vec<Release> {
        use crate::notes::{Entry, Level};
        ["0.10.0", "0.9.1", "0.9.0"]
            .into_iter()
            .map(|version| Release {
                version: version.into(),
                entries: [Level::Patch, Level::Minor]
                    .into_iter()
                    .map(|level| Entry {
                        level,
                        line: format!("{} in {version}", level.as_str()),
                        body: String::new(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// The versions and lines `start` shows, or why it shows nothing.
    fn shown(start: WhatsNewStart) -> Result<(String, Vec<String>), WhatsNewStart> {
        match start {
            WhatsNewStart::Show(shown) => Ok((
                shown.title(),
                shown
                    .releases
                    .iter()
                    .flat_map(|release| release.entries.iter().map(|entry| entry.line.clone()))
                    .collect(),
            )),
            other => Err(other),
        }
    }

    #[test]
    fn whats_new_shows_releases_since_the_seen_one_once() {
        let releases = releases();
        let seen = |version: Option<&str>| NoticeLog {
            seen_version: version.map(str::to_owned),
            ..NoticeLog::default()
        };
        let rule = |log: Option<&NoticeLog>, running, local| {
            shown(whats_new_on_start(log, running, local, &releases))
        };
        assert_eq!(
            rule(Some(&seen(Some("0.9.0"))), "0.10.0", false),
            Ok((
                "What's new since 0.9.0".into(),
                vec![
                    "minor in 0.10.0".into(),
                    "patch in 0.10.0".into(),
                    "minor in 0.9.1".into(),
                    "patch in 0.9.1".into(),
                ]
            ))
        );
        assert_eq!(
            rule(Some(&seen(Some("0.9.0"))), "0.9.1", false).map(|(title, _)| title),
            Ok("What's new in 0.9.1".into())
        );
        // A log from before seen versions shows the running release alone.
        assert_eq!(
            rule(Some(&seen(None)), "0.9.1", false).map(|(title, _)| title),
            Ok("What's new in 0.9.1".into())
        );
        // A fresh home records silently; nothing to show records too.
        assert_eq!(rule(None, "0.10.0", false), Err(WhatsNewStart::Record));
        assert_eq!(
            rule(Some(&seen(Some("0.10.0"))), "0.10.1", false),
            Err(WhatsNewStart::Record)
        );
        // Seen already, a downgrade, a prerelease, a local build.
        for (log, running, local) in [
            (seen(Some("0.10.0")), "0.10.0", false),
            (seen(Some("0.10.0")), "0.9.1", false),
            (seen(Some("0.9.0")), "0.10.1-next.2", false),
            (seen(Some("0.9.0")), "0.10.0", true),
            (seen(Some("0.9.0")), "local", false),
        ] {
            assert_eq!(
                rule(Some(&log), running, local),
                Err(WhatsNewStart::Nothing),
                "{running}"
            );
        }
        assert_eq!(rule(None, "0.10.0", true), Err(WhatsNewStart::Nothing));
    }

    #[test]
    fn whats_new_is_claimed_once_per_home() -> std::io::Result<()> {
        let releases = releases();
        let dir = tempfile::tempdir()?;
        // A fresh home records the version and shows nothing.
        assert_eq!(claim_whats_new(dir.path(), "0.9.0", false, &releases), None);
        assert_eq!(
            NoticeLog::load(dir.path()).seen_version.as_deref(),
            Some("0.9.0")
        );
        let shown = claim_whats_new(dir.path(), "0.10.0", false, &releases);
        assert_eq!(
            shown.map(|shown| shown.title()).as_deref(),
            Some("What's new since 0.9.0")
        );
        assert_eq!(
            claim_whats_new(dir.path(), "0.10.0", false, &releases),
            None
        );
        // Another process installing, or showing it, holds the lock.
        let mut log = NoticeLog::load(dir.path());
        log.seen_version = Some("0.9.0".into());
        log.save(dir.path())?;
        let lock = crate::install::Lock::acquire(dir.path(), Duration::ZERO)
            .map_err(std::io::Error::other)?;
        assert_eq!(
            claim_whats_new(dir.path(), "0.10.0", false, &releases),
            None
        );
        drop(lock);
        assert!(claim_whats_new(dir.path(), "0.10.0", false, &releases).is_some());
        Ok(())
    }

    #[test]
    fn notices_repeat_daily_and_persist() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut log = NoticeLog::load(dir.path());
        assert!(log.due("update:0.3.0", 1_000));
        log.mark_shown("update:0.3.0", 1_000);
        assert!(!log.due("update:0.3.0", 1_000 + REPEAT_MS - 1));
        assert!(log.due("update:0.3.0", 1_000 + REPEAT_MS));
        assert!(log.due("update:0.4.0", 1_001));
        log.dismiss("update:0.3.0");
        log.save(dir.path())?;
        let loaded = NoticeLog::load(dir.path());
        assert!(loaded.is_dismissed("update:0.3.0"));
        assert_eq!(loaded.shown.get("update:0.3.0"), Some(&1_000));
        std::fs::write(dir.path().join(NOTICES_FILE), "broken")?;
        assert_eq!(NoticeLog::load(dir.path()), NoticeLog::default());
        Ok(())
    }
}
