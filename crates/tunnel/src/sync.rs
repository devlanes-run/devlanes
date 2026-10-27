//! Folder syncs streamed to a client (plan 14.5).
//!
//! On `follow`, the client sends its destination's manifest in batches, ended by
//! an end-of-input marker. Each batch is compared with the source as it arrives
//! ([`Syncer::offer_held`]); nothing is sent before the marker. Then a [`Syncer`]
//! writes into [`MuxTarget`], which turns each file into a manifest header plus
//! chunks, and each deletion into a delete message, so only differences travel.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use devlanes_client::{Client, Paths};
use devlanes_proto::{
    Context, Method,
    remote::{
        MAX_SYNC_FILE, ManifestBatch, SyncEntry, SyncFile, SyncManifest, SyncRequest,
        SyncStatusParams,
    },
};
use devlanes_sync::{ContentHash, HeldFile, Manifest, RelPath, SyncOptions, SyncTarget, Syncer};
use devlanes_transport::{SyncError, SyncFeed, SyncInput, SyncInputEvent, SyncMessage, SyncSource};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

/// File bytes per chunk; well under the transport's 8 MiB frame limit.
const CHUNK: usize = 1024 * 1024;

/// Syncs one client may follow at once; each holds a watcher and a task here.
const MAX_SUBSCRIPTIONS: usize = 16;

/// The entries the box offered clients when it was asked after registry
/// change `.0`; `None` before the first answer.
type Offered = (u64, Option<Arc<Vec<SyncEntry>>>);

/// [`SyncSource`] for the entries the box daemon lists in `sync.status`.
pub(crate) struct DaemonSync {
    paths: Paths,
    slots: Arc<Semaphore>,
    /// Counts the registry changes a session reports.
    changes: watch::Sender<u64>,
    /// What the box offers, asked once per registry change for every follow
    /// at once ([`look_up_offers`]), so each follow can check that its entry
    /// is still offered.
    offered: Arc<watch::Sender<Offered>>,
    /// Starts [`look_up_offers`] with the first follow.
    looking_up: std::sync::Once,
}

impl DaemonSync {
    /// Looks entries up through the daemon behind `paths`.
    pub(crate) fn new(paths: Paths) -> Self {
        Self {
            paths,
            slots: Arc::new(Semaphore::new(MAX_SUBSCRIPTIONS)),
            changes: watch::Sender::new(0),
            offered: Arc::new(watch::Sender::new((0, None))),
            looking_up: std::sync::Once::new(),
        }
    }

    /// Streams one entry until the client drops the feed or the box stops
    /// offering the entry (its service became hidden, or its lane stopped);
    /// `read_only_modes` as the client's request says (see
    /// [`SyncOptions::read_only_modes`]).
    async fn follow(
        &self,
        lane: String,
        unit: String,
        index: usize,
        read_only_modes: bool,
        input: SyncInput,
    ) -> Result<SyncFeed, SyncError> {
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| {
            SyncError::new(format!(
                "this client already follows {MAX_SUBSCRIPTIONS} syncs on the box"
            ))
        })?;
        self.looking_up.call_once(|| {
            tokio::spawn(look_up_offers(
                self.paths.clone(),
                self.changes.subscribe(),
                self.offered.clone(),
            ));
        });
        let wanted = (lane, unit, index);
        // Taken before this follow's own lookup, so an answer to a change
        // during it is checked and older answers are not.
        let since = *self.changes.borrow();
        let offered = self.offered.subscribe();
        let entry = entries(&self.paths)
            .await?
            .into_iter()
            .find(|entry| offers(entry, &wanted))
            .ok_or_else(|| {
                let (lane, unit, index) = &wanted;
                SyncError::new(format!(
                    "{lane}:{unit} has no sync entry {index} running on this box"
                ))
            })?;
        let (revoke, revoked) = watch::channel(false);
        tokio::spawn(revoke_when_withdrawn(offered, since, wanted, revoke));
        follow_entry(&entry, read_only_modes, input, permit, revoked)
    }
}

