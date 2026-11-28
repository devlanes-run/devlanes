//! The endpoint's secret key, kept in one file private to the user.
//!
//! Written the way the daemon writes its `client-id`
//! (`crates/daemon/src/remote/identity.rs`), and checked on Windows the way
//! `crates/pki` checks the CA key; neither has a helper this crate can share.
//! Everything here blocks, so callers run it in `spawn_blocking`.

use std::io::{self, Write};
use std::path::Path;

use devlanes_transport::PeerError;
use iroh::SecretKey;

/// Size of the key file: `SecretKey::to_bytes`.
const KEY_LEN: usize = 32;

/// The key in `path`, created first when the file is missing.
///
/// A new key is written whole to a temporary file and then hard-linked into
/// place, which fails when the file exists: a daemon reading at the same time
/// never sees half a key, and a key another daemon of this home just stored
/// is read instead of replaced. Where the file system has no hard links, the
/// key is written to its file directly, which also fails when it exists.
///
/// A file that is not a key is an error rather than replaced: a new key is a
/// new identity, which every paired box would refuse without saying why.
pub(crate) fn load_or_create(path: &Path) -> Result<SecretKey, PeerError> {
    let failed = |source| PeerError::Key {
        path: path.to_owned(),
        source,
    };
    loop {
        match read(path) {
            Ok(key) => return Ok(key),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(failed(error)),
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        let key = SecretKey::generate();
        match store(path, &key) {
            Ok(()) => return Ok(key),
            // Another daemon of this home stored one first: read that.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failed(error)),
        }
    }
}

/// Reads the key in `path`, after checking on Windows that no other account
/// can read it.
fn read(path: &Path) -> io::Result<SecretKey> {
    #[cfg(windows)]
    check_private(path)?;
    let bytes = std::fs::read(path)?;
    let bytes: [u8; KEY_LEN] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "holds {} bytes instead of a {KEY_LEN}-byte key; move it away to create a new key",
                bytes.len()
            ),
        )
    })?;
    Ok(SecretKey::from_bytes(&bytes))
}

/// Stores `key` at `path` through a temporary file and a hard link, falling
/// back to writing it in place. Fails with `AlreadyExists` when `path` exists.
fn store(path: &Path, key: &SecretKey) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    // The new key's own id keeps two daemons racing here off each other's file.
    temporary.push(format!(".{}.tmp", key.public().fmt_short()));
    let temporary = Path::new(&temporary);
    write_new(temporary, key)?;
    let linked = std::fs::hard_link(temporary, path);
    let _ = std::fs::remove_file(temporary);
    match linked {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            tracing::debug!(%error, "no hard link for the peer key; writing it in place");
            write_new(path, key)
        }
        linked => linked,
    }
}

/// Creates `path` (mode 0600 on Unix) holding `key` and syncs it. On Windows
/// the file inherits the access list of the daemon's private home.
fn write_new(path: &Path, key: &SecretKey) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&key.to_bytes())?;
    file.sync_all()
}

/// Fails when `path` is not a regular file with one name that only this user
/// can open, so a key other accounts could copy is never used.
#[cfg(windows)]
fn check_private(path: &Path) -> io::Result<()> {
    use devlanes_sys_windows::security::{Problem, check_private_file};
    let problem = match check_private_file(path)? {
        Ok(()) => return Ok(()),
        Err(problem) => problem,
    };
    let text = match problem {
        Problem::NotADirectory | Problem::NotAFile => "it is not a regular file".to_owned(),
        Problem::ReparsePoint => "it is a symlink or junction".to_owned(),
        Problem::Owner(owner) => format!("another account ({owner}) owns it"),
        Problem::Acl(others) => {
            let others: Vec<String> = others.iter().map(ToString::to_string).collect();
            format!("other accounts can read it ({})", others.join(", "))
        }
        Problem::HardLinks(links) => format!("it has {links} hard links"),
        Problem::Unreadable => "its owner or access list cannot be read or judged".to_owned(),
    };
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{text}; move it away to create a new key"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The first load creates the key and every later one reads the same key
    /// back, so the endpoint keeps its id across restarts; on Unix the file
    /// is readable by its owner only.
    #[test]
    fn a_key_is_created_once_and_kept() -> TestResult {
        let home = tempfile::tempdir()?;
        let path = home.path().join("p2p-key");
        let created = load_or_create(&path)?;
        let loaded = load_or_create(&path)?;
        assert_eq!(loaded.public(), created.public());
        assert_eq!(std::fs::read(&path)?.len(), KEY_LEN);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)?.permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(home.path())?.collect::<Result<_, _>>()?;
        assert_eq!(leftovers.len(), 1, "temporary files left: {leftovers:?}");
        Ok(())
    }

    /// A key other accounts can read is refused with the path, instead of
    /// being used as this machine's identity.
    #[cfg(windows)]
    #[test]
    fn a_key_others_can_read_is_refused() -> TestResult {
        let home = tempfile::tempdir()?;
        let path = home.path().join("p2p-key");
        load_or_create(&path)?;
        let icacls = devlanes_sys_windows::file::system_directory()?.join("icacls.exe");
        let status = std::process::Command::new(icacls)
            .arg(&path)
            .args(["/grant", "*S-1-1-0:R"])
            .stdout(std::process::Stdio::null())
            .status()?;
        assert!(status.success(), "icacls {}", path.display());
        let Err(PeerError::Key {
            path: named,
            source,
        }) = load_or_create(&path)
        else {
            panic!("an exposed key was used");
        };
        assert_eq!(named, path);
        assert!(source.to_string().contains("S-1-1-0"), "{source}");
        Ok(())
    }
}
