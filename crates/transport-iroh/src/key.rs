//! The endpoint's secret key, kept in one file private to the user.
//!
//! Written the way the daemon writes its `client-id`
//! (`crates/daemon/src/remote/identity.rs`), and checked on Windows the way
//! `crates/pki` checks the CA key; neither has a helper this crate can share.
//! Everything here blocks, so callers run it in `spawn_blocking`.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use devlanes_transport::PeerError;
use iroh::SecretKey;

/// Size of the key file: `SecretKey::to_bytes`.
const KEY_LEN: usize = 32;

/// How often a step that another daemon of the same home, or another
/// process holding the file for a moment, may be in the middle of is tried
/// before it counts as failed.
const ATTEMPTS: u32 = 20;

/// The pause between those tries: 20 of them wait about 1 s in all.
const PAUSE: Duration = Duration::from_millis(50);

/// What to do about a key file that cannot be used as it is.
const MOVE_AWAY: &str =
    "move it away to create a new key, then pair this machine with its boxes again";

/// The key in `path`, created first when the file is missing.
///
/// A new key is written whole to a temporary file and then hard-linked into
/// place, which fails when the file exists: a daemon reading at the same time
/// never sees half a key, and a key another daemon of this home just stored
/// is read instead of replaced. Where the file system has no hard links, the
/// key is written to its file directly, which also fails when it exists. A
/// new key is read back through the same checks as an existing one, so a key
/// in a directory that is not private fails now instead of on the next start.
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
            Ok(()) => {}
            // Another daemon of this home stored one first: read that.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(failed(error)),
        }
        let stored = read(path).map_err(failed)?;
        return Ok(stored);
    }
}

/// Reads the key in `path` after checking that no other account can read
/// it. An empty file is another daemon writing it in place (the fallback
/// without hard links), so it is read again for a short while first.
fn read(path: &Path) -> io::Result<SecretKey> {
    let mut attempt = 1;
    loop {
        check_private(path)?;
        let bytes = std::fs::read(path)?;
        if bytes.is_empty() && attempt < ATTEMPTS {
            attempt += 1;
            std::thread::sleep(PAUSE);
            continue;
        }
        let bytes: [u8; KEY_LEN] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "it holds {} bytes instead of a {KEY_LEN}-byte key; {MOVE_AWAY}",
                    bytes.len()
                ),
            )
        })?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
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
    let removed = remove(temporary);
    match linked {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            tracing::debug!(%error, "no hard link for the peer key; writing it in place");
            write_new(path, key)
        }
        // A leftover second name would fail every later read on Windows
        // ("2 hard links"), so it is an error now, while it is explained.
        Ok(()) => removed.map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "the temporary file {} could not be removed ({error}); remove it by hand",
                    temporary.display()
                ),
            )
        }),
        linked => linked,
    }
}

/// Removes `path`, trying again for a short while: on Windows another
/// process (an antivirus scan, an indexer) may hold it open for a moment.
fn remove(path: &Path) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) if attempt < ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(PAUSE);
            }
            Err(error) => return Err(error),
        }
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

/// Fails when the group or others have any access to `path`, so a key other
/// accounts could copy or replace is never used.
#[cfg(unix)]
fn check_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 == 0 {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "other users have access to it (mode {:03o}); run `chmod 600 '{}'`",
            mode & 0o777,
            path.display()
        ),
    ))
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
        Problem::NotADirectory | Problem::NotAFile => {
            format!("it is not a regular file; {MOVE_AWAY}")
        }
        Problem::ReparsePoint => format!("it is a symlink or junction; {MOVE_AWAY}"),
        Problem::Owner(owner) => format!("another account ({owner}) owns it; {MOVE_AWAY}"),
        Problem::Acl(others) => {
            let others: Vec<String> = others.iter().map(ToString::to_string).collect();
            let others = others.join(", ");
            match icacls_fix(path) {
                Some(fix) => format!(
                    "other accounts can read it ({others}); remove their access in \
                     PowerShell with `{fix}`"
                ),
                None => format!("other accounts can read it ({others}); {MOVE_AWAY}"),
            }
        }
        Problem::HardLinks(links) => {
            format!("it has {links} hard links; remove the other names it has")
        }
        Problem::Unreadable => {
            format!("its owner or access list cannot be read or judged; {MOVE_AWAY}")
        }
    };
    Err(io::Error::new(io::ErrorKind::PermissionDenied, text))
}

