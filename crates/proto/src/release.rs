//! What the daemon's release check reports. The daemon only reads the latest
//! release's version; it never downloads or installs. Version ordering lives in
//! `devlanes-update`.

use serde::{Deserialize, Serialize};

/// A newer release, in `daemon.status` and `status` results once the daemon has
/// seen it for a while (Homebrew and npm publish a few minutes after GitHub).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub latest: String,
    /// When the daemon last read the release, in milliseconds since the epoch.
    pub checked_at: u64,
}

/// The release check's state, in `daemon.status` for `doctor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseCheck {
    /// `off` (see `reason`), `pending` before the first answer, or `ok`.
    pub state: String,
    /// Why checks are off: `debug_build`, `ci`, `env` (DEVLANES_NO_UPDATE_CHECK) or `config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The latest release version the daemon read, newer or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// When it was read, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<u64>,
}
