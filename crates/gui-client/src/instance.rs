//! One viewer process per GUI service on a machine.
//!
//! A viewer holds an exclusive lock on `<key>.lock` in a directory of the
//! devlanes home for as long as it runs; the OS lets go of it when the
//! process ends, crashed or not. Next to it, `<key>.info` says which process
//! holds it and whether the daemon started it, so a daemon can end a viewer
//! an earlier daemon started, and `<key>.show` is what another `devlanes gui
//! open` writes to ask the running viewer for its windows. The key names the
//! service and its port, so `devlanes gui open` and the daemon's auto-open
//! agree on it.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long [`claim`] keeps trying while someone holds the lock, so that a
/// [`running`] check that holds it for a moment does not look like a viewer.
const CLAIM_PATIENCE: Duration = Duration::from_millis(500);

/// The file name stem for the service `service` (such as `acme@2:desktop`)
/// whose GUI port is `port` on this machine: only characters that are safe
/// in a file name on every OS.
pub fn key(service: &str, port: u16) -> String {
    let safe: String = service
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '@' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();
    format!("{safe}-{port}")
}

/// Who started a viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedBy {
    /// The daemon, for auto-open: it ends the viewer once the service is gone.
    Daemon,
    /// `devlanes gui open` or a person.
    User,
}

/// What `<key>.info` says about the viewer that holds the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The viewer's process id.
    pub pid: u32,
    /// The viewer's process start time, as `devlanes-sys` reads it, so a
    /// reused pid is never mistaken for the viewer; 0 when unknown.
    pub start_time: u64,
    /// Who started it.
    pub started_by: StartedBy,
    /// The service it shows, as `lane:unit`, so a daemon that finds it after
    /// a restart knows whose viewer it is.
    pub service: String,
}

impl Info {
    /// The file's text: one `name=value` line per field.
    fn to_text(&self) -> String {
        let by = match self.started_by {
            StartedBy::Daemon => "daemon",
            StartedBy::User => "user",
        };
        // A service label has no line break; one would only cut it short.
        let service = self.service.lines().next().unwrap_or_default();
        format!(
            "pid={}\nstart_time={}\nstarted_by={by}\nservice={service}\n",
            self.pid, self.start_time
        )
    }

    /// Reads the file's text; `None` when a field is missing or malformed.
    fn parse(text: &str) -> Option<Self> {
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
        };
        Some(Self {
            pid: field("pid")?.parse().ok()?,
            start_time: field("start_time")?.parse().ok()?,
            started_by: match field("started_by")? {
                "daemon" => StartedBy::Daemon,
                "user" => StartedBy::User,
                _ => return None,
            },
            service: field("service")?.to_owned(),
        })
    }
}

/// The paths of one key in `dir`.
fn path(dir: &Path, key: &str, extension: &str) -> PathBuf {
    dir.join(format!("{key}.{extension}"))
}

/// The lock a running viewer holds; released when dropped or when the
/// process ends.
#[derive(Debug)]
pub struct Instance {
    _lock: File,
    show: PathBuf,
    /// The show file's last modification seen, so an older request is not
    /// taken for a new one.
    seen: Option<SystemTime>,
}

impl Instance {
    /// Whether someone asked for the windows since the last call.
    pub fn show_requested(&mut self) -> bool {
        let modified = std::fs::metadata(&self.show)
            .and_then(|meta| meta.modified())
            .ok();
        if modified.is_some() && modified != self.seen {
            self.seen = modified;
            return true;
        }
        false
    }
}

/// What [`claim`] found.
#[derive(Debug)]
pub enum Claim {
    /// This process is now the viewer of the service.
    Claimed(Instance),
    /// Another viewer of the service runs.
    Held,
}