/// Asks the box daemon what it offers after each registry change in
/// `changes` while a follow listens on `offered`, once for all of them; a
/// burst of changes during one answer costs one more. Ends with the
/// [`DaemonSync`], which holds the sender of `changes`.
async fn look_up_offers(
    paths: Paths,
    mut changes: watch::Receiver<u64>,
    offered: Arc<watch::Sender<Offered>>,
) {
    while changes.changed().await.is_ok() {
        let change = *changes.borrow_and_update();
        if offered.receiver_count() == 0 {
            continue;
        }
        match entries(&paths).await {
            Ok(listed) => {
                offered.send_replace((change, Some(Arc::new(listed))));
            }
            Err(error) => tracing::debug!(%error, "cannot check the followed sync entries"),
        }
    }
}

/// Sync entries of the box's running lanes that a client may follow,
/// without hidden services' entries. Listing, following and the check that a
/// followed entry is still offered all go through here.
async fn entries(paths: &Paths) -> Result<Vec<SyncEntry>, SyncError> {
    let mut client = Client::connect(paths)
        .await
        .map_err(|error| SyncError::new(format!("box daemon: {error}")))?;
    let params = serde_json::to_value(SyncStatusParams { for_clients: true })
        .map_err(|error| SyncError::new(error.to_string()))?;
    let data = client
        .request(Method::SyncStatus, params, Context::default())
        .await
        .map_err(|error| SyncError::new(format!("sync.status: {error}")))?;
    serde_json::from_value(data.get("items").cloned().unwrap_or(Value::Null))
        .map_err(|error| SyncError::new(format!("sync.status: {error}")))
}

/// Whether `entry` is the one a follow named: `(lane, unit, index)`.
fn offers(entry: &SyncEntry, wanted: &(String, String, usize)) -> bool {
    entry.lane == wanted.0 && entry.unit == wanted.1 && entry.index == wanted.2
}

/// Sets `revoke` once the box no longer offers the `wanted` entry to clients,
/// as `offered` says after a registry change later than `since`, the way a
/// data stream to a unit that stopped or became hidden is closed. Ends with
/// the follow, which drops the receiver.
async fn revoke_when_withdrawn(
    mut offered: watch::Receiver<Offered>,
    since: u64,
    wanted: (String, String, usize),
    revoke: watch::Sender<bool>,
) {
    loop {
        tokio::select! {
            () = revoke.closed() => return,
            changed = offered.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
        let withdrawn = match &*offered.borrow_and_update() {
            (change, Some(listed)) if *change > since => {
                !listed.iter().any(|entry| offers(entry, &wanted))
            }
            _ => false,
        };
        if withdrawn {
            let (lane, unit, index) = &wanted;
            tracing::debug!(%lane, %unit, index, "a followed sync entry is no longer offered");
            let _ = revoke.send(true);
            return;
        }
    }
}

/// Feeds one sync entry to a client whose manifest arrives on `input`.
///
/// A client that leaves before its end marker leaves nothing behind: the next
/// subscription starts over with a fresh manifest. Modes compare by write
/// access when either side keeps only a read-only attribute: the client says
/// so in its request, and a Windows box knows it of itself, since its files
/// all report `0o644` or `0o444` and an executable a Unix client holds would
/// otherwise be resent without its exec bits on every connect.
///
/// The feed ends cleanly once `revoked` turns true, at any point; a sender
/// that is gone revokes nothing.
pub(crate) fn follow_entry(
    entry: &SyncEntry,
    read_only_modes: bool,
    mut input: SyncInput,
    permit: OwnedSemaphorePermit,
    mut revoked: watch::Receiver<bool>,
) -> Result<SyncFeed, SyncError> {
    let (messages, receiver) = mpsc::channel(16);
    let target = Arc::new(MuxTarget {
        messages: messages.clone(),
    });
    let options = SyncOptions {
        ignore: entry.ignore.clone(),
        max_file: Some(MAX_SYNC_FILE),
        read_only_modes: read_only_modes || devlanes_sync::READ_ONLY_MODES,
        ..SyncOptions::default()
    };
    let mut syncer = Syncer::new(&entry.from, target, options)
        .map_err(|error| SyncError::new(error.to_string()))?;
    tokio::spawn(async move {
        let _permit = permit;
        loop {
            let event = tokio::select! {
                () = messages.closed() => return,
                Ok(_) = revoked.wait_for(|revoked| *revoked) => return,
                event = input.recv() => event,
            };
            match event {
                Some(SyncInputEvent::Value(value)) => {
                    let batch = match serde_json::from_value::<ManifestBatch>(value) {
                        Ok(batch) => batch,
                        Err(error) => {
                            let message = format!("bad manifest batch: {error}");
                            let _ = messages.send(Err(SyncError::new(message))).await;
                            return;
                        }
                    };
                    let held = batch.files.into_iter().filter_map(held_file).collect();
                    if let Err(error) = syncer.offer_held(held).await {
                        let _ = messages.send(Err(SyncError::new(error.to_string()))).await;
                        return;
                    }
                }
                Some(SyncInputEvent::End) => break,
                // The client left mid-manifest.
                None => return,
            }
        }
        drop(input);
        // `start` returns after the initial pass, which needs the feed being read.
        match syncer.start().await {
            Ok(handle) => {
                if let Ok(ready) = serde_json::to_value(SyncManifest::Ready)
                    && messages
                        .send(Ok(SyncMessage::Manifest(ready)))
                        .await
                        .is_ok()
                {
                    tokio::select! {
                        () = messages.closed() => {}
                        Ok(_) = revoked.wait_for(|revoked| *revoked) => {}
                    }
                }
                // Stopping drops the syncer's target, and with it the last
                // sender, so the feed ends.
                handle.stop().await;
            }
            Err(error) => {
                let _ = messages.send(Err(SyncError::new(error.to_string()))).await;
            }
        }
    });
    Ok(feed(receiver))
}

/// The receiver as a feed.
fn feed(receiver: mpsc::Receiver<Result<SyncMessage, SyncError>>) -> SyncFeed {
    futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    })
    .boxed()
}

