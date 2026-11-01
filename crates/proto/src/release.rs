//! What the daemon's release check reports. The daemon reads the latest
//! version from the registry devlanes was installed from; it never downloads or
//! installs. Version ordering, sources and release notes live in
//! `devlanes-update`.

use serde::{Deserialize, Serialize};

/// A newer release than the daemon runs, as `daemon.status` and `status`
/// report it in `update` and `update.check` replies with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// The newest release on the channel.
    pub latest: String,
    /// The channel it came from: `stable` or `next`.
    #[serde(default)]
    pub channel: String,
    /// How far it is from the running version: `patch` or `minor` (which may
    /// break what worked, before 1.0).
    #[serde(default)]
    pub bump: String,
    /// `available`: found, not installed.
    #[serde(default)]
    pub state: String,
    /// When the daemon last read the release, in milliseconds since the epoch.
    pub checked_at: u64,
    /// The changelog entries of the releases after the running version up to
    /// `latest`, newest release first; empty until the daemon read them.
    #[serde(default)]
    pub notes: Vec<UpdateNote>,
}

/// One changelog entry of a release newer than the running one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNote {
    /// The release that made the change.
    pub version: String,
    /// `minor` or `patch`, the changelog heading it sits under.
    pub level: String,
    /// The entry's first line, without the commit link.
    pub line: String,
}

/// The release check's state, in `daemon.status` for `doctor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseCheck {
    /// `off` (see `reason`), `pending` before the first answer, or `ok`.
    pub state: String,
    /// Why checks are off: `debug_build`, `ci`, `env` (DEVLANES_NO_UPDATE_CHECK),
    /// `config` (`updates.mode: disable`), `local` (a project dependency) or
    /// `source` (a build from a checkout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `updates.mode` as the daemon loaded it: `disable`, `notify` or `auto`.
    #[serde(default)]
    pub mode: String,
    /// The channel the check follows: `stable` or `next`; absent until the
    /// daemon knows its install method, and when it has no source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Where the check reads versions: `github`, `npm`, `pypi` or `homebrew`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The latest release version the daemon read, newer or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// `patch` or `minor` when `latest` is newer than the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bump: Option<String>,
    /// When it was read, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<u64>,
}

/// Params of `update.check`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheckParams {
    /// Check even when the last check is recent.
    #[serde(default)]
    pub force: bool,
}
