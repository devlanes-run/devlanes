//! Client side: the id and name this machine sends to every box, so a box
//! lists the same client under the same id across reconnects and restarts.

use std::{
    io::{self, Write},
    path::Path,
};

use devlanes_proto::remote::{CLIENT_ID_PREFIX, is_random_id};

/// File under `$DEVLANES_HOME` that holds the id.
pub(crate) const ID_FILE: &str = "client-id";

/// What a client daemon tells boxes about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientIdentity {
    /// Random id kept in [`ID_FILE`]: `dl_cid_` and 64 lowercase hex digits.
    /// Boxes bind their pairing with this machine to it.
    pub id: String,
    /// This machine's host name.
    pub name: String,
}

/// Reads the id from `home`, creating it the first time (mode 0600 on Unix)
/// or replacing a damaged one, plus the host name. Blocking.
pub(crate) fn load(home: &Path) -> io::Result<ClientIdentity> {
    Ok(ClientIdentity {
        id: stored_id(home)?,
        name: host_name(),
    })
}

/// The id in `home`, written first when missing or not an id.
///
/// A new id is written whole to a temporary file and then hard-linked into
/// place, which fails when the file exists: a daemon reading at the same time
/// never sees a half-written id, and an id another daemon of this home just
/// stored is read instead of replaced. Where the file system has no hard
/// links, the id is written to its file directly, which also fails when the
/// file exists; a daemon reading at that moment may then see a partial id.
fn stored_id(home: &Path) -> io::Result<String> {
    stored_id_with(home, |from, to| std::fs::hard_link(from, to))
}

/// [`stored_id`] with `link` in place of [`std::fs::hard_link`], so a test
/// can take the fallback.
fn stored_id_with(
    home: &Path,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<String> {
    let path = home.join(ID_FILE);
    loop {
        match std::fs::read_to_string(&path) {
            Ok(text) if valid(text.trim()) => return Ok(text.trim().to_owned()),
            // Something else wrote there; a fresh id beats failing every connect.
            Ok(_) => {
                if let Err(error) = std::fs::remove_file(&path)
                    && error.kind() != io::ErrorKind::NotFound
                {
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::create_dir_all(home)?;
        let id = random_id()?;
        let temporary = home.join(format!("{ID_FILE}.{id}.tmp"));
        write_new(&temporary, &id)?;
        let linked = link(&temporary, &path);
        let _ = std::fs::remove_file(&temporary);
        let stored = match linked {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                tracing::debug!(%error, "no hard link for the client id; writing it in place");
                write_new(&path, &id)
            }
            linked => linked,
        };
        match stored {
            Ok(()) => return Ok(id),
            // Another daemon of this home stored one first: read that.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

/// Creates `path` (mode 0600 on Unix) holding `id` and syncs it.
fn write_new(path: &Path, id: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{id}")?;
    file.sync_all()
}

/// Whether `text` is an id this module writes.
fn valid(text: &str) -> bool {
    is_random_id(text, CLIENT_ID_PREFIX)
}

/// `dl_cid_` and 256 bits from the OS random source as lowercase hex.
fn random_id() -> io::Result<String> {
    super::pairing::random_hex(CLIENT_ID_PREFIX)
}

/// The machine's host name, empty when unknown.
#[cfg(unix)]
fn host_name() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}

/// The machine's host name, empty when unknown.
#[cfg(windows)]
fn host_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_created_once_and_kept() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let first = load(dir.path())?;
        assert!(valid(&first.id), "{}", first.id);
        assert!(first.id.starts_with("dl_cid_"), "{}", first.id);
        assert_eq!(first.id.len(), "dl_cid_".len() + 64);
        assert_eq!(load(dir.path())?, first, "a second read keeps the id");
        let stored = std::fs::read_to_string(dir.path().join(ID_FILE))?;
        assert_eq!(stored.trim(), first.id);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(ID_FILE))?
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let other = tempfile::tempdir()?;
        assert_ne!(
            load(other.path())?.id,
            first.id,
            "each home gets its own id"
        );
        Ok(())
    }

    #[test]
    fn a_damaged_id_is_replaced() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        // Phase 2's bare 128-bit form is not an id either.
        std::fs::write(
            dir.path().join(ID_FILE),
            b"0123456789abcdef0123456789abcdef\n",
        )?;
        let identity = load(dir.path())?;
        assert!(valid(&identity.id), "{}", identity.id);
        assert_eq!(load(dir.path())?.id, identity.id);
        Ok(())
    }

    /// Without hard links the id is written in place, the temporary file is
    /// removed, and an id already there is read instead of replaced.
    #[test]
    fn the_id_is_written_in_place_without_hard_links() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let unsupported = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::Unsupported));
        let id = stored_id_with(dir.path(), unsupported)?;
        assert!(valid(&id), "{id}");
        let files: Vec<_> = std::fs::read_dir(dir.path())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<_>>()?;
        assert_eq!(files, [ID_FILE], "{files:?}");
        assert_eq!(stored_id_with(dir.path(), unsupported)?, id);

        // A daemon that stores its id between this one's read and write wins.
        let other = tempfile::tempdir()?;
        let theirs = format!("dl_cid_{}", "a".repeat(64));
        let racing = |_: &Path, to: &Path| {
            std::fs::write(to, format!("{theirs}\n"))?;
            Err(io::Error::from(io::ErrorKind::Unsupported))
        };
        assert_eq!(stored_id_with(other.path(), racing)?, theirs);
        Ok(())
    }

    /// Daemons creating the id at the same moment agree on one, and leave no
    /// temporary files behind.
    #[test]
    fn concurrent_creation_agrees_on_one_id() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let ids = std::thread::scope(|scope| {
            let readers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| stored_id(dir.path())))
                .collect();
            readers
                .into_iter()
                .map(|reader| {
                    reader
                        .join()
                        .map_err(|_| io::Error::other("reader panicked"))?
                })
                .collect::<io::Result<Vec<String>>>()
        })?;
        assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
        let files: Vec<_> = std::fs::read_dir(dir.path())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<_>>()?;
        assert_eq!(files, [ID_FILE], "{files:?}");
        Ok(())
    }
}