/// A client manifest entry as a held file; an invalid path is dropped (it cannot
/// name a source file), an unreadable hash means "send or delete it".
fn held_file(file: SyncFile) -> Option<HeldFile> {
    Some(HeldFile {
        path: RelPath::new(file.path).ok()?,
        size: file.size,
        mode: file.mode,
        hash: ContentHash::from_hex(&file.hash),
        mtime: None,
    })
}

#[async_trait]
impl SyncSource for DaemonSync {
    /// Lists entries or follows one.
    async fn subscribe(&self, request: Value, input: SyncInput) -> Result<SyncFeed, SyncError> {
        match serde_json::from_value(request)
            .map_err(|error| SyncError::new(format!("bad sync request: {error}")))?
        {
            SyncRequest::List => {
                let entries = SyncManifest::Entries {
                    entries: entries(&self.paths).await?,
                };
                let entries = serde_json::to_value(entries)
                    .map_err(|error| SyncError::new(error.to_string()))?;
                Ok(
                    futures_util::stream::once(async move { Ok(SyncMessage::Manifest(entries)) })
                        .boxed(),
                )
            }
            SyncRequest::Follow {
                lane,
                unit,
                index,
                read_only_modes,
            } => self.follow(lane, unit, index, read_only_modes, input).await,
        }
    }

    /// Makes every follow check that its entry is still offered.
    fn registry_changed(&self) {
        self.changes
            .send_modify(|count| *count = count.wrapping_add(1));
    }
}

/// The client end of a sync as a [`SyncTarget`].
struct MuxTarget {
    messages: mpsc::Sender<Result<SyncMessage, SyncError>>,
}

impl MuxTarget {
    /// Queues one message; fails once the client is gone, which stops the syncer's pass.
    async fn send(&self, message: SyncMessage) -> Result<(), devlanes_sync::SyncError> {
        self.messages
            .send(Ok(message))
            .await
            .map_err(|_| devlanes_sync::SyncError::Target("client disconnected".into()))
    }
}

