use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    future::Future,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::SystemTime,
};

use crate::{Error, cli::ComposeProject, model::ComposeModel};

mod references;

use references::References;

/// Size and modification time of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    /// `None` when the file does not exist or cannot be read.
    fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

/// One `docker compose` invocation: its global flags (project name, `-f` files,
/// `--env-file`, profiles) and working directory. A project keeps one cache entry
/// per slot, so edits replace the entry instead of adding one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Slot {
    working_dir: PathBuf,
    args: Vec<OsString>,
}

impl Slot {
    /// The slot of `project` run with `extra_files` after its own files.
    pub(crate) fn of(project: &ComposeProject, extra_files: &[PathBuf]) -> Self {
        Self {
            working_dir: project.working_dir.clone(),
            args: project.args(extra_files),
        }
    }
}

/// What Compose reads besides its flags. Compose files, `.env` files and the files
/// the compose files name (see [`References`]) are compared by size and
/// modification time; extra files (the lane override, rewritten on every start) by
/// content; the interpolation variables by a hash, so their values, which can be
/// secrets, are not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    files: Vec<(PathBuf, Option<FileStamp>)>,
    references: Vec<(PathBuf, Option<FileStamp>)>,
    extra: Vec<(PathBuf, Option<u64>)>,
    env: u64,
}

/// Cache key of one `config` or `config --hash` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigKey {
    slot: Slot,
    fingerprint: Fingerprint,
    /// Why the fingerprint misses a file Compose reads, when it does. Such a key
    /// never hits.
    untracked: Option<&'static str>,
}

impl ConfigKey {
    /// Parses `project`'s compose files for the files they name, stats those and
    /// the files `project` reads, and reads `extra_files`, on a blocking thread.
    /// Extra files are not parsed: devlanes generates them without file references.
    pub(crate) async fn read(
        project: &ComposeProject,
        extra_files: &[PathBuf],
    ) -> Result<Self, Error> {
        let slot = Slot::of(project, extra_files);
        let env = hash_of(&project.env);
        let watched = watched_files(project);
        let compose_files = project.absolute_files();
        // Compose's default project directory, since devlanes never passes
        // `--project-directory`.
        let project_dir = compose_files
            .first()
            .and_then(|file| file.parent())
            .unwrap_or(&project.working_dir)
            .to_owned();
        let extra = extra_files.to_vec();
        let (files, references, untracked, extra) = tokio::task::spawn_blocking(move || {
            // Stamped before the scan, so an edit during the scan shows as a newer
            // stamp on the next read.
            let files = stamps(watched);
            let scanned = References::scan(&compose_files, &project_dir);
            let extra = extra
                .into_iter()
                .map(|path| {
                    let content = std::fs::read(&path).ok().map(|bytes| hash_of(&bytes));
                    (path, content)
                })
                .collect();
            (files, stamps(scanned.files), scanned.untracked, extra)
        })
        .await
        .map_err(|error| Error::Io(std::io::Error::other(error)))?;
        Ok(Self {
            slot,
            fingerprint: Fingerprint {
                files,
                references,
                extra,
                env,
            },
            untracked,
        })
    }
}

/// Each of `paths` with its current stamp.
fn stamps(paths: Vec<PathBuf>) -> Vec<(PathBuf, Option<FileStamp>)> {
    paths
        .into_iter()
        .map(|path| {
            let stamp = FileStamp::read(&path);
            (path, stamp)
        })
        .collect()
}

/// Compose files plus the `.env` Compose loads: `--env-file` when given, else the
/// one in the working directory or next to the first compose file (Compose's
/// default project directory), whichever exists.
fn watched_files(project: &ComposeProject) -> Vec<PathBuf> {
    let mut files = project.absolute_files();
    match &project.env_file {
        Some(env_file) => files.push(project.working_dir.join(env_file)),
        None => {
            let mut dirs = vec![project.working_dir.clone()];
            if let Some(dir) = files.first().and_then(|file| file.parent())
                && !dirs.iter().any(|known| known == dir)
            {
                dirs.push(dir.to_owned());
            }
            files.extend(dirs.into_iter().map(|dir| dir.join(".env")));
        }
    }
    files
}

