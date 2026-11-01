//! Updating devlanes: how this binary was installed, the stable path that
//! runs the install after an update, release versions, release checks and
//! notes, installing a release the way devlanes was installed, and the
//! notices clients show. The daemon finds new releases and, with
//! `updates.mode: auto`, installs patch releases; `devlanes update` installs
//! the latest release when it is newer.

pub mod check;
pub mod install;
mod method;
pub mod notes;
mod notice;
#[cfg(windows)]
mod old_copies;
#[cfg(windows)]
mod retain;
pub mod version;

pub use method::{
    InstallFacts, InstallMethod, SCRIPT_NAME, Venv, binstall_lists_devlanes, default_script_dir,
    entry_or, installed_version, running_exe, script_install_command, spawn_exe, version_of,
};
pub use notice::{
    NoticeLog, WhatsNewStart, claim_whats_new, now_ms, ready_key, ready_line, skew_key, skew_line,
    update_command, update_key, update_line, whats_new_on_start,
};
#[cfg(windows)]
pub use old_copies::{OldCopies, remove_old_copies};
#[cfg(windows)]
pub use retain::{Retained, retain_own_exe};

/// Whether a package manager may replace this process's executable while it
/// runs: always on macOS and Linux; on Windows only while this process keeps
/// a second link to it ([`retain_own_exe`]). Feeds [`install::Plan::new`].
pub fn own_exe_retained() -> bool {
    #[cfg(windows)]
    {
        retain::held()
    }
    #[cfg(not(windows))]
    {
        true
    }
}