#[async_trait]
impl SyncTarget for MuxTarget {
    /// Never read: the client's manifest arrives through [`Syncer::offer_held`]
    /// (an empty manifest means an empty destination).
    async fn manifest(&self) -> Result<Manifest, devlanes_sync::SyncError> {
        Ok(Manifest::new())
    }

    /// A `file` header, then the contents in chunks.
    async fn write_file(
        &self,
        path: &RelPath,
        contents: Bytes,
        mode: u32,
    ) -> Result<(), devlanes_sync::SyncError> {
        let header = SyncManifest::File {
            path: path.as_str().to_owned(),
            mode,
            size: contents.len() as u64,
        };
        let header = serde_json::to_value(header)
            .map_err(|error| devlanes_sync::SyncError::Target(error.to_string()))?;
        self.send(SyncMessage::Manifest(header)).await?;
        let mut offset = 0;
        loop {
            let end = (offset + CHUNK).min(contents.len());
            self.send(SyncMessage::Chunk {
                path: path.as_str().to_owned(),
                offset: offset as u64,
                data: contents.slice(offset..end),
                last: end == contents.len(),
            })
            .await?;
            if end == contents.len() {
                break;
            }
            offset = end;
        }
        Ok(())
    }

    /// A deletion message.
    async fn remove(&self, path: &RelPath) -> Result<(), devlanes_sync::SyncError> {
        self.send(SyncMessage::Delete {
            path: path.as_str().to_owned(),
        })
        .await
    }

    /// Remote: no local destination to compare against the source.
    fn local_root(&self) -> Option<&Path> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use devlanes_sync::{HashCache, LocalTarget};
    use devlanes_transport::{
        BoxServices, LoopbackDialer, RegistrySnapshot, ServeOptions, Session, SessionOptions,
        SharedRegistry, SyncSubscription, memory::memory_transport, serve,
    };
    use serde_json::json;

    use super::*;

    /// A box serving one fixed sync entry over an in-memory transport.
    struct TestSource {
        entry: SyncEntry,
        slots: Arc<Semaphore>,
    }

