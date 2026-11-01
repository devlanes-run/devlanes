//! The `devlanes.exe.old-*` copies `install.ps1` leaves on Windows.

use std::path::{Path, PathBuf};

/// Copies of `devlanes.exe` that an upgrade left in a directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OldCopies {
    /// Deleted now.
    pub removed: Vec<PathBuf>,
    /// Still held by a running process (usually the daemon), so kept.
    pub in_use: Vec<PathBuf>,
}

/// Deletes the `devlanes.exe.old-*` files `install.ps1` leaves next to an
/// executable that was running during an upgrade. Windows refuses to delete
/// one that still runs; it is reported in `in_use` and tried again next time.
/// Blocking filesystem work.
pub fn remove_old_copies(dir: &Path) -> OldCopies {
    let mut copies = OldCopies::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return copies;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            name.strip_prefix("devlanes.exe.old-")
                .is_some_and(|stamp| !stamp.is_empty())
        })
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        match std::fs::remove_file(&path) {
            Ok(()) => copies.removed.push(path),
            Err(_) => copies.in_use.push(path),
        }
    }
    copies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_copies_are_removed_unless_running() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("devlanes.exe"), "current")?;
        std::fs::write(dir.path().join("devlanes.exe.old-20261023101500"), "old")?;
        std::fs::write(dir.path().join("devlanes.exe.old-"), "no stamp")?;
        std::fs::write(dir.path().join("other.exe.old-20261023101500"), "other")?;
        // An open handle without delete sharing stands in for a running executable.
        let held = dir.path().join("devlanes.exe.old-20261023101600");
        std::fs::write(&held, "running")?;
        let handle = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&held)?
        };
        let copies = remove_old_copies(dir.path());
        assert_eq!(
            copies.removed,
            vec![dir.path().join("devlanes.exe.old-20261023101500")]
        );
        assert_eq!(copies.in_use, vec![held.clone()]);
        drop(handle);
        assert_eq!(remove_old_copies(dir.path()).removed, vec![held]);
        for kept in [
            "devlanes.exe",
            "devlanes.exe.old-",
            "other.exe.old-20261023101500",
        ] {
            assert!(dir.path().join(kept).exists(), "{kept}");
        }
        Ok(())
    }
}
