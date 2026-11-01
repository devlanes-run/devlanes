//! Release notices on the client side: their text, their keys, and when each
//! was last shown or dismissed.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use crate::{InstallMethod, version::Bump};

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
        Bump::Minor => " (breaking changes)",
        Bump::Patch => "",
    };
    format!("devlanes {latest} is available (this is {current}): {command}{breaking}")
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
}

impl NoticeLog {
    /// Reads the log under `home`; a missing or broken file is an empty log.
    pub fn load(home: &Path) -> Self {
        std::fs::read_to_string(home.join(NOTICES_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
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
            "devlanes 0.10.0 is available (this is 0.9.0): devlanes update (breaking changes)"
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