/// Becomes the viewer of `key` in `dir` (created when missing), or finds
/// that another process is. Blocking.
pub fn claim(dir: &Path, key: &str, info: &Info) -> io::Result<Claim> {
    std::fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path(dir, key, "lock"))?;
    let deadline = Instant::now() + CLAIM_PATIENCE;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(Claim::Held),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    write_info(dir, key, info)?;
    let show = path(dir, key, "show");
    // A request written in the moments before this claim was meant for this
    // viewer (a `gui open` that found the lock held by one ending, or that
    // raced this start); an older one was answered by an earlier viewer.
    let seen = std::fs::metadata(&show)
        .and_then(|m| m.modified())
        .ok()
        .filter(|modified| {
            SystemTime::now()
                .duration_since(*modified)
                .is_ok_and(|age| age > FRESH_SHOW)
        });
    Ok(Claim::Claimed(Instance {
        _lock: lock,
        show,
        seen,
    }))
}

/// How recent a show request at [`claim`] counts as one for the new viewer.
const FRESH_SHOW: Duration = Duration::from_secs(5);

/// Writes `<key>.info` through a temporary file, so a reader never sees half
/// of it. On Windows the rename fails while a reader has the file open, so it
/// is retried for a moment and then written in place.
fn write_info(dir: &Path, key: &str, info: &Info) -> io::Result<()> {
    let info_path = path(dir, key, "info");
    let temporary = path(dir, key, "info.tmp");
    File::create(&temporary)?.write_all(info.to_text().as_bytes())?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&temporary, &info_path) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = std::fs::remove_file(&temporary);
                return std::fs::write(&info_path, info.to_text());
            }
        }
    }
}

/// Whether a viewer of `key` in `dir` runs. Blocking. It takes the lock for
/// an instant when nobody holds it, which a viewer starting at that moment
/// waits out in [`claim`].
pub fn running(dir: &Path, key: &str) -> io::Result<bool> {
    let lock = match OpenOptions::new().write(true).open(path(dir, key, "lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match lock.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// What the running viewer of `key` in `dir` wrote about itself, or `None`
/// when no viewer of it runs. Blocking.
pub fn holder(dir: &Path, key: &str) -> io::Result<Option<Info>> {
    if !running(dir, key)? {
        return Ok(None);
    }
    let mut text = String::new();
    match File::open(path(dir, key, "info")) {
        Ok(mut file) => {
            file.read_to_string(&mut text)?;
            Ok(Info::parse(&text))
        }
        // A viewer between taking the lock and writing its info.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The keys in `dir` with a lock file, running or not. Blocking.
pub fn keys(dir: &Path) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut keys = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        if let Some(key) = name.to_str().and_then(|name| name.strip_suffix(".lock")) {
            keys.push(key.to_owned());
        }
    }
    keys.sort();
    Ok(keys)
}

/// Asks the running viewer of `key` in `dir` to show its windows. Blocking.
pub fn ask_to_show(dir: &Path, key: &str) -> io::Result<()> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // A new modification time is the request; the content only makes sure
    // the write changes the file.
    std::fs::write(path(dir, key, "show"), nanos.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One viewer per key: a second claim finds it held, `running` and
    /// `holder` see it, and its end frees the key. Another key is free.
    #[test]
    fn a_service_has_one_viewer_at_a_time() {
        let dir = tempfile::tempdir().expect("dir");
        let key = key("acme@2:desktop", 26012);
        let info = Info {
            pid: 42,
            start_time: 7,
            started_by: StartedBy::Daemon,
            service: "acme@2:desktop".into(),
        };
        assert!(!running(dir.path(), &key).expect("check"));
        let Claim::Claimed(mut first) = claim(dir.path(), &key, &info).expect("claim") else {
            panic!("the key was free");
        };
        assert!(matches!(claim(dir.path(), &key, &info), Ok(Claim::Held)));
        assert!(running(dir.path(), &key).expect("check"));
        assert_eq!(
            holder(dir.path(), &key).expect("holder"),
            Some(info.clone())
        );
        assert!(!first.show_requested());
        ask_to_show(dir.path(), &key).expect("show");
        assert!(first.show_requested());
        assert!(!first.show_requested(), "one request is seen once");
        assert!(matches!(
            claim(dir.path(), "acme@2_desktop-26013", &info),
            Ok(Claim::Claimed(_))
        ));
        drop(first);
        assert!(!running(dir.path(), &key).expect("check"));
        assert_eq!(holder(dir.path(), &key).expect("holder"), None);
        assert!(matches!(
            claim(dir.path(), &key, &info),
            Ok(Claim::Claimed(_))
        ));
    }
}