/// A stable in-process hash; nothing outside this daemon run compares it.
fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// One cached value and the inputs it was read from.
#[derive(Debug)]
struct Entry<T> {
    fingerprint: Fingerprint,
    value: T,
    /// False when the project reads files the fingerprint does not cover
    /// ([`ConfigKey::untracked`]); the value then only serves [`Store::last`].
    reusable: bool,
}

/// Values per [`Slot`], valid while their [`Fingerprint`] matches.
#[derive(Debug)]
pub(crate) struct Store<T> {
    entries: Mutex<HashMap<Slot, Entry<T>>>,
}

impl<T> Default for Store<T> {
    /// An empty store.
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Clone> Store<T> {
    /// The value stored for `key` when nothing it was read from changed since and
    /// both `key` and the entry cover every file Compose reads.
    fn get(&self, key: &ConfigKey) -> Option<T> {
        if key.untracked.is_some() {
            return None;
        }
        // A panic while holding the lock cannot leave a half-written entry.
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = entries.get(&key.slot)?;
        (entry.reusable && entry.fingerprint == key.fingerprint).then(|| entry.value.clone())
    }

    /// The last value read for `slot`, even when its files changed since. For
    /// progress polling, where a model one edit old is good enough and a fresh one
    /// costs a Compose process per poll.
    pub(crate) fn last(&self, slot: &Slot) -> Option<T> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.get(slot).map(|entry| entry.value.clone())
    }

    /// Stores `value` for `key`, replacing what the slot held. The entry only
    /// serves [`Store::last`] when `key` is untracked.
    fn put(&self, key: ConfigKey, value: T) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.insert(
            key.slot,
            Entry {
                fingerprint: key.fingerprint,
                value,
                reusable: key.untracked.is_none(),
            },
        );
    }

    /// The value for `key`, from the store or else from `load`. `what` names the
    /// call in debug logs. Two concurrent misses both load; the later one wins the
    /// slot.
    pub(crate) async fn get_or_load<F, Fut>(
        &self,
        key: ConfigKey,
        what: &'static str,
        load: F,
    ) -> Result<T, Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        let dir = key.slot.working_dir.display().to_string();
        if let Some(value) = self.get(&key) {
            tracing::debug!(what, dir, hit = true, "compose config cache");
            return Ok(value);
        }
        let reason = match key.untracked {
            Some(reason) => reason,
            None => {
                let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
                match entries.get(&key.slot) {
                    None => "first read",
                    Some(entry) if !entry.reusable => "files outside the key",
                    Some(_) => "inputs changed",
                }
            }
        };
        tracing::debug!(what, dir, hit = false, reason, "compose config cache");
        let value = load().await?;
        self.put(key, value.clone());
        Ok(value)
    }
}

/// Everything [`crate::Compose`] caches from `docker compose config`.
#[derive(Debug, Default)]
pub(crate) struct ConfigCache {
    /// `config --format json`.
    pub(crate) models: Store<ComposeModel>,
    /// `config --hash '*'`.
    pub(crate) hashes: Store<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::*;

    fn project(dir: &Path) -> ComposeProject {
        ComposeProject {
            name: Some("acme".into()),
            working_dir: dir.to_owned(),
            files: vec!["compose.yaml".into()],
            env_file: None,
            profiles: Vec::new(),
            env: BTreeMap::from([("PORT".to_owned(), "5432".to_owned())]),
        }
    }

    /// Moves `path`'s modification time one minute back, as an edit with the same
    /// size would move it forward.
    fn touch_back(path: &Path) {
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open");
        let modified = file
            .metadata()
            .and_then(|metadata| metadata.modified())
            .expect("mtime");
        file.set_modified(modified - Duration::from_secs(60))
            .expect("set mtime");
    }

