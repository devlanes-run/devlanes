//! Windows refuses to delete an executable that runs, so a package manager
//! that deletes and rewrites `devlanes.exe` (npm, pnpm, uv, pipx) fails while
//! the daemon or a TUI runs it. Once a second hard link to the file exists,
//! the original name can be deleted and a new file written there, while the
//! process keeps running from the link. Long-running devlanes processes keep
//! such a link in `$DEVLANES_HOME/run/exe/<pid>.exe`.

use std::path::{Path, PathBuf};

/// A hard link to this process's executable, deleted when dropped. A link
/// that is the last name of a running file cannot be deleted; the next
/// [`retain_own_exe`] after its process ended does.
#[derive(Debug)]
pub struct Retained {
    link: PathBuf,
}

impl Retained {
    /// The link, which the process's executable can be reached through
    /// after its original name is deleted.
    pub fn path(&self) -> &Path {
        &self.link
    }
}

impl Drop for Retained {
    /// Deletes the link.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.link);
    }
}

/// Links this process's executable into `$DEVLANES_HOME/run/exe/<pid>.exe`
/// under `home`, after deleting the links of processes that ended. `None`
/// when the link cannot be made, such as when the executable lives on
/// another volume than the home. Blocking.
pub fn retain_own_exe(home: &Path) -> Option<Retained> {
    let exe = std::env::current_exe().ok()?;
    retain(home, &exe, std::process::id())
}

/// [`retain_own_exe`] for the process `pid` running `exe`.
fn retain(home: &Path, exe: &Path, pid: u32) -> Option<Retained> {
    let dir = home.join("run").join("exe");
    std::fs::create_dir_all(&dir).ok()?;
    sweep(&dir, pid);
    let link = dir.join(format!("{pid}.exe"));
    // A link left by an earlier process that had this PID.
    let _ = std::fs::remove_file(&link);
    std::fs::hard_link(exe, &link).ok()?;
    Some(Retained { link })
}

/// Deletes the links in `dir` whose process ended, other than `own`'s.
fn sweep(dir: &Path, own: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".exe"))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let running = matches!(devlanes_sys::process(pid), Ok(Some(info)) if !info.zombie);
        if pid != own && !running {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    /// A copy of System32's `ping.exe` that runs for a while stands in for a
    /// running `devlanes.exe`: Windows refuses to delete it, but once it is
    /// retained, its name can be deleted and written again, as a package
    /// manager does. The link of the ended process goes at the next sweep.
    #[test]
    fn a_retained_running_exe_can_be_replaced() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let (bin, home) = (root.path().join("bin"), root.path().join("home"));
        std::fs::create_dir_all(&bin)?;
        let system = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"));
        let exe = bin.join("devlanes.exe");
        std::fs::copy(system.join(r"System32\PING.EXE"), &exe)?;
        let mut child = Command::new(&exe)
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let pid = child.id();
        let result = (|| {
            assert!(
                std::fs::remove_file(&exe).is_err(),
                "a running exe without a second link cannot be deleted"
            );
            let retained = retain(&home, &exe, pid).expect("linked");
            assert!(retained.link.is_file());
            std::fs::remove_file(&exe)?;
            std::fs::write(&exe, b"the new release")?;
            assert_eq!(std::fs::read(&exe)?, b"the new release");
            child.kill()?;
            child.wait()?;
            // Left behind as by a process that ended without dropping it.
            std::mem::forget(retained);
            let link = home.join(format!(r"run\exe\{pid}.exe"));
            assert!(link.is_file());
            let own = retain(&home, &exe, std::process::id()).expect("own link");
            assert!(!link.exists(), "the ended process's link was swept");
            let own_link = own.link.clone();
            drop(own);
            assert!(!own_link.exists(), "dropping the guard deletes its link");
            Ok(())
        })();
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}