    #[async_trait]
    impl SyncSource for TestSource {
        async fn subscribe(&self, request: Value, input: SyncInput) -> Result<SyncFeed, SyncError> {
            let permit = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| SyncError::new("busy"))?;
            let read_only_modes = request["read_only_modes"].as_bool().unwrap_or(false);
            // A dropped sender: nothing revokes these follows.
            let (_, revoked) = watch::channel(false);
            follow_entry(&self.entry, read_only_modes, input, permit, revoked)
        }
    }

    /// Starts the box and connects a client session.
    async fn connect(source: &Path) -> Session {
        let (transport, mut listener) = memory_transport();
        let services = BoxServices {
            registry: Arc::new(SharedRegistry::new(RegistrySnapshot::default(), 4)),
            dialer: Arc::new(LoopbackDialer),
            pairing: Arc::new(devlanes_transport::NoPairing),
            sync: Arc::new(TestSource {
                entry: SyncEntry {
                    lane: "app@0".into(),
                    project: "app".into(),
                    unit: "web".into(),
                    index: 0,
                    from: source.to_path_buf(),
                    leaf: "ext".into(),
                    ignore: Vec::new(),
                },
                slots: Arc::new(Semaphore::new(4)),
            }),
        };
        tokio::spawn(async move {
            while let Some(link) = listener.accept().await {
                let services = services.clone();
                tokio::spawn(serve(link, services, ServeOptions::new("test-box")));
            }
        });
        Session::connect(&transport, SessionOptions::new("test"))
            .await
            .expect("session")
    }

    /// Opens a follow subscription.
    async fn follow(session: &Session) -> SyncSubscription {
        let request = json!({"op": "follow", "lane": "app@0", "unit": "web", "index": 0});
        session.subscribe_sync(request).await.expect("subscribe")
    }

    /// Sends a destination's manifest in batches of `batch`.
    async fn send_manifest(sync: &mut SyncSubscription, destination: &Path, batch: usize) {
        let mut walk = LocalTarget::new(destination).held(HashCache::default(), batch);
        while let Some(files) = walk.next().await {
            let files = files
                .into_iter()
                .map(|file| SyncFile {
                    path: file.path.as_str().to_owned(),
                    size: file.size,
                    mode: file.mode,
                    hash: file.hash.map(|hash| hash.to_hex()).unwrap_or_default(),
                    mtime_ms: None,
                })
                .collect();
            let batch = serde_json::to_value(ManifestBatch { files }).expect("batch");
            sync.send(batch).await.expect("send batch");
        }
    }

    /// Messages until `ready`, as `(written paths, deleted paths)`.
    async fn until_ready(sync: &mut SyncSubscription) -> (Vec<String>, Vec<String>) {
        let (mut written, mut deleted) = (Vec::new(), Vec::new());
        while let Some(message) = sync.next().await {
            match message.expect("message") {
                SyncMessage::Manifest(value) => match serde_json::from_value(value) {
                    Ok(SyncManifest::Ready) => return (written, deleted),
                    Ok(SyncManifest::File { path, .. }) => written.push(path),
                    _ => {}
                },
                SyncMessage::Delete { path } => deleted.push(path),
                SyncMessage::Chunk { .. } => {}
            }
        }
        panic!("the feed ended before ready");
    }

    /// Writes `count` files below `root` with settled mtimes.
    fn tree(root: &Path, count: usize) {
        let settled = std::time::SystemTime::now() - Duration::from_secs(60);
        for i in 0..count {
            let path = root.join(format!("d{}/f{i}.txt", i % 10));
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            std::fs::write(&path, format!("file {i}")).expect("write");
            std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(settled))
                .expect("mtime");
        }
    }

    /// The tunnel asks the box daemon only for the entries clients may follow,
    /// which leaves hidden services' entries on the box, and a follow of an
    /// entry the daemon did not list is refused.
    #[tokio::test]
    async fn only_entries_for_clients_are_listed_and_followed() -> crate::test_daemon::TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let paths = crate::test_daemon::home(dir.path())?;
            let daemon = crate::test_daemon::answer_one(&paths, |_| devlanes_proto::Outcome::Ok {
                data: json!({"items": []}),
            })
            .await?;
            let sync = DaemonSync::new(paths);
            let (_input, receiver) = mpsc::channel(1);
            let input = SyncInput::from_receiver(receiver);
            let refused = sync
                .follow("app@0".into(), "vault".into(), 0, false, input)
                .await;
            let Err(error) = refused else {
                panic!("followed an entry the daemon did not list");
            };
            assert!(error.to_string().contains("has no sync entry"), "{error}");
            drop(sync);
            let request = daemon.await??;
            assert_eq!(request.method, Method::SyncStatus);
            assert_eq!(request.params, json!({"for_clients": true}));
            Ok(())
        })
        .await?
    }

    /// Follows sync entry 0 of `app@0:web` on `sync` and reads its feed up
    /// to the end of the first pass.
    async fn follow_until_ready(
        sync: &DaemonSync,
    ) -> Result<SyncFeed, Box<dyn std::error::Error + Send + Sync>> {
        let (input, receiver) = mpsc::channel(4);
        let mut feed = sync
            .follow(
                "app@0".into(),
                "web".into(),
                0,
                false,
                SyncInput::from_receiver(receiver),
            )
            .await?;
        input
            .send(SyncInputEvent::End)
            .await
            .map_err(|_| "the follow stopped reading its input")?;
        loop {
            match feed.next().await.ok_or("the feed ended before ready")?? {
                SyncMessage::Manifest(value)
                    if serde_json::from_value::<SyncManifest>(value.clone())
                        .is_ok_and(|manifest| matches!(manifest, SyncManifest::Ready)) =>
                {
                    return Ok(feed);
                }
                _ => {}
            }
        }
    }

    /// Follows end cleanly once a registry change finds their entry no longer
    /// offered (the service became hidden or its lane stopped), and a change
    /// that keeps it offered leaves them running. The box daemon is asked
    /// once per change, however many follows there are.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_follow_ends_when_its_entry_is_withdrawn() -> crate::test_daemon::TestResult {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let source = dir.path().join("src");
            tree(&source, 3);
            let paths = crate::test_daemon::home(dir.path())?;
            let offered = Arc::new(AtomicBool::new(true));
            let asked = Arc::new(AtomicUsize::new(0));
            let (listing, counting) = (offered.clone(), asked.clone());
            let entry = json!({
                "lane": "app@0", "project": "app", "unit": "web", "index": 0,
                "from": source, "leaf": "src",
            });
            let daemon = crate::test_daemon::answer_all(&paths, move |_| {
                counting.fetch_add(1, Ordering::SeqCst);
                let items = if listing.load(Ordering::SeqCst) {
                    vec![entry.clone()]
                } else {
                    Vec::new()
                };
                devlanes_proto::Outcome::Ok {
                    data: json!({"items": items}),
                }
            })
            .await?;
            let sync = DaemonSync::new(paths);
            let mut first = follow_until_ready(&sync).await?;
            let mut second = follow_until_ready(&sync).await?;
            assert_eq!(asked.load(Ordering::SeqCst), 2, "one lookup per follow");

            // Still offered: the follows go on.
            sync.registry_changed();
            while asked.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let quiet = tokio::time::timeout(Duration::from_millis(500), first.next()).await;
            assert!(
                quiet.is_err(),
                "the feed moved: {:?}",
                quiet.ok().flatten().map(|m| m.is_ok())
            );
            assert_eq!(asked.load(Ordering::SeqCst), 3, "one lookup for both");

            offered.store(false, Ordering::SeqCst);
            sync.registry_changed();
            for feed in [&mut first, &mut second] {
                let ended = tokio::time::timeout(Duration::from_secs(10), feed.next()).await?;
                assert!(ended.is_none(), "the feed ends cleanly");
            }
            assert_eq!(asked.load(Ordering::SeqCst), 4, "one lookup for both");
            daemon.abort();
            Ok(())
        })
        .await?
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deletions_come_only_after_the_end_marker() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir().expect("tempdir");
            let (source, destination) = (dir.path().join("src"), dir.path().join("dst"));
            tree(&source, 20);
            tree(&destination, 20);
            std::fs::write(destination.join("stale.txt"), b"gone on the box").expect("stale");
            std::fs::write(destination.join("d3/f3.txt"), b"changed here").expect("changed");
            let session = connect(&source).await;
            let mut sync = follow(&session).await;
            send_manifest(&mut sync, &destination, 5).await;
            // Before the end marker the box sends nothing, deletions included.
            let early = tokio::time::timeout(Duration::from_millis(300), sync.next()).await;
            assert!(early.is_err(), "box sent {early:?} before the end marker");
            sync.finish_input().await.expect("end");
            let (written, deleted) = until_ready(&mut sync).await;
            assert_eq!(written, ["d3/f3.txt"]);
            assert_eq!(deleted, ["stale.txt"]);
        })
        .await
        .expect("timeout");
    }

    /// A client that keeps only a read-only attribute (Windows) reports `0o644`
    /// for an executable it holds; with `read_only_modes` the box does not resend
    /// it, without it a Unix box sends it to restore the mode.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn read_only_clients_compare_modes_by_write_access() {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir().expect("tempdir");
            let (source, destination) = (dir.path().join("src"), dir.path().join("dst"));
            tree(&source, 3);
            tree(&destination, 3);
            let script = source.join("d1/f1.txt");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            let session = connect(&source).await;
            for (read_only, expected) in [
                (true, Vec::<String>::new()),
                (false, vec!["d1/f1.txt".to_owned()]),
            ] {
                let request = json!({
                    "op": "follow", "lane": "app@0", "unit": "web", "index": 0,
                    "read_only_modes": read_only,
                });
                let mut sync = session.subscribe_sync(request).await.expect("subscribe");
                send_manifest(&mut sync, &destination, 10).await;
                sync.finish_input().await.expect("end");
                let (written, deleted) = until_ready(&mut sync).await;
                assert_eq!(written, expected, "read_only_modes: {read_only}");
                assert!(deleted.is_empty(), "{deleted:?}");
            }
        })
        .await
        .expect("timeout");
    }

    /// A Windows box knows only the read-only attribute of its files, so it
    /// compares modes by write access whatever the client asks: a Unix
    /// client's executable (`0o755`) is left alone, and a file the box made
    /// read-only is sent again with `0o444`. Paths travel with `/`.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread")]
    async fn windows_boxes_compare_modes_by_write_access() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir().expect("tempdir");
            let (source, destination) = (dir.path().join("src"), dir.path().join("dst"));
            tree(&source, 3);
            tree(&destination, 3);
            let locked = source.join("d2").join("f2.txt");
            let mut permissions = std::fs::metadata(&locked).expect("meta").permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&locked, permissions).expect("read-only");
            let session = connect(&source).await;
            let request = json!({
                "op": "follow", "lane": "app@0", "unit": "web", "index": 0,
                "read_only_modes": false,
            });
            let mut sync = session.subscribe_sync(request).await.expect("subscribe");
            let mut walk = LocalTarget::new(&destination).held(HashCache::default(), 10);
            while let Some(files) = walk.next().await {
                let files = files
                    .into_iter()
                    .map(|file| SyncFile {
                        mode: if file.path.as_str() == "d1/f1.txt" {
                            0o755
                        } else {
                            file.mode
                        },
                        path: file.path.as_str().to_owned(),
                        size: file.size,
                        hash: file.hash.map(|hash| hash.to_hex()).unwrap_or_default(),
                        mtime_ms: None,
                    })
                    .collect();
                let batch = serde_json::to_value(ManifestBatch { files }).expect("batch");
                sync.send(batch).await.expect("send batch");
            }
            sync.finish_input().await.expect("end");
            let mut written = Vec::new();
            while let Some(message) = sync.next().await {
                match message.expect("message") {
                    SyncMessage::Manifest(value) => match serde_json::from_value(value) {
                        Ok(SyncManifest::Ready) => break,
                        Ok(SyncManifest::File { path, mode, .. }) => written.push((path, mode)),
                        _ => {}
                    },
                    SyncMessage::Delete { path } => panic!("deleted {path}"),
                    SyncMessage::Chunk { .. } => {}
                }
            }
            assert_eq!(written, [("d2/f2.txt".to_owned(), 0o444)]);
        })
        .await
        .expect("timeout");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_subscription_dropped_mid_manifest_starts_over_cleanly() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir().expect("tempdir");
            let (source, destination) = (dir.path().join("src"), dir.path().join("dst"));
            tree(&source, 30);
            tree(&destination, 30);
            std::fs::write(destination.join("stale.txt"), b"x").expect("stale");
            let session = connect(&source).await;
            // Half a manifest, then the client goes away.
            let mut partial = follow(&session).await;
            let mut walk = LocalTarget::new(&destination).held(HashCache::default(), 10);
            let first = walk.next().await.expect("batch");
            let files = first
                .into_iter()
                .map(|file| SyncFile {
                    path: file.path.as_str().to_owned(),
                    size: file.size,
                    mode: file.mode,
                    hash: file.hash.map(|hash| hash.to_hex()).unwrap_or_default(),
                    mtime_ms: None,
                })
                .collect();
            let batch = serde_json::to_value(ManifestBatch { files }).expect("batch");
            partial.send(batch).await.expect("send");
            drop(partial);
            drop(walk.finish().await);
            // A new subscription with the whole manifest sees only the real difference,
            // and the dropped one freed its slot.
            for _ in 0..4 {
                let mut sync = follow(&session).await;
                send_manifest(&mut sync, &destination, 10).await;
                sync.finish_input().await.expect("end");
                let (written, deleted) = until_ready(&mut sync).await;
                assert!(written.is_empty(), "{written:?}");
                assert_eq!(deleted, ["stale.txt"]);
            }
        })
        .await
        .expect("timeout");
    }
}
