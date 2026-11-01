//! Updating devlanes: how this binary was installed, the command that upgrades
//! it, the stable path that runs the install after an upgrade, release
//! versions, release checks and notes, and the notices clients show. The
//! daemon finds new releases; clients print what it reports.

pub mod check;
mod method;
pub mod notes;
mod notice;
#[cfg(windows)]
mod old_copies;
pub mod version;

pub use method::{InstallFacts, InstallMethod, Venv, binstall_lists_devlanes, spawn_exe};
pub use notice::{NoticeLog, now_ms, skew_key, skew_line, update_command, update_key, update_line};
#[cfg(windows)]
pub use old_copies::{OldCopies, remove_old_copies};