    #[tokio::test]
    async fn key_follows_files_and_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("compose.yaml"), "services: {}\n").expect("write");
        let project = project(dir.path());
        let first = ConfigKey::read(&project, &[]).await.expect("key");
        assert_eq!(first, ConfigKey::read(&project, &[]).await.expect("key"));

        touch_back(&dir.path().join("compose.yaml"));
        let touched = ConfigKey::read(&project, &[]).await.expect("key");
        assert_ne!(first, touched, "an mtime change invalidates");

        std::fs::write(dir.path().join(".env"), "A=1\n").expect("write");
        let with_env = ConfigKey::read(&project, &[]).await.expect("key");
        assert_ne!(touched, with_env, "a new default .env invalidates");

        let mut other_env = project.clone();
        other_env.env.insert("PORT".into(), "5433".into());
        let other = ConfigKey::read(&other_env, &[]).await.expect("key");
        assert_eq!(with_env.slot, other.slot);
        assert_ne!(with_env, other, "another interpolation value invalidates");
    }

    #[tokio::test]
    async fn extra_files_compare_by_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("compose.yaml"), "services: {}\n").expect("write");
        let extra = dir.path().join("override.yaml");
        std::fs::write(&extra, "a\n").expect("write");
        let project = project(dir.path());
        let extras = [extra.clone()];
        let first = ConfigKey::read(&project, &extras).await.expect("key");
        std::fs::write(&extra, "a\n").expect("rewrite");
        assert_eq!(
            first,
            ConfigKey::read(&project, &extras).await.expect("key"),
            "rewriting the same override keeps the entry"
        );
        std::fs::write(&extra, "b\n").expect("write");
        assert_ne!(
            first,
            ConfigKey::read(&project, &extras).await.expect("key")
        );
    }

    /// Loads `value` through `store` and counts the loads.
    async fn load(store: &Store<u32>, key: &ConfigKey, value: u32, loads: &AtomicUsize) -> u32 {
        store
            .get_or_load(key.clone(), "test", || async {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(value)
            })
            .await
            .expect("load")
    }

    #[tokio::test]
    async fn store_hits_until_the_key_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("compose.yaml"), "services: {}\n").expect("write");
        let project = project(dir.path());
        let key = ConfigKey::read(&project, &[]).await.expect("key");
        let store = Store::default();
        let loads = AtomicUsize::new(0);
        assert_eq!(load(&store, &key, 1, &loads).await, 1);
        assert_eq!(load(&store, &key, 2, &loads).await, 1, "hit");
        assert_eq!(loads.load(Ordering::SeqCst), 1);

        touch_back(&dir.path().join("compose.yaml"));
        let changed = ConfigKey::read(&project, &[]).await.expect("key");
        assert_eq!(load(&store, &changed, 3, &loads).await, 3, "miss");
        assert_eq!(loads.load(Ordering::SeqCst), 2);

        // An untracked key loads on every call, and `last` still answers for polling.
        let untracked = ConfigKey {
            untracked: Some("test"),
            ..changed
        };
        let store = Store::default();
        assert_eq!(load(&store, &untracked, 5, &loads).await, 5);
        assert_eq!(load(&store, &untracked, 6, &loads).await, 6);
        assert_eq!(store.last(&untracked.slot), Some(6));
    }

    #[tokio::test]
    async fn referenced_files_join_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("compose.yaml"),
            "services:\n  api:\n    image: acme/api\n    env_file: [./.env.api]\n",
        )
        .expect("write");
        let env_file = dir.path().join(".env.api");
        let project = project(dir.path());
        let missing = ConfigKey::read(&project, &[]).await.expect("key");
        assert_eq!(
            missing.untracked, None,
            "a referenced file keeps the key usable"
        );

        std::fs::write(&env_file, "A=1\n").expect("write");
        let created = ConfigKey::read(&project, &[]).await.expect("key");
        assert_ne!(missing, created, "a referenced file appearing invalidates");
        assert_eq!(created, ConfigKey::read(&project, &[]).await.expect("key"));

        touch_back(&env_file);
        let edited = ConfigKey::read(&project, &[]).await.expect("key");
        assert_ne!(created, edited, "an edit to a referenced file invalidates");

        let store = Store::default();
        let loads = AtomicUsize::new(0);
        assert_eq!(load(&store, &edited, 1, &loads).await, 1);
        assert_eq!(load(&store, &edited, 2, &loads).await, 1, "hit");
    }

    #[tokio::test]
    async fn interpolated_reference_is_untracked() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("compose.yaml"),
            "services:\n  api:\n    image: acme/api\n    env_file: ${ENV_FILE}\n",
        )
        .expect("write");
        let key = ConfigKey::read(&project(dir.path()), &[])
            .await
            .expect("key");
        assert_eq!(key.untracked, Some("interpolated file path"));

        let store = Store::default();
        let loads = AtomicUsize::new(0);
        assert_eq!(load(&store, &key, 1, &loads).await, 1);
        assert_eq!(load(&store, &key, 2, &loads).await, 2, "never hits");
    }
}