/// The `icacls` commands that leave `path` with one grant, to this user, as
/// `crates/pki`'s fix does: `/reset` drops the explicit grants, then the
/// file keeps only this user's. The steps are joined with `; `, which Windows
/// PowerShell 5.1 and PowerShell 7 run in order; Git Bash rewrites `/reset`
/// into a path, so the message names PowerShell. `None` when this user's SID
/// cannot be read, so the advice falls back to moving the key away instead
/// of hiding the real problem behind the lookup's error.
#[cfg(windows)]
fn icacls_fix(path: &Path) -> Option<String> {
    let user = devlanes_sys_windows::token::current_user().ok()?;
    let shown = path.display();
    Some(format!(
        r#"icacls "{shown}" /reset; icacls "{shown}" /inheritance:r /grant:r "*{user}:F""#
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

    /// A file that is not a key fails with its path and stays as it was,
    /// since a new key would be a new identity every paired box refuses.
    #[test]
    fn a_file_that_is_not_a_key_is_an_error_and_kept() -> TestResult {
        let home = tempfile::tempdir()?;
        let path = home.path().join("p2p-key");
        load_or_create(&path)?;
        let short = b"not a key".to_vec();
        std::fs::write(&path, &short)?;
        let Err(PeerError::Key { path: named, .. }) = load_or_create(&path) else {
            panic!("a file that is not a key was used or replaced");
        };
        assert_eq!(named, path);
        assert_eq!(std::fs::read(&path)?, short);
        Ok(())
    }

    /// Exposed keys are refused with the path and the accounts or mode that
    /// expose them, instead of being used as this machine's identity, and
    /// the command the message prints makes the same key load again: on Unix
    /// for a key the group or others may read, on Windows for one Everyone
    /// can read.
    #[test]
    fn a_key_others_can_read_is_refused_until_the_printed_fix_runs() -> TestResult {
        let home = tempfile::tempdir()?;
        let path = home.path().join("p2p-key");
        let created = load_or_create(&path)?;
        expose(&path)?;
        let Err(PeerError::Key {
            path: named,
            source,
        }) = load_or_create(&path)
        else {
            panic!("an exposed key was used");
        };
        assert_eq!(named, path);
        assert_eq!(source.kind(), io::ErrorKind::PermissionDenied, "{source}");
        let text = source.to_string();
        #[cfg(unix)]
        assert!(text.contains("644"), "the message names no mode: {text}");
        #[cfg(windows)]
        assert!(
            text.contains("S-1-1-0"),
            "the message names no account: {text}"
        );
        let fix = text
            .split('`')
            .nth(1)
            .ok_or_else(|| format!("the message prints no command: {text}"))?;
        run_shell(fix)?;
        assert_eq!(load_or_create(&path)?.public(), created.public());
        Ok(())
    }

    /// Lets the group and others read `path`.
    #[cfg(unix)]
    fn expose(path: &Path) -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
        Ok(())
    }

    /// Grants Everyone read access to `path` with System32's `icacls`.
    #[cfg(windows)]
    fn expose(path: &Path) -> TestResult {
        let icacls = devlanes_sys_windows::file::system_directory()?.join("icacls.exe");
        let mut command = std::process::Command::new(icacls);
        command.arg(path).args(["/grant", "*S-1-1-0:R"]);
        finish(command, "icacls")
    }

    /// Runs a command a key message prints in the shell it names: `sh` on
    /// Unix, Windows PowerShell on Windows.
    fn run_shell(script: &str) -> TestResult {
        #[cfg(unix)]
        let command = {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", script]);
            command
        };
        #[cfg(windows)]
        let command = {
            let shell = devlanes_sys_windows::file::system_directory()?
                .join(r"WindowsPowerShell\v1.0\powershell.exe");
            let mut command = std::process::Command::new(shell);
            command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
            command
        };
        finish(command, script)
    }

    /// Runs `command` to its end and fails unless it succeeds. It is killed
    /// after 30 s, so a hung tool fails the test instead of hanging it.
    fn finish(mut command: std::process::Command, name: &str) -> TestResult {
        let mut child = command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if std::time::Instant::now() > deadline {
                child.kill()?;
                child.wait()?;
                return Err(format!("{name} did not finish within 30 s").into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success(), "{name}: {status}");
        Ok(())
    }
}
