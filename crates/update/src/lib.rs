//! Updating devlanes: how this binary was installed, the stable path that
//! runs the install after an upgrade, release versions, release checks and
//! notes, installing a release the way devlanes was installed, and the
//! notices clients show. The daemon finds new releases and may download them;
//! `devlanes update` installs them.

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
    running_exe, script_install_command, spawn_exe,
};
pub use notice::{
    NoticeLog, WhatsNewStart, claim_whats_new, now_ms, ready_key, ready_line, skew_key, skew_line,
    update_command, update_key, update_line, whats_new_on_start,
};
#[cfg(windows)]
pub use old_copies::{OldCopies, remove_old_copies};
#[cfg(windows)]
pub use retain::{Retained, retain_own_exe};
