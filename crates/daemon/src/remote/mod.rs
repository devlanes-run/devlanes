//! Remote operation (plan M14). Remote is mirroring only: a client never runs
//! commands on a box, and nothing in this path carries environments or secrets.
//!
//! Client side: `connect` keeps one reconnecting [`Link`] per box in the daemon,
//! mirrors the box's registry ([`mirror`]: same-number loopback listeners and proxy
//! routes) and applies its folder syncs ([`sync`]) below
//! `$DEVLANES_HOME/sync/<host>/`; every box knows this machine by the id and
//! name in [`identity`]. A box that connected stays wanted in `remote_hosts`
//! until `disconnect`, and a starting daemon connects it again. A reconnect
//! that needs a person (a prompt without an attached CLI, rejected
//! credentials after a first success) waits in `needs_auth` for the next
//! `connect`, keeping listeners and routes.
//!
//! Client and box exchange [`report`]s of what the client mirrors: the client
//! sends one after its mirror changed, and the box keeps the latest per client.
//!
//! Which lane serves hostnames that several lanes claim, local or on a box, is
//! one decision for the whole client ([`assign`]), recomputed whenever a
//! registry, a link, a local lane or a choice (`remote.use`, [`choice`])
//! changes. The mirrors and the route publisher read it.
//!
//! A box that requires pairing serves a client only with the remote token it
//! gave that client for a one-time code ([`pairing`]). The client keeps the
//! token per box; without a valid one and nobody to type a code, its link
//! waits in `needs_pairing`, keeping listeners and routes like `needs_auth`,
//! until `remote connect` (with `--code` or a prompt) pairs it again.
//!
//! Box side: `remote.registry` ([`feed`]) streams what a tunnel's client
//! mirrors, `status` lists the tunnels serving clients with their reports,
//! `url` and `hosts` add the URLs each client opens, and `sync.status` lists
//! the sync entries a tunnel may stream. `remote.pair`, `remote.pairing.*`,
//! `remote.clients` and `remote.revoke` manage pairings.

mod assign;
mod choice;
mod feed;
mod identity;
mod mirror;
mod pairing;
mod report;
mod sync;

pub(crate) use assign::Assignment;
pub(crate) use report::{HIDDEN, UnitTarget};

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use bytes::Bytes;
use devlanes_proto::{
    Event, RpcError,
    remote::{self as wire, RemoteClient, SyncEntry},
};
use devlanes_sync::{HashCache, RelPath};
use devlanes_transport::{
    BoxDuplex, BoxInfo, ClientRegistry, Link, LinkState, Session as BoxSession, SessionOptions,
    Transport, TransportError,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Session,
    engine::{Inner, required},
    integrations::{RemotePrompt, RemotePrompter, RouteEntry},
    model::lock,
};

/// `connect --background` returns once connected, failed, or after this long.
const CONNECT_WAIT: Duration = Duration::from_secs(150);
/// Unanswered SSH and pairing code prompts are cancelled after this long. The
/// attempt's `connect_timeout` (3 minutes) counts the time a person takes too,
/// so a late SSH answer leaves less of it for the code; past it the attempt
/// times out, its prompt is dropped and the link retries.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a local spawn waits for box mirrors to release its ports.
const RELEASE_WAIT: Duration = Duration::from_secs(3);
/// How long `connect` waits for a connection's record; longer than the
/// store's 5 s busy timeout, after which the write fails and is only logged.
const RECORD_WAIT: Duration = Duration::from_secs(10);

/// Stream ids for prompts start far above the engine's own attach ids, so both
/// can share one client connection.
static NEXT_STREAM: AtomicU64 = AtomicU64::new(1 << 40);

/// Remote state owned by the engine.
#[derive(Default)]
pub(crate) struct Remotes {
    hosts: Mutex<BTreeMap<String, Arc<HostConn>>>,
    /// Serializes connects so a replaced connection is gone before its successor
    /// binds the same ports, and orders `disconnect` against the restore so a
    /// box disconnected meanwhile is not opened again.
    connecting: tokio::sync::Mutex<()>,
    /// Box side: tunnels currently serving a client, for `status`, each with
    /// the serial of the [`ClientGuard`] that listed it.
    clients: Mutex<BTreeMap<ClientKey, (u64, RemoteClient)>>,
    next_client: AtomicU64,
    /// Box side: every open `remote.registry` feed by its serial, with its
    /// client's id and the token `daemon revoke` cancels to end it. Apart from
    /// `clients`, where a newer connection of a client replaces the entry
    /// while the older feed may still run.
    feeds: Mutex<BTreeMap<u64, (String, CancellationToken)>>,
    /// Box side: outstanding pairing codes and recent failed attempts.
    codes: Mutex<pairing::Codes>,
    /// What this machine sends every box, read from disk on the first connect.
    identity: tokio::sync::OnceCell<identity::ClientIdentity>,
    /// Hands each new connection its place in connect order ([`HostConn::serial`]).
    next_serial: AtomicU64,
    /// Extra HTTPS ports the proxy serves for boxes whose proxy port differs from
    /// this machine's ([`Inner::reconcile_box_ports`]). The lock also serializes
    /// opening and closing them.
    box_ports: tokio::sync::Mutex<BoxPorts>,
    /// Set while a starting daemon reopens the boxes an earlier one had
    /// connected ([`Inner::restore_remotes`]).
    restoring: AtomicBool,
    /// The restore task, awaited on shutdown before the store closes.
    restore: Mutex<Option<JoinHandle<()>>>,
    /// Why the restore could not open a wanted box, by name, shown by `remote
    /// ls` until the box connects or is disconnected.
    restore_errors: Mutex<BTreeMap<String, String>>,
    /// Which box serves a lane several lanes clash on, lane reference to box,
    /// as `remote_lane_choices` holds it.
    choices: Mutex<BTreeMap<String, String>>,
    /// Set once `choices` was read from the store.
    choices_loaded: tokio::sync::OnceCell<()>,
    /// The latest [`assign::Assignment`] and its version, which goes up with
    /// every change.
    assignment: Mutex<(u64, Arc<assign::Assignment>)>,
    /// Held while the assignment is computed and stored, so one computed
    /// from older inputs never replaces a newer one.
    assigning: Mutex<()>,
    /// This machine's lanes in a start that has not spawned yet, with how
    /// many starts each. They count as running, so they win at once.
    starting: Mutex<BTreeMap<String, usize>>,
}

/// How often box HTTPS ports that could not be opened are tried again: nothing
/// tells the daemon when another process frees one.
const BOX_PORT_RETRY: Duration = Duration::from_secs(30);

/// The box HTTPS ports the proxy serves or failed to serve.
#[derive(Default)]
struct BoxPorts {
    open: BTreeSet<u16>,
    /// Ports a box needs that could not be opened, retried every [`BOX_PORT_RETRY`].
    failed: BTreeSet<u16>,
    /// The retry timer, while `failed` is not empty.
    retry: Option<JoinHandle<()>>,
}

/// Why the proxy cannot serve a box's HTTPS port, as `proxy_port_in_use` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoxPortProblem {
    /// `in use by nginx, pid 812`, or why the bind failed.
    holder: String,
    pid: Option<u32>,
}

/// The `remote.warning` code and `HostConn::warnings` key of a box whose
/// `ports.range` overlaps another connected box's.
const RANGE_OVERLAP: &str = "ports_range_overlap";

impl Remotes {
    /// No box is connected, no tunnel serves a client and no pairing code is
    /// outstanding, so an idle exit would cut nobody off or void a code
    /// someone is about to type. A box counts as connected while its link
    /// reconnects or waits for a sign-in or pairing, and while a starting
    /// daemon restores it, so a daemon with wanted boxes stays up for them. A
    /// box whose link failed for good does not count: it mirrors nothing, and
    /// it stays wanted, so the next daemon tries it again.
    pub(crate) fn is_idle(&self) -> bool {
        lock(&self.hosts).values().all(|conn| conn.ended())
            && lock(&self.clients).is_empty()
            && !self.restoring.load(Ordering::Relaxed)
            && !lock(&self.codes).outstanding(std::time::Instant::now())
    }

    /// Mirrored routes of every connected box, in connect order, of the lanes
    /// `assignment` mirrors. A box that lost a lane may not have dropped its
    /// routes yet, and the winner's come in only once it built them, so a
    /// hostname is never routed to two boxes.
    pub(crate) fn routes(&self, assignment: &assign::Assignment) -> Vec<RouteEntry> {
        self.all()
            .iter()
            .flat_map(|conn| {
                lock(&conn.view)
                    .routes
                    .iter()
                    .filter(|route| assignment.mirrors(&conn.name, &route.lane))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Ports of every live box's non-hidden units, idle ones included, which new
    /// local port blocks keep clear of: both machines allocate from the same
    /// default pool, so their first lanes would otherwise share numbers.
    pub(crate) fn advertised_ports(&self) -> BTreeSet<u16> {
        self.all()
            .iter()
            .filter(|conn| !conn.ended())
            .flat_map(|conn| conn.registry.current().advertised_ports())
            .collect()
    }

    /// Ends the registry feed, and with it the session, of every client whose
    /// id `ended` picks; returns how many feeds it ended.
    fn end_feeds(&self, ended: impl Fn(&str) -> bool) -> usize {
        let mut count = 0;
        for (id, revoked) in lock(&self.feeds).values() {
            if ended(id) {
                revoked.cancel();
                count += 1;
            }
        }
        count
    }

    /// Every connection, in connect order.
    fn all(&self) -> Vec<Arc<HostConn>> {
        let mut all: Vec<Arc<HostConn>> = lock(&self.hosts).values().cloned().collect();
        all.sort_by_key(|conn| conn.serial);
        all
    }

    /// Makes every mirror but `serial`'s reconcile again, after that box's routes
    /// or bound ports changed or it went away: a later box may now skip or regain
    /// a lane, and an earlier one may bind a port it released.
    fn wake_others(&self, serial: u64) {
        for conn in self.all() {
            if conn.serial != serial {
                conn.wake.notify_one();
            }
        }
    }
}

/// Where a prompt's answer goes; `None` means cancelled.
type Answer = oneshot::Sender<Option<String>>;

/// CLIs attached to `connect` and the prompts waiting for them.
#[derive(Default)]
struct PromptBoard {
    watchers: Mutex<Vec<Session>>,
    /// Pending prompts by `(session, stream)`.
    pending: Mutex<HashMap<(u64, u64), Answer>>,
    /// Set when the transport asked something while no CLI was attached;
    /// [`Attended`] reads and clears it around each attempt.
    unanswered: AtomicBool,
}

impl PromptBoard {
    /// Registers `session` as a prompt target until the guard drops.
    fn watch(self: &Arc<Self>, session: &Session) -> WatchGuard {
        lock(&self.watchers).push(session.clone());
        WatchGuard {
            board: self.clone(),
            id: session.id(),
        }
    }

    /// Delivers an answer frame; an empty frame cancels. False when not a prompt.
    fn answer(&self, session: u64, stream: u64, data: &[u8]) -> bool {
        let Some(waiting) = lock(&self.pending).remove(&(session, stream)) else {
            return false;
        };
        let answer = (!data.is_empty()).then(|| {
            String::from_utf8_lossy(data)
                .trim_end_matches(['\r', '\n'])
                .to_owned()
        });
        let _ = waiting.send(answer);
        true
    }
}

/// Removes a `connect` CLI from the prompt targets.
struct WatchGuard {
    board: Arc<PromptBoard>,
    id: u64,
}

impl Drop for WatchGuard {
    /// Forgets the session.
    fn drop(&mut self) {
        lock(&self.board.watchers).retain(|session| session.id() != self.id);
    }
}

/// Sends ssh's login prompts to the most recently attached `connect` CLI.
struct HostPrompter {
    board: Arc<PromptBoard>,
}

#[async_trait]
impl RemotePrompter for HostPrompter {
    /// Emits a prompt event and waits for the CLI's answer frame.
    async fn ask(&self, prompt: RemotePrompt) -> Option<String> {
        let Some(session) = lock(&self.board.watchers).last().cloned() else {
            // A notice needs no answer; anything else leaves the attempt
            // waiting for a sign-in instead of failing or asking again.
            if prompt.kind != "notify" {
                self.board.unanswered.store(true, Ordering::Relaxed);
            }
            return None;
        };
        let stream_id = NEXT_STREAM.fetch_add(1, Ordering::Relaxed);
        let (answer_tx, answer) = oneshot::channel();
        lock(&self.board.pending).insert((session.id(), stream_id), answer_tx);
        // Also when the attempt's timeout or a disconnect drops this future.
        let _pending = PendingGuard {
            board: self.board.clone(),
            key: (session.id(), stream_id),
        };
        let event = Event {
            seq: 0,
            event: wire::PROMPT_EVENT.into(),
            data: json!({
                "host": prompt.host,
                "prompt": prompt.prompt,
                "kind": prompt.kind,
                "stream_id": stream_id,
            }),
        };
        if session.event(event).await.is_ok() {
            let disconnected = session.disconnected();
            tokio::select! {
                reply = answer => return reply.ok().flatten(),
                () = disconnected.cancelled() => {}
                () = tokio::time::sleep(PROMPT_TIMEOUT) => {}
            }
        }
        // Nobody answered, as when no CLI is attached: the attempt then waits
        // for a sign-in, so a `connect` attached later resumes it.
        if prompt.kind != "notify" {
            self.board.unanswered.store(true, Ordering::Relaxed);
        }
        None
    }
}

/// Removes a prompt from the pending ones once its `ask` ends or is dropped,
/// so an answer that comes later is not taken for a prompt.
struct PendingGuard {
    board: Arc<PromptBoard>,
    key: (u64, u64),
}

impl Drop for PendingGuard {
    /// Forgets the prompt.
    fn drop(&mut self) {
        lock(&self.board.pending).remove(&self.key);
    }
}

/// The box's transport, as the connector built it, with one more rule: an
/// attempt that asked a prompt while no CLI was attached and then failed
/// authentication needs a sign-in ([`TransportError::NeedsAuth`]), also on a
/// first connection, so the link waits for `remote connect` instead of
/// stopping. An attempt that failed for another reason keeps its class: a
/// transient failure is retried, whatever happened to the prompt.
struct Attended {
    inner: Arc<dyn Transport>,
    board: Arc<PromptBoard>,
}

#[async_trait]
impl Transport for Attended {
    /// Connects through the inner transport and reclassifies an
    /// authentication failure after an unanswered prompt.
    async fn connect(&self) -> Result<BoxDuplex, TransportError> {
        self.board.unanswered.store(false, Ordering::Relaxed);
        let result = self.inner.connect().await;
        let unanswered = self.board.unanswered.swap(false, Ordering::Relaxed);
        result.map_err(|error| {
            if unanswered {
                error.into_needs_auth()
            } else {
                error
            }
        })
    }
}

/// One `connect`ed box: the link, its mirror and its syncs.
pub(crate) struct HostConn {
    name: String,
    kind: &'static str,
    state: watch::Receiver<LinkState>,
    registry: ClientRegistry,
    link: tokio::sync::Mutex<Option<Link>>,
    cancel: CancellationToken,
    board: Arc<PromptBoard>,
    view: Mutex<mirror::View>,
    /// Makes the mirror reconcile now, for a local spawn waiting on a port.
    wake: tokio::sync::Notify,
    /// Waited for on close, so this host's listeners are gone afterwards.
    mirror_task: Mutex<Option<JoinHandle<()>>>,
    /// [`watch_link`], waited for on close, so a disconnect that clears
    /// `remote_hosts.wanted` comes after its last record of a connection.
    watch_task: Mutex<Option<JoinHandle<()>>>,
    /// Sync status by `lane:unit#index`.
    syncs: Mutex<BTreeMap<String, Value>>,
    /// Destination hashes by sync key, reused while files keep size and mtime.
    hashes: Mutex<HashMap<String, HashCache>>,
    /// Version, linger and port range findings about the box, by code.
    warnings: Mutex<BTreeMap<&'static str, String>>,
    /// Place in connect order: a box with a lower serial connected earlier and
    /// keeps the hostnames a later box also claims.
    serial: u64,
    /// What the box said about itself in its latest handshake, kept while it
    /// reconnects.
    info: Mutex<BoxInfo>,
    /// The box's version label (`devlanes 0.5.0 (linger off)`) from its latest
    /// handshake, kept while it reconnects like `info`.
    label: Mutex<String>,
    /// The `proxy_port_in_use` message while the proxy cannot serve the box's
    /// HTTPS port.
    proxy_port: Mutex<Option<String>>,
    /// Set once [`watch_link`] wrote a connection of this link to
    /// `remote_hosts`, which makes the box wanted.
    recorded: watch::Sender<bool>,
    /// The assignment version the mirror last reconciled with.
    applied: watch::Sender<u64>,
    /// When the live session started (Unix ms); `None` while not connected.
    connected_since: Mutex<Option<u64>>,
    /// Woken when the registry, the mirror view or a sync changed;
    /// [`announce_changes`] turns bursts of wakes into one [`wire::MIRROR_EVENT`].
    changed: tokio::sync::Notify,
    /// [`announce_changes`], waited for on close.
    announce_task: Mutex<Option<JoinHandle<()>>>,
    /// A code from `remote connect --code`, for the next time the box asks
    /// for one; cleared once a session is up and when that `connect` returns,
    /// and never sent past a code's lifetime, so a stale code never goes out
    /// (a wrong code voids the box's other codes).
    code: pairing::GivenCode,
}

/// Shortest time between two [`wire::MIRROR_EVENT`]s for one box: a start on
/// the box changes many units, and each reconcile would announce it again.
const MIRROR_EVENT_EVERY: Duration = Duration::from_millis(250);

impl HostConn {
    /// The live session, if connected.
    fn session(&self) -> Option<BoxSession> {
        live(&self.state.borrow())
    }

    /// Waits up to `limit` for a live session; `None` when the link gave up or
    /// waits for a sign-in or pairing.
    async fn session_within(&self, limit: Duration) -> Option<BoxSession> {
        let mut state = self.state.clone();
        let wait = async {
            loop {
                let (session, over) = {
                    let current = state.borrow_and_update();
                    (
                        live(&current),
                        matches!(
                            *current,
                            LinkState::Failed { .. }
                                | LinkState::Stopped
                                | LinkState::NeedsAuth { .. }
                                | LinkState::NeedsPairing { .. }
                        ),
                    )
                };
                if session.is_some() || over {
                    return session;
                }
                if state.changed().await.is_err() {
                    return None;
                }
            }
        };
        tokio::time::timeout(limit, wait).await.ok().flatten()
    }

    /// Why the link is down, while it retries, waits for a sign-in or
    /// pairing, or after it failed.
    fn link_error(&self) -> Option<String> {
        match &*self.state.borrow() {
            LinkState::Retrying { error, .. }
            | LinkState::NeedsAuth { error }
            | LinkState::NeedsPairing { error, .. }
            | LinkState::Failed { error } => Some(error.clone()),
            _ => None,
        }
    }

    /// True once the link stopped for good, so `connect` must start a new one.
    fn ended(&self) -> bool {
        matches!(
            *self.state.borrow(),
            LinkState::Failed { .. } | LinkState::Stopped
        )
    }

    /// True while the link waits for a person to sign in again.
    fn needs_auth(&self) -> bool {
        matches!(*self.state.borrow(), LinkState::NeedsAuth { .. })
    }

    /// True while the link waits for a person to pair this machine with the
    /// box again.
    fn needs_pairing(&self) -> bool {
        matches!(*self.state.borrow(), LinkState::NeedsPairing { .. })
    }

    /// Makes a link waiting for a sign-in or pairing try again, now that a
    /// CLI can answer its prompts or gave a code. Does nothing otherwise; true
    /// when it resumed the link.
    async fn resume(&self) -> bool {
        let resumed = self.link.lock().await.as_ref().is_some_and(Link::resume);
        if resumed {
            tracing::info!(host = %self.name, "retrying a box that needs a sign-in or pairing");
        }
        resumed
    }

    /// Waits, at most `limit`, for the attempt in flight when a `connect` CLI
    /// attached, and resumes the link once if that attempt ended needing a
    /// sign-in, or pairing with nobody having given a code: it may have asked
    /// its prompt before the CLI was there, as an attempt of a restore that a
    /// `remote connect` right after a daemon restart catches. The CLI stays
    /// registered meanwhile, so the next attempt asks it. A refused code is
    /// not tried again: the box voided its codes, and `connect` reports why.
    async fn resume_after_attempt(&self, limit: Duration) {
        let mut state = self.state.clone();
        let attempt = async {
            state
                .wait_for(|state| !matches!(state, LinkState::Connecting { .. }))
                .await
                .is_ok_and(|state| {
                    matches!(
                        *state,
                        LinkState::NeedsAuth { .. }
                            | LinkState::NeedsPairing { refused: false, .. }
                    )
                })
        };
        if matches!(tokio::time::timeout(limit, attempt).await, Ok(true)) {
            self.resume().await;
        }
    }

    /// The error `connect` answers with for a link that failed for good or
    /// waits for a sign-in or pairing; `None` otherwise.
    fn stuck(&self) -> Option<RpcError> {
        match &*self.state.borrow() {
            LinkState::Failed { error } => Some(remote_failed(&self.name, error)),
            LinkState::NeedsAuth { error } => Some(remote_needs_auth(&self.name, error)),
            LinkState::NeedsPairing {
                error, retry_at, ..
            } => Some(remote_needs_pairing(&self.name, error, *retry_at)),
            _ => None,
        }
    }

    /// Resolves when the link stops or needs a sign-in or pairing again:
    /// `Err` carries a permanent failure or what the person has to do.
    async fn finished(&self) -> Result<(), RpcError> {
        let mut state = self.state.clone();
        loop {
            if matches!(*state.borrow_and_update(), LinkState::Stopped) {
                return Ok(());
            }
            if let Some(error) = self.stuck() {
                return Err(error);
            }
            if state.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    /// Waits until connected, stopped or waiting for a sign-in or pairing, at
    /// most `limit`.
    async fn settled(&self, limit: Duration) {
        let mut state = self.state.clone();
        let wait = async {
            loop {
                let done = matches!(
                    *state.borrow_and_update(),
                    LinkState::Connected(_)
                        | LinkState::Failed { .. }
                        | LinkState::NeedsAuth { .. }
                        | LinkState::NeedsPairing { .. }
                        | LinkState::Stopped
                );
                if done || state.changed().await.is_err() {
                    return;
                }
            }
        };
        let _ = tokio::time::timeout(limit, wait).await;
    }

    /// Waits, at most [`RECORD_WAIT`], until a connected link's connection is
    /// in `remote_hosts`, so a daemon restarted right after `connect` reports
    /// it connected still restores the box. Returns at once when not connected.
    async fn record_written(&self) {
        if self.session().is_none() {
            return;
        }
        let mut recorded = self.recorded.subscribe();
        let _ = tokio::time::timeout(RECORD_WAIT, recorded.wait_for(|done| *done)).await;
    }

    /// `remote.state` payload for the current state, with the command that
    /// resumes a link waiting for a sign-in or pairing as `fix`.
    fn state_json(&self) -> Value {
        let mut value = describe(&self.state.borrow());
        if let Some(map) = value.as_object_mut() {
            map.insert("host".into(), json!(self.name));
            match map.get("state").and_then(Value::as_str) {
                Some("needs_auth") => {
                    map.insert("fix".into(), json!(sign_in_command(&self.name)));
                }
                Some("needs_pairing") => {
                    map.insert("fix".into(), json!(pair_command(&self.name)));
                }
                _ => {}
            }
        }
        value
    }

    /// State plus what is mirrored, for `connect`, `remote ls` and `status`.
    fn status_json(&self) -> Value {
        let mut value = self.state_json();
        let lanes = self.registry.current().lanes().count();
        let view = lock(&self.view).clone();
        let syncs: Vec<Value> = lock(&self.syncs).values().cloned().collect();
        let mut warnings: Vec<String> = lock(&self.warnings).values().cloned().collect();
        warnings.extend(lock(&self.proxy_port).clone());
        warnings.extend(view.warnings());
        let https_port = lock(&self.info).https_port;
        if let Some(map) = value.as_object_mut() {
            map.insert("transport".into(), json!(self.kind));
            map.insert("https_port".into(), json!(https_port));
            map.insert("lanes".into(), json!(lanes));
            map.insert("ports".into(), json!(view.ports_json()));
            map.insert(
                "routes".into(),
                json!(
                    view.routes
                        .iter()
                        .map(|route| json!({"host": route.host, "port": route.port, "lane": route.lane, "unit": route.unit}))
                        .collect::<Vec<_>>()
                ),
            );
            map.insert("skipped_lanes".into(), json!(view.skipped));
            map.insert("skipped_hosts".into(), json!(view.refused_hosts));
            map.insert("syncs".into(), json!(syncs));
            map.insert("warnings".into(), json!(warnings));
        }
        value
    }

    /// Stops reconnecting, closes the session and waits until this host's
    /// listeners and routes are gone.
    async fn close(&self) {
        self.cancel.cancel();
        let link = self.link.lock().await.take();
        if let Some(link) = link {
            link.shutdown().await;
        }
        let mirror = lock(&self.mirror_task).take();
        if let Some(mirror) = mirror {
            let _ = mirror.await;
        }
        let watcher = lock(&self.watch_task).take();
        if let Some(watcher) = watcher {
            let _ = watcher.await;
        }
        let announcer = lock(&self.announce_task).take();
        if let Some(announcer) = announcer {
            let _ = announcer.await;
        }
    }

    /// Asks for a [`wire::MIRROR_EVENT`]: what `host.list` shows of the box
    /// changed.
    fn mark_changed(&self) {
        self.changed.notify_one();
    }

    /// The `host.list` entry: [`HostConn::status_json`] plus what a client
    /// needs to show the box without asking anything else. Routes get the
    /// `url` to open here (`https_port` is this machine's proxy port) and their
    /// `state`, and while the link has not ended the entry carries the box's
    /// `registry` (lanes and units with states, ports and hostnames), its
    /// handshake info under `box`, `connected_since_ms`, the name `suffixes`
    /// box hostnames must end in and the extra HTTPS port opened for the box
    /// (`proxy.box_port`, `open` when it is served).
    fn list_json(
        &self,
        https_port: Option<u16>,
        suffixes: &[String],
        box_port_open: bool,
    ) -> Value {
        let mut value = self.status_json();
        let routes: Vec<Value> = lock(&self.view)
            .routes
            .iter()
            .map(|route| {
                // Wildcards have no single address to open.
                let url = (!route.host.contains('*'))
                    .then(|| devlanes_resolve::https_url(&route.host, https_port));
                json!({
                    "host": route.host,
                    "port": route.port,
                    "lane": route.lane,
                    "unit": route.unit,
                    "url": url,
                    "state": crate::routes::route_state(&route.status),
                })
            })
            .collect();
        let info = lock(&self.info).clone();
        let label = lock(&self.label).clone();
        let version = label_version(&label).map(str::to_owned);
        let box_port = info.https_port.filter(|port| Some(*port) != https_port);
        let Some(map) = value.as_object_mut() else {
            return value;
        };
        map.insert("routes".into(), json!(routes));
        map.insert(
            "connected_since_ms".into(),
            json!(*lock(&self.connected_since)),
        );
        if self.ended() {
            return value;
        }
        map.insert("registry".into(), json!(self.registry.current().snapshot()));
        map.insert(
            "box".into(),
            json!({
                "version": version,
                "label": (!label.is_empty()).then_some(label),
                "https_port": info.https_port,
                "ports_range": info.ports_range,
            }),
        );
        map.insert("suffixes".into(), json!(suffixes));
        map.insert(
            "proxy".into(),
            json!({
                "https_port": https_port,
                "box_port": box_port,
                "open": box_port.is_some() && box_port_open,
            }),
        );
        value
    }
}

/// The session of a connected state that has not ended yet.
fn live(state: &LinkState) -> Option<BoxSession> {
    match state {
        LinkState::Connected(session) if session.end().is_none() => Some(session.clone()),
        _ => None,
    }
}

/// Machine-readable link state.
fn describe(state: &LinkState) -> Value {
    match state {
        LinkState::Connecting { attempt } => json!({"state": "connecting", "attempt": attempt}),
        LinkState::Connected(session) => json!({"state": "connected", "server": session.server()}),
        LinkState::Retrying { delay, error } => json!({
            "state": "reconnecting",
            "retry_in_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "error": error,
        }),
        LinkState::NeedsAuth { error } => json!({"state": "needs_auth", "error": error}),
        LinkState::NeedsPairing {
            error, retry_at, ..
        } => json!({
            "state": "needs_pairing",
            "error": error,
            "pairing_retry_at_ms": retry_at.map(unix_ms),
        }),
        LinkState::Failed { error } => json!({"state": "failed", "error": error}),
        LinkState::Stopped => json!({"state": "stopped"}),
    }
}

/// The command that signs in to `host` again and resumes its link.
fn sign_in_command(host: &str) -> String {
    format!("devlanes remote connect {host}")
}

/// The command that pairs with `host` again, with the code `devlanes daemon
/// pair` printed on the box.
fn pair_command(host: &str) -> String {
    format!("devlanes daemon pair on {host}, then devlanes remote connect {host} --code <code>")
}

/// `time` in milliseconds since the Unix epoch.
fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// `host_not_connected`: `disconnect` named a box without a `connect`.
fn not_connected(host: &str) -> RpcError {
    RpcError::new("host_not_connected", json!({"host": host}), 4)
}

/// `port_mirrored`: a local service needs a port that the mirror of box `host`
/// holds for its unit `unit` (`project@lane:unit`).
fn port_mirrored(host: &str, port: u16, unit: &str) -> RpcError {
    RpcError::new(
        "port_mirrored",
        json!({
            "port": port,
            "host": host,
            "unit": unit,
            "message": format!(
                "port {port} is mirrored from box {host} for {unit}; stop {unit} on the box or run `devlanes remote disconnect {host}`"
            ),
        }),
        1,
    )
}

/// `remote_failed`: the link failed.
fn remote_failed(host: &str, message: impl std::fmt::Display) -> RpcError {
    RpcError::new(
        "remote_failed",
        json!({"host": host, "message": message.to_string()}),
        4,
    )
}

/// The transport name `remote_hosts` records for a box reached by a command.
const COMMAND: &str = "command";

/// `box_command_missing`: a restored box was reached by a command that the
/// user config no longer lists, so the restore does not try ssh instead.
fn command_missing(host: &str) -> RpcError {
    RpcError::new(
        "box_command_missing",
        json!({
            "host": host,
            "message": format!(
                "the user config no longer has remote.boxes.{host}.command for box {host}. Add it \
                 back and run `devlanes remote connect {host}`, or run `devlanes remote disconnect \
                 {host}`"
            ),
        }),
        1,
    )
}

/// One line for a failure nobody waits for, such as a restore's: its message
/// and, for a `config_error`, the file and line it is about.
fn error_text(error: &RpcError) -> String {
    let data = &error.data;
    let message = data
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| format!("{}: {data}", error.code), str::to_owned);
    match (
        data.get("file").and_then(Value::as_str),
        data.get("line")
            .and_then(Value::as_u64)
            .filter(|line| *line > 0),
    ) {
        (Some(file), Some(line)) => format!("{file} is invalid at line {line}: {message}"),
        (Some(file), None) if error.code == "config_error" => {
            format!("{file} is invalid: {message}")
        }
        _ => message,
    }
}

/// `remote_needs_auth`: the link waits for a person to sign in again, with
/// the command that does it as `fix`.
fn remote_needs_auth(host: &str, message: impl std::fmt::Display) -> RpcError {
    RpcError::new(
        "remote_needs_auth",
        json!({"host": host, "message": message.to_string(), "fix": sign_in_command(host)}),
        4,
    )
}

/// `remote_needs_pairing`: the box serves only paired clients and took no
/// token or code from this machine, with the commands that pair it as `fix`
/// and, after too many wrong codes, when the box takes codes again.
fn remote_needs_pairing(
    host: &str,
    message: impl std::fmt::Display,
    retry_at: Option<SystemTime>,
) -> RpcError {
    RpcError::new(
        "remote_needs_pairing",
        json!({
            "host": host,
            "message": message.to_string(),
            "fix": pair_command(host),
            "retry_at_ms": retry_at.map(unix_ms),
        }),
        4,
    )
}

/// `connect` parameters. Not `Debug`: `code` is a secret.
#[derive(Deserialize)]
struct ConnectParams {
    host: String,
    #[serde(default)]
    background: bool,
    /// A pairing code from `devlanes daemon pair` on the box.
    #[serde(default)]
    code: Option<String>,
}

/// `disconnect` parameters; no host means every box.
#[derive(Debug, Default, Deserialize)]
struct DisconnectParams {
    #[serde(default)]
    host: Option<String>,
}

/// Where the box lists a client: under its id, so a reconnect replaces the
/// entry its dropped connection left, or apart when it sent no id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ClientKey {
    Id(String),
    Anonymous(u64),
}

/// Drops the code a `connect` gave once that request returns or its CLI
/// detaches, unless a later `connect` gave another one meanwhile.
struct ForgetCode {
    slot: pairing::GivenCode,
    code: String,
}

impl Drop for ForgetCode {
    /// Clears the slot when it still holds this code.
    fn drop(&mut self) {
        let mut slot = lock(&self.slot);
        if slot.as_ref().is_some_and(|(code, _)| *code == self.code) {
            *slot = None;
        }
    }
}

/// Clears a flag when dropped, however the code holding it ends.
struct ClearOnDrop<'a>(&'a AtomicBool);

impl Drop for ClearOnDrop<'_> {
    /// Sets the flag to false.
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Removes a tunnel from the box's client list when its registry feed ends.
struct ClientGuard {
    engine: Arc<Inner>,
    key: ClientKey,
    serial: u64,
    /// Cancelled by `daemon revoke` to end the feed, and with it the session.
    revoked: CancellationToken,
}

impl Drop for ClientGuard {
    /// Forgets the tunnel, unless a newer connection of the same client
    /// replaced its entry.
    fn drop(&mut self) {
        lock(&self.engine.remote.feeds).remove(&self.serial);
        let removed = {
            let mut clients = lock(&self.engine.remote.clients);
            let current = clients
                .get(&self.key)
                .is_some_and(|(serial, _)| *serial == self.serial);
            current && clients.remove(&self.key).is_some()
        };
        if removed {
            self.engine.clients_changed();
        }
    }
}

/// Names ssh accepts as destinations; a leading `-` would read as an option.
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && !host.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// `$DEVLANES_HOME/sync/<host>/<project>/<lane>/<leaf>` for a box sync entry.
///
/// Every component is a single validated path name, so nothing the box sends can
/// point the destination elsewhere; the box's own `to` is never used.
fn sync_destination(home: &Path, host: &str, entry: &SyncEntry) -> Result<PathBuf, String> {
    let host = host.replace('/', "_");
    // Windows stores the host, which is what the user typed (`ssh://box:2222`
    // suits ssh), under its encoded file name; Unix keeps it as it is. The
    // encoding would turn `..` into a plain name, so that is refused first.
    #[cfg(windows)]
    let host = match host.as_str() {
        "." | ".." => return Err(format!("the box sent an unusable host name {host:?}")),
        _ => devlanes_core::paths::name_on_disk(&host).into_owned(),
    };
    let mut path = home.join("sync");
    for (what, part) in [
        ("host", host.as_str()),
        ("project", entry.project.as_str()),
        ("lane", entry.lane.as_str()),
        ("folder", entry.leaf.as_str()),
    ] {
        let valid = !part.contains('/') && RelPath::new(part).is_ok();
        if !valid {
            return Err(format!("the box sent an unusable {what} name {part:?}"));
        }
        path.push(part);
    }
    Ok(path)
}

impl Inner {
    /// `connect`: creates (or reuses) the box's link, then streams `remote.state`
    /// events until the CLI detaches. The connection lives in the daemon either way;
    /// `background` only returns once the first attempt settled and, when it
    /// connected, the connection is recorded. A `code` is kept for the next
    /// time the box asks for one, and dropped when the link is up already and
    /// when this request returns or its CLI detaches.
    pub(crate) async fn connect_method(
        self: &Arc<Self>,
        value: Value,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: ConnectParams = required(value)?;
        if !valid_host(&args.host) {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": "invalid host", "host": args.host}),
                2,
            ));
        }
        let code = args.code.filter(|code| !code.trim().is_empty());
        let (conn, _watching) = self
            .ensure_host(&args.host, Some(session), false, code.clone())
            .await?;
        let _forget = code.map(|code| ForgetCode {
            slot: conn.code.clone(),
            code,
        });
        // Connecting to a box again retries a box HTTPS port that was taken.
        self.reconcile_box_ports().await;
        // This CLI can answer prompts now, so a link waiting for one goes on,
        // and one still trying may have asked nobody before the CLI attached.
        let in_flight =
            !conn.resume().await && matches!(*conn.state.borrow(), LinkState::Connecting { .. });
        if conn.session().is_some() {
            lock(&conn.code).take();
        }
        if args.background {
            if in_flight {
                conn.resume_after_attempt(CONNECT_WAIT).await;
            }
            conn.settled(CONNECT_WAIT).await;
            if let Some(error) = conn.stuck() {
                return Err(error);
            }
            conn.record_written().await;
            return Ok(conn.status_json());
        }
        // Later `connected` events come from `watch_link` after the record.
        conn.record_written().await;
        let first = Event {
            seq: 0,
            event: wire::STATE_EVENT.into(),
            data: conn.state_json(),
        };
        if session.event(first).await.is_err() {
            return Ok(conn.status_json());
        }
        let host = conn.name.clone();
        let mine = move |event: &Event| {
            (event.event == wire::STATE_EVENT || event.event == wire::WARNING_EVENT)
                && event.data.get("host").and_then(Value::as_str) == Some(host.as_str())
        };
        let finished = async {
            if in_flight {
                conn.resume_after_attempt(CONNECT_WAIT).await;
            }
            conn.finished().await
        };
        tokio::select! {
            () = self.bus.forward(session, mine) => Ok(conn.status_json()),
            finished = finished => finished.map(|()| conn.status_json()),
        }
    }

    /// The live connection to `host`, or a new one. A stopped link is closed, with
    /// its listeners, before its replacement starts. `watcher`, the CLI of a
    /// `connect`, gets the link's prompts from before its first attempt until
    /// the returned guard drops. `connected_before` marks a box restored after
    /// a daemon restart (see [`RemoteConnector::transport`]). `code` is a
    /// pairing code for the next time the box asks for one.
    ///
    /// [`RemoteConnector::transport`]: crate::integrations::RemoteConnector::transport
    async fn ensure_host(
        self: &Arc<Self>,
        host: &str,
        watcher: Option<&Session>,
        connected_before: bool,
        code: Option<String>,
    ) -> Result<(Arc<HostConn>, Option<WatchGuard>), RpcError> {
        let identity = self.client_identity().await?;
        self.load_choices().await;
        let _connecting = self.remote.connecting.lock().await;
        self.replace_host(host, identity, watcher, connected_before, None, code)
            .await
    }

    /// [`Inner::ensure_host`]'s work, with [`Remotes::connecting`] held.
    /// `recorded` is the transport `remote_hosts` holds for a restored box
    /// (see [`Inner::open_host`]).
    async fn replace_host(
        self: &Arc<Self>,
        host: &str,
        identity: identity::ClientIdentity,
        watcher: Option<&Session>,
        connected_before: bool,
        recorded: Option<&str>,
        code: Option<String>,
    ) -> Result<(Arc<HostConn>, Option<WatchGuard>), RpcError> {
        let old = {
            let mut hosts = lock(&self.remote.hosts);
            match hosts.get(host) {
                Some(conn) if !conn.ended() => {
                    let guard = watcher.map(|session| conn.board.watch(session));
                    if let Some(code) = code {
                        *lock(&conn.code) = Some((code, std::time::Instant::now()));
                    }
                    return Ok((conn.clone(), guard));
                }
                _ => hosts.remove(host),
            }
        };
        if let Some(old) = old {
            old.close().await;
            self.remote.wake_others(old.serial);
            self.refresh_range_overlaps();
        }
        let (conn, guard) = self
            .open_host(host, identity, watcher, connected_before, recorded, code)
            .await?;
        lock(&self.remote.hosts).insert(host.to_owned(), conn.clone());
        lock(&self.remote.restore_errors).remove(host);
        Ok((conn, guard))
    }

    /// The id and name this machine sends to boxes, created on first use.
    /// Failing names the file: `client_identity` with `{message, path}`.
    async fn client_identity(&self) -> Result<identity::ClientIdentity, RpcError> {
        self.remote
            .identity
            .get_or_try_init(|| async {
                let home = self.opts.home.clone();
                let failed = |error: &dyn std::fmt::Display| {
                    RpcError::new(
                        "client_identity",
                        json!({
                            "message": error.to_string(),
                            "path": self.opts.home.join(identity::ID_FILE),
                        }),
                        1,
                    )
                };
                tokio::task::spawn_blocking(move || identity::load(&home))
                    .await
                    .map_err(|error| failed(&error))?
                    .map_err(|error| failed(&error))
            })
            .await
            .cloned()
    }

    /// Starts the link and the host's watcher and mirror tasks, with
    /// `watcher` registered for prompts and `code` kept for pairing before the
    /// link's first attempt. A box `recorded` as reached by a command fails
    /// with `box_command_missing` when the connector no longer has a command
    /// for it, instead of trying the name as an ssh destination.
    async fn open_host(
        self: &Arc<Self>,
        host: &str,
        identity: identity::ClientIdentity,
        watcher: Option<&Session>,
        connected_before: bool,
        recorded: Option<&str>,
        code: Option<String>,
    ) -> Result<(Arc<HostConn>, Option<WatchGuard>), RpcError> {
        let board = Arc::new(PromptBoard::default());
        let prompter = Arc::new(HostPrompter {
            board: board.clone(),
        });
        let remote = self
            .integ
            .remote
            .transport(host, prompter.clone(), connected_before)
            .await?;
        if recorded == Some(COMMAND) && remote.kind != COMMAND {
            return Err(command_missing(host));
        }
        let transport = Arc::new(Attended {
            inner: remote.transport,
            board: board.clone(),
        });
        let guard = watcher.map(|session| board.watch(session));
        let code: pairing::GivenCode = Arc::new(Mutex::new(
            code.map(|code| (code, std::time::Instant::now())),
        ));
        let mut options = SessionOptions::new(format!("devlanes {}", devlanes_version::VERSION));
        options.client_id = identity.id;
        options.client_name = identity.name;
        options.pairing = Some(Arc::new(pairing::HostPairing {
            host: host.to_owned(),
            store: self.store.clone(),
            prompter,
            code: code.clone(),
        }));
        let link = Link::spawn(transport, options, self.integ.remote.backoff());
        let conn = Arc::new(HostConn {
            name: host.to_owned(),
            kind: remote.kind,
            state: link.state(),
            registry: link.registry().clone(),
            link: tokio::sync::Mutex::new(Some(link)),
            cancel: CancellationToken::new(),
            board,
            view: Mutex::new(mirror::View::default()),
            wake: tokio::sync::Notify::new(),
            mirror_task: Mutex::new(None),
            watch_task: Mutex::new(None),
            syncs: Mutex::new(BTreeMap::new()),
            hashes: Mutex::new(HashMap::new()),
            warnings: Mutex::new(BTreeMap::new()),
            serial: self.remote.next_serial.fetch_add(1, Ordering::Relaxed),
            info: Mutex::new(BoxInfo::default()),
            label: Mutex::new(String::new()),
            proxy_port: Mutex::new(None),
            recorded: watch::Sender::new(false),
            applied: watch::Sender::new(0),
            connected_since: Mutex::new(None),
            changed: tokio::sync::Notify::new(),
            announce_task: Mutex::new(None),
            code,
        });
        let watcher = tokio::spawn(watch_link(Arc::downgrade(self), conn.clone()));
        *lock(&conn.watch_task) = Some(watcher);
        let mirror = tokio::spawn(mirror::run(Arc::downgrade(self), conn.clone()));
        *lock(&conn.mirror_task) = Some(mirror);
        let announcer = tokio::spawn(announce_changes(Arc::downgrade(self), conn.clone()));
        *lock(&conn.announce_task) = Some(announcer);
        Ok((conn, guard))
    }

    /// `disconnect [host]`: closes one box or all of them, unmirrors them and
    /// clears `remote_hosts.wanted`, so a restarted daemon leaves them alone.
    /// A wanted box without a connection here, such as one whose restore
    /// failed or has not reached it yet, is only forgotten.
    pub(crate) async fn disconnect_method(
        self: &Arc<Self>,
        value: Value,
    ) -> Result<Value, RpcError> {
        let args: DisconnectParams = crate::engine::params(value)?;
        // Held until `wanted` is cleared: a restore checks it under this lock
        // before opening a box, so it either sees the box unwanted or has
        // opened it already, and then the box is closed here.
        let connecting = self.remote.connecting.lock().await;
        let wanted = match self.wanted_hosts().await {
            Ok(wanted) => wanted,
            Err(error) => {
                tracing::warn!(%error, "cannot read the boxes to restore");
                BTreeSet::new()
            }
        };
        let closing: Vec<Arc<HostConn>> = {
            let mut hosts = lock(&self.remote.hosts);
            match &args.host {
                Some(host) => match hosts.remove(host) {
                    Some(conn) => vec![conn],
                    None if wanted.contains(host) => Vec::new(),
                    None => return Err(not_connected(host)),
                },
                None => std::mem::take(&mut *hosts).into_values().collect(),
            }
        };
        let mut names = BTreeSet::new();
        for conn in closing {
            conn.close().await;
            self.bus.emit(
                wire::STATE_EVENT,
                json!({"host": conn.name, "state": "stopped"}),
            );
            names.insert(conn.name.clone());
            self.remote.wake_others(conn.serial);
        }
        match &args.host {
            Some(host) => {
                names.insert(host.clone());
            }
            None => names.extend(wanted.iter().cloned()),
        }
        // After `close`, which waited for the last record of a connection.
        for name in &names {
            lock(&self.remote.restore_errors).remove(name);
            if let Err(error) = self.store.set_remote_host_wanted(name.clone(), false).await {
                tracing::warn!(%error, host = %name, "a restarted daemon may connect this box again");
            }
            self.forget_choices_of(name).await;
        }
        drop(connecting);
        let names: Vec<String> = names.into_iter().collect();
        self.refresh_range_overlaps();
        self.reconcile_box_ports().await;
        self.reassign();
        self.publish_routes();
        Ok(json!({"disconnected": names}))
    }

    /// `remote ls`: boxes known from `remote_hosts` plus live connection state,
    /// with each live box's registry and mirror view ([`HostConn::list_json`]).
    /// A wanted box the restore could not open is `failed` with its error.
    /// Every box lists its `chosen_lanes`, and `yielded_local_lanes` names this
    /// machine's stopped lanes whose hostnames a chosen box lane serves.
    pub(crate) async fn host_list(self: &Arc<Self>) -> Result<Value, RpcError> {
        self.load_choices().await;
        let https_port = self.integ.routes.https_port();
        let suffixes = mirror::route_suffixes(self);
        let open_ports = self.remote.box_ports.lock().await.open.clone();
        let stored = self.store.remote_hosts().await.unwrap_or_default();
        let restore_errors = lock(&self.remote.restore_errors).clone();
        let choices = lock(&self.remote.choices).clone();
        let chosen = |host: &str| -> Vec<&String> {
            choices
                .iter()
                .filter(|(_, chosen)| *chosen == host)
                .map(|(lane, _)| lane)
                .collect()
        };
        let mut items: BTreeMap<String, Value> = BTreeMap::new();
        for host in stored {
            let mut item = json!({
                "host": host.name,
                "transport": host.transport,
                "address": host.address,
                "daemon_version": host.daemon_version,
                "last_connected_ms": host.last_connected,
                "state": "disconnected",
            });
            if let Some(error) = restore_errors.get(&host.name) {
                item["state"] = json!("failed");
                item["error"] = json!(error);
            }
            items.insert(host.name.clone(), item);
        }
        for conn in self.remote.all() {
            let box_port_open = lock(&conn.info)
                .https_port
                .is_some_and(|port| open_ports.contains(&port));
            let mut status = conn.list_json(https_port, &suffixes, box_port_open);
            if let (Some(stored), Some(map)) = (items.get(&conn.name), status.as_object_mut()) {
                for key in ["address", "daemon_version", "last_connected_ms"] {
                    if let Some(value) = stored.get(key) {
                        map.insert(key.into(), value.clone());
                    }
                }
            }
            items.insert(conn.name.clone(), status);
        }
        for (name, item) in &mut items {
            item["chosen_lanes"] = json!(chosen(name));
        }
        let yielded: Vec<Value> = self
            .remote
            .assignment()
            .yielded
            .iter()
            .map(|(lane, winner)| json!({"lane": lane, "winner": winner}))
            .collect();
        Ok(json!({
            "items": items.into_values().collect::<Vec<_>>(),
            "yielded_local_lanes": yielded,
            "truncated": false,
            "next_cursor": null,
        }))
    }

    /// Routes binary frames that answer ssh login prompts; false for other streams.
    pub(crate) async fn remote_input(
        &self,
        stream_id: u64,
        data: &Bytes,
        session: &Session,
    ) -> bool {
        self.remote
            .all()
            .iter()
            .any(|conn| conn.board.answer(session.id(), stream_id, data))
    }

    /// `status`, plus the tunnels serving clients (on a box) and the boxes this
    /// machine is connected to.
    pub(crate) async fn remote_status(
        self: &Arc<Self>,
        value: Value,
        context: &devlanes_proto::Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let mut result = self.status_method(value, context, session).await?;
        let clients: Vec<RemoteClient> = lock(&self.remote.clients)
            .values()
            .map(|(_, client)| client.clone())
            .collect();
        let hosts: Vec<Value> = self
            .remote
            .all()
            .iter()
            .map(|conn| conn.state_json())
            .collect();
        if let Some(map) = result.as_object_mut() {
            if !clients.is_empty() {
                map.insert("clients".into(), json!(clients));
            }
            if !hosts.is_empty() {
                map.insert("hosts".into(), json!(hosts));
            }
        }
        Ok(result)
    }

    /// `sync.status`: sync entries of lanes with running services (`kind: local`
    /// with the local copy's state under `local`), plus the state of syncs this
    /// machine receives from boxes (`kind: remote`). With `for_clients`, what a
    /// tunnel may stream: a hidden service's entries are left out, so a client
    /// can neither list nor follow them.
    pub(crate) async fn sync_status_method(
        self: &Arc<Self>,
        value: Value,
    ) -> Result<Value, RpcError> {
        let args: devlanes_proto::remote::SyncStatusParams = crate::engine::params(value)?;
        let mut items = Vec::new();
        for (project, lane) in self.all_lanes() {
            let (alive, reference) = {
                let st = lane.st();
                (st.services_alive(), st.reference(&project.name()))
            };
            if !alive {
                continue;
            }
            let resolved = match self.resolve_full(&project, &lane).await {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::debug!(lane = %reference, code = %error.code, "sync entries unavailable");
                    continue;
                }
            };
            for service in resolved
                .services
                .values()
                .filter(|service| !(args.for_clients && service.hidden))
            {
                for (index, spec) in service.sync.iter().enumerate() {
                    let entry = SyncEntry {
                        lane: reference.clone(),
                        project: project.name(),
                        unit: service.unit.clone(),
                        index,
                        from: spec.from.clone(),
                        leaf: spec
                            .from
                            .file_name()
                            .map(|leaf| leaf.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "sync".into()),
                        ignore: spec.ignore.clone(),
                    };
                    // Tunnels read the entry fields; `kind` and `local` are extra.
                    let mut item = serde_json::to_value(entry).unwrap_or_default();
                    item["kind"] = json!("local");
                    item["local"] =
                        crate::local_sync::status(&lane, &service.unit, index).unwrap_or_default();
                    items.push(item);
                }
            }
        }
        let remote: Vec<Value> = self
            .remote
            .all()
            .iter()
            .flat_map(|conn| lock(&conn.syncs).values().cloned().collect::<Vec<_>>())
            .map(|mut status| {
                if let Some(fields) = status.as_object_mut() {
                    fields.insert("kind".into(), json!("remote"));
                }
                status
            })
            .collect();
        Ok(json!({
            "items": items,
            "remote": remote,
            "truncated": false,
            "next_cursor": null,
        }))
    }

    /// Makes sure no box mirror holds one of `ports` before a local service of
    /// `lane` binds them. A starting lane counts as running, so it takes its
    /// hostnames back from every box lane it clashes with, deleting a choice
    /// of one ([`Inner::reassign`]). A mirror gives up ports inside a local
    /// block and those of lanes it no longer mirrors, so this wakes the
    /// mirrors and waits up to [`RELEASE_WAIT`]; a port still held then, such
    /// as a literal `PORT` the box also uses, fails with `port_mirrored`.
    pub(crate) async fn release_for_local(
        &self,
        lane: &str,
        ports: &BTreeSet<u16>,
    ) -> Result<(), RpcError> {
        let _starting = self.remote.starting(lane);
        self.load_choices().await;
        self.reassign();
        let deadline = tokio::time::Instant::now() + RELEASE_WAIT;
        loop {
            let held = self.remote.all().into_iter().find_map(|conn| {
                let found = lock(&conn.view)
                    .bound
                    .iter()
                    .find(|(port, _)| ports.contains(*port))
                    .map(|(port, unit)| (*port, unit.clone()));
                found.map(|(port, unit)| (conn, port, unit))
            });
            let Some((conn, port, unit)) = held else {
                return Ok(());
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(port_mirrored(&conn.name, port, &unit));
            }
            conn.wake.notify_one();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Connects again, in the background, every box an earlier daemon had
    /// connected and that was not disconnected since, with no CLI attached: a
    /// prompt then leaves the box waiting for `remote connect`. Until every
    /// such box is open, the daemon does not count as idle.
    pub(crate) fn restore_remotes(self: &Arc<Self>) {
        self.remote.restoring.store(true, Ordering::Relaxed);
        let engine = self.clone();
        let task = tokio::spawn(async move {
            // Also on a panic or an abort, which would otherwise keep the
            // daemon from ever exiting on idle.
            let _restored = ClearOnDrop(&engine.remote.restoring);
            engine.restore_wanted().await;
        });
        *lock(&self.remote.restore) = Some(task);
    }

    /// [`Inner::restore_remotes`]'s work. Stops opening boxes once a shutdown
    /// began, which waits for this before it closes the boxes opened so far.
    async fn restore_wanted(self: &Arc<Self>) {
        // Before any box connects, so the first assignment has the choices.
        self.load_choices().await;
        // Name order, so the restore order does not depend on the store.
        let wanted: BTreeMap<String, String> = match self.store.remote_hosts().await {
            Ok(hosts) => hosts
                .into_iter()
                .filter(|host| host.wanted)
                .map(|host| (host.name, host.transport))
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot read the boxes to restore");
                return;
            }
        };
        let mut restored = false;
        for (name, transport) in wanted {
            if self.shutdown.is_cancelled() {
                return;
            }
            let identity = match self.client_identity().await {
                Ok(identity) => identity,
                Err(error) => {
                    tracing::warn!(code = %error.code, "cannot restore box connections");
                    return;
                }
            };
            let _connecting = self.remote.connecting.lock().await;
            // A `remote disconnect` since the list was read wins.
            match self.wanted_hosts().await {
                Ok(wanted) if wanted.contains(&name) => {}
                Ok(_) => continue,
                Err(error) => {
                    tracing::warn!(%error, host = %name, "cannot read the boxes to restore");
                    continue;
                }
            }
            if self.shutdown.is_cancelled() {
                return;
            }
            match self
                .replace_host(&name, identity, None, true, Some(&transport), None)
                .await
            {
                Ok(_) => {
                    tracing::info!(host = %name, "reconnecting a box connected before the restart");
                    restored = true;
                }
                Err(error) => {
                    let message = error_text(&error);
                    tracing::warn!(host = %name, code = %error.code, %message, "cannot restore the box connection");
                    lock(&self.remote.restore_errors).insert(name.clone(), message.clone());
                    self.bus.emit(
                        wire::STATE_EVENT,
                        json!({"host": name, "state": "failed", "error": message}),
                    );
                }
            }
        }
        if restored {
            self.reconcile_box_ports().await;
        }
    }

    /// Names of the boxes recorded as wanted in `remote_hosts`.
    async fn wanted_hosts(&self) -> Result<BTreeSet<String>, devlanes_state::Error> {
        Ok(self
            .store
            .remote_hosts()
            .await?
            .into_iter()
            .filter(|host| host.wanted)
            .map(|host| host.name)
            .collect())
    }

    /// Closes every box connection; called on daemon shutdown so ssh masters
    /// exit. Waits for the restore first, so it opens nothing afterwards and
    /// is done with the store before the store closes. Reported as the
    /// `remote` step of `steps`.
    pub(crate) async fn remote_shutdown(self: &Arc<Self>, steps: &crate::stopping::StopSteps<'_>) {
        let restore = lock(&self.remote.restore).take();
        if let Some(restore) = restore {
            let _ = restore.await;
        }
        let hosts: Vec<Arc<HostConn>> = std::mem::take(&mut *lock(&self.remote.hosts))
            .into_values()
            .collect();
        let run = steps.begin(devlanes_proto::StoppingStep::Remote, hosts.len(), None);
        for conn in hosts {
            conn.close().await;
        }
        self.reconcile_box_ports().await;
        if let Some(run) = run {
            steps.done(&run);
        }
    }

    /// Opens the extra HTTPS port of every box whose proxy port differs from
    /// this machine's, so the URLs the box generated open here unchanged, and
    /// closes the ports no box needs anymore. A box keeps its port while it
    /// reconnects, like its mirrored ports. A port that cannot be opened is
    /// retried on the next call and every [`BOX_PORT_RETRY`], and reported with
    /// `proxy_port_in_use` to each box needing it, again only when the reason
    /// changes. Looking up the holder can be slow, so it runs without the lock.
    pub(super) async fn reconcile_box_ports(self: &Arc<Self>) {
        let own = self.integ.routes.https_port();
        let mut wanted: BTreeMap<u16, Vec<Arc<HostConn>>> = BTreeMap::new();
        for conn in self.remote.all() {
            let port = lock(&conn.info).https_port;
            match (port, own) {
                (Some(port), Some(own)) if port != own && !conn.ended() => {
                    wanted.entry(port).or_default().push(conn);
                }
                _ => *lock(&conn.proxy_port) = None,
            }
        }
        let mut failures = Vec::new();
        {
            let mut ports = self.remote.box_ports.lock().await;
            ports.failed.retain(|port| wanted.contains_key(port));
            let stale: Vec<u16> = ports
                .open
                .iter()
                .copied()
                .filter(|port| !wanted.contains_key(port))
                .collect();
            for port in stale {
                ports.open.remove(&port);
                self.integ.routes.close_box_port(port).await;
            }
            for (port, conns) in &wanted {
                if !ports.open.contains(port) {
                    match self.integ.routes.open_box_port(*port) {
                        Ok(()) => {
                            ports.failed.remove(port);
                            ports.open.insert(*port);
                            // Their mirror reports say which port URLs open on.
                            for conn in conns {
                                conn.mark_changed();
                            }
                        }
                        Err(error) => {
                            ports.failed.insert(*port);
                            failures.push((*port, error, conns.clone()));
                            continue;
                        }
                    }
                }
                for conn in conns {
                    *lock(&conn.proxy_port) = None;
                }
            }
            if !ports.failed.is_empty() && ports.retry.is_none() {
                ports.retry = Some(tokio::spawn(retry_box_ports(Arc::downgrade(self))));
            }
        }
        let Some(own) = own else { return };
        for (port, error, conns) in failures {
            let problem = self.box_port_problem(port, &error).await;
            let ports = self.remote.box_ports.lock().await;
            // Opened by another reconcile while the holder was looked up.
            if !ports.failed.contains(&port) {
                continue;
            }
            for conn in &conns {
                let message = format!(
                    "this machine cannot use the box's HTTPS port {port} ({}). Open its hostnames on port {own}.",
                    problem.holder
                );
                let changed = {
                    let mut current = lock(&conn.proxy_port);
                    let changed = current.as_ref() != Some(&message);
                    *current = Some(message.clone());
                    changed
                };
                if changed {
                    conn.mark_changed();
                    tracing::warn!(host = %conn.name, port, %message, "cannot serve the box's https port");
                    self.bus.emit(
                        wire::WARNING_EVENT,
                        json!({
                            "host": conn.name,
                            "code": "proxy_port_in_use",
                            "port": port,
                            "own_port": own,
                            "holder": problem.holder,
                            "pid": problem.pid,
                            "message": message,
                        }),
                    );
                }
            }
            drop(ports);
        }
    }

    /// What keeps the proxy from serving a box's HTTPS `port`: the process holding
    /// it, or why the bind failed.
    async fn box_port_problem(&self, port: u16, error: &std::io::Error) -> BoxPortProblem {
        // The warning offers this machine's own port instead, which is the
        // way out whatever denied the bind.
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return BoxPortProblem {
                holder: "permission denied".to_owned(),
                pid: None,
            };
        }
        if error.kind() != std::io::ErrorKind::AddrInUse {
            return BoxPortProblem {
                holder: mirror::bind_failure(error),
                pid: None,
            };
        }
        let pid = self.integ.ports.holder(port).await;
        let holder = match pid {
            // In parentheses in the warning, so without its own.
            Some(pid) => format!("in use by {}, pid {pid}", mirror::process_name(pid).await),
            None => "in use by another process".to_owned(),
        };
        BoxPortProblem { holder, pid }
    }

    /// Updates the `ports_range_overlap` finding of every live box, after a box
    /// connected, reconnected or went away, and emits it where it appeared or
    /// changed. Only boxes are compared: local allocations already keep clear
    /// of box ports, while two boxes know nothing of each other's.
    fn refresh_range_overlaps(&self) {
        let (live, ended): (Vec<Arc<HostConn>>, Vec<Arc<HostConn>>) = self
            .remote
            .all()
            .into_iter()
            .partition(|conn| !conn.ended());
        for conn in ended {
            lock(&conn.warnings).remove(RANGE_OVERLAP);
        }
        for conn in &live {
            let finding = range_overlap(conn, &live);
            let changed = {
                let mut warnings = lock(&conn.warnings);
                match &finding {
                    Some((message, _)) => {
                        warnings.insert(RANGE_OVERLAP, message.clone()).as_ref() != Some(message)
                    }
                    None => {
                        warnings.remove(RANGE_OVERLAP);
                        false
                    }
                }
            };
            if let (true, Some((message, mut data))) = (changed, finding) {
                tracing::warn!(host = %conn.name, code = RANGE_OVERLAP, %message, "box warning");
                if let Some(fields) = data.as_object_mut() {
                    fields.insert("host".into(), json!(conn.name));
                    fields.insert("code".into(), json!(RANGE_OVERLAP));
                    fields.insert("message".into(), json!(message));
                }
                self.bus.emit(wire::WARNING_EVENT, data);
            }
        }
    }
}

/// Tries the failed box HTTPS ports again every [`BOX_PORT_RETRY`] until none
/// is left or the engine is gone. Boxed because `reconcile_box_ports` spawns it.
fn retry_box_ports(engine: std::sync::Weak<Inner>) -> futures_util::future::BoxFuture<'static, ()> {
    Box::pin(async move {
        loop {
            tokio::time::sleep(BOX_PORT_RETRY).await;
            let Some(inner) = engine.upgrade() else {
                return;
            };
            {
                let mut ports = inner.remote.box_ports.lock().await;
                if ports.failed.is_empty() {
                    // Cleared under the lock, so the next failure starts a new timer.
                    ports.retry = None;
                    return;
                }
            }
            inner.reconcile_box_ports().await;
        }
    })
}

/// The `ports_range_overlap` finding for `conn` among the `live` boxes, as
/// message and event fields: its box's `ports.range` overlaps another box's.
/// Lanes allocated on both can then get the same ports, and only one box's
/// can be mirrored.
fn range_overlap(conn: &HostConn, live: &[Arc<HostConn>]) -> Option<(String, Value)> {
    let range = lock(&conn.info).ports_range.clone()?;
    let theirs = parse_range(&range)?;
    let mut overlaps = Vec::new();
    for other in live {
        if other.serial == conn.serial {
            continue;
        }
        let Some(other_range) = lock(&other.info).ports_range.clone() else {
            continue;
        };
        if parse_range(&other_range).is_some_and(|bounds| ranges_overlap(bounds, theirs)) {
            overlaps.push(format!("box {} ({other_range})", other.name));
        }
    }
    if overlaps.is_empty() {
        return None;
    }
    let message = format!(
        "the box's ports.range {range} overlaps {}, so their lanes can get the same ports. Give one box a different ports.range",
        overlaps.join(" and ")
    );
    Some((message, json!({"range": range, "overlaps": overlaps})))
}

/// A `ports.range` value (`20000-32767`) as inclusive bounds, `None` when invalid.
fn parse_range(range: &str) -> Option<(u16, u16)> {
    devlanes_config::PortsConfig {
        range: range.trim().to_owned(),
    }
    .bounds()
    .ok()
}

/// Whether two inclusive port ranges share a port.
fn ranges_overlap(a: (u16, u16), b: (u16, u16)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// The release in a handshake label: `0.5.0` of `devlanes 0.5.0 (linger off)`.
fn label_version(label: &str) -> Option<&str> {
    label
        .strip_prefix("devlanes ")
        .and_then(|rest| rest.split_whitespace().next())
}

/// Findings about the box behind a handshake label (`devlanes <version>`,
/// optionally `(linger off)` or `(daemon stops with the session)`), as
/// warning code and message.
fn label_warnings(label: &str) -> Vec<(&'static str, String)> {
    let ours = devlanes_version::VERSION;
    let mut warnings = Vec::new();
    let theirs = label_version(label).unwrap_or("");
    if theirs != ours {
        warnings.push((
            "box_version",
            format!("the box runs devlanes {theirs} and this machine {ours}. Install the same release on both."),
        ));
    }
    if label.contains("(linger off)") {
        warnings.push((
            "box_linger_off",
            "the box stops your processes at logout. Run loginctl enable-linger or devlanes daemon install there.".to_owned(),
        ));
    }
    // A Windows box whose ssh session job kept the daemon the tunnel started.
    if label.contains("(daemon stops with the session)") {
        warnings.push((
            "box_daemon_in_session",
            "the box daemon stops when this SSH session ends. Run `devlanes daemon install` on the box".to_owned(),
        ));
    }
    warnings
}

/// Publishes state changes, records successful connects and restarts syncs on
/// every new session. A connection is recorded before it is published, so a
/// `connect` that saw it connected can rely on a restarted daemon restoring it.
async fn watch_link(engine: std::sync::Weak<Inner>, conn: Arc<HostConn>) {
    let mut state = conn.state.clone();
    let mut syncing: Option<JoinHandle<()>> = None;
    loop {
        let session = live(&state.borrow_and_update());
        let Some(inner) = engine.upgrade() else { break };
        {
            let mut since = lock(&conn.connected_since);
            match &session {
                Some(_) => {
                    since.get_or_insert_with(now_ms);
                }
                None => *since = None,
            }
        }
        if session.is_some() {
            // Paired, or never asked: a code left over must not go out later.
            lock(&conn.code).take();
        }
        if let Some(session) = &session {
            let record = devlanes_state::RemoteHost {
                name: conn.name.clone(),
                transport: conn.kind.to_owned(),
                address: conn.name.clone(),
                daemon_version: Some(session.server().to_owned()),
                last_connected: i64::try_from(now_ms()).ok(),
                // A box that connected is restored after a restart until
                // `remote disconnect`.
                wanted: true,
            };
            if let Err(error) = inner.store.upsert_remote_host(record).await {
                tracing::warn!(%error, host = %conn.name, "cannot record remote host");
            }
            conn.recorded.send_replace(true);
        }
        inner.bus.emit(wire::STATE_EVENT, conn.state_json());
        if let Some(session) = session {
            *lock(&conn.info) = session.box_info().clone();
            *lock(&conn.label) = session.server().to_owned();
            let findings = label_warnings(session.server());
            let known = {
                let mut warnings = lock(&conn.warnings);
                let known = warnings.clone();
                warnings.retain(|code, _| *code == RANGE_OVERLAP);
                warnings.extend(findings.iter().cloned());
                known
            };
            // A reconnect repeats a finding only in `remote ls`, not as an event.
            for (code, message) in findings {
                if known.get(code) == Some(&message) {
                    continue;
                }
                tracing::warn!(host = %conn.name, code, %message, "box warning");
                inner.bus.emit(
                    wire::WARNING_EVENT,
                    json!({"host": conn.name, "code": code, "message": message}),
                );
            }
            inner.refresh_range_overlaps();
            inner.reconcile_box_ports().await;
            // The mirror stamps the box's HTTPS port on its routes.
            conn.wake.notify_one();
            // A new session's box has no report of this mirror yet.
            conn.mark_changed();
            if let Some(task) = syncing.take() {
                task.abort();
            }
            syncing = Some(tokio::spawn(sync::run(
                engine.clone(),
                conn.clone(),
                session,
            )));
        } else if conn.ended() {
            inner.refresh_range_overlaps();
            inner.reconcile_box_ports().await;
        }
        drop(inner);
        tokio::select! {
            () = conn.cancel.cancelled() => break,
            changed = state.changed() => if changed.is_err() { break },
        }
    }
    if let Some(task) = syncing {
        task.abort();
    }
}

/// Emits [`wire::MIRROR_EVENT`] for `conn` after [`HostConn::mark_changed`],
/// at most once per [`MIRROR_EVENT_EVERY`]: the wakes of one burst share a
/// single event, which goes out once the burst has had that long to settle.
/// The live session gets the mirror report then too; the transport sends it
/// only when it differs from the session's last one and the box takes reports.
async fn announce_changes(engine: std::sync::Weak<Inner>, conn: Arc<HostConn>) {
    use futures_util::FutureExt as _;
    loop {
        tokio::select! {
            () = conn.cancel.cancelled() => return,
            () = conn.changed.notified() => {}
        }
        tokio::select! {
            () = conn.cancel.cancelled() => return,
            () = tokio::time::sleep(MIRROR_EVENT_EVERY) => {}
        }
        // A wake during the pause is covered by this event: a client reading
        // `host.list` after it sees that change too.
        let _ = conn.changed.notified().now_or_never();
        let Some(inner) = engine.upgrade() else {
            return;
        };
        if let Some(session) = conn.session() {
            let report = inner.mirror_report_of(&conn).await;
            match serde_json::to_value(report) {
                Ok(report) => {
                    session.report(report);
                }
                Err(error) => {
                    tracing::debug!(%error, host = %conn.name, "cannot encode the mirror report")
                }
            }
        }
        inner
            .bus
            .emit(wire::MIRROR_EVENT, json!({"host": conn.name}));
    }
}

impl Inner {
    /// `conn`'s mirror report with this machine's HTTPS port, and whether it
    /// serves the box's: the same port, or the box's extra port opened here.
    async fn mirror_report_of(&self, conn: &HostConn) -> wire::MirrorReport {
        let own = self.integ.routes.https_port();
        let theirs = lock(&conn.info).https_port;
        let serves = match (theirs, own) {
            (Some(theirs), Some(own)) if theirs != own => {
                self.remote.box_ports.lock().await.open.contains(&theirs)
            }
            (Some(_), Some(_)) => true,
            _ => false,
        };
        conn.mirror_report(own, serves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(project: &str, lane: &str, leaf: &str) -> SyncEntry {
        SyncEntry {
            lane: lane.into(),
            project: project.into(),
            unit: "web".into(),
            index: 0,
            from: PathBuf::from("/box/ext"),
            leaf: leaf.into(),
            ignore: Vec::new(),
        }
    }

    #[test]
    fn sync_destinations_stay_under_the_sync_root() {
        let home = Path::new("/home/u/.devlanes");
        assert_eq!(
            sync_destination(home, "box", &entry("app", "app@1", "ext")),
            Ok(PathBuf::from("/home/u/.devlanes/sync/box/app/app@1/ext"))
        );
        for bad in [
            entry("..", "app@1", "ext"),
            entry("app", "../..", "ext"),
            entry("app", "app@1", ".."),
            entry("app", "app@1", "a/b"),
            entry("app", "app@1", ""),
            entry("app", "a\0b", "ext"),
        ] {
            assert!(sync_destination(home, "box", &bad).is_err(), "{bad:?}");
        }
        assert!(sync_destination(home, "..", &entry("app", "app@1", "ext")).is_err());
        assert_eq!(
            sync_destination(home, "user@box/x", &entry("app", "app@1", "ext")),
            Ok(PathBuf::from(
                "/home/u/.devlanes/sync/user@box_x/app/app@1/ext"
            ))
        );
    }

    /// Windows encodes what the user typed as the host, and refuses box names
    /// it cannot store.
    #[cfg(windows)]
    #[test]
    fn windows_sync_destinations_hold_only_windows_names() {
        let home = Path::new(r"C:\Users\acme\.devlanes");
        assert_eq!(
            sync_destination(home, "ssh://box:2222", &entry("app", "app@1", "ext")),
            Ok(home.join(r"sync\ssh%3A__box%3A2222\app\app@1\ext"))
        );
        for bad in [
            entry("con", "app@1", "ext"),
            entry("app", "app@1", "a:b"),
            entry("app", "app@1", r"a\..\..\x"),
            entry("app", "app@1", "dist."),
        ] {
            assert!(sync_destination(home, "box", &bad).is_err(), "{bad:?}");
        }
        assert!(sync_destination(home, ".", &entry("app", "app@1", "ext")).is_err());
    }

    #[test]
    fn labels_report_version_skew_and_linger() {
        let ours = format!("devlanes {}", devlanes_version::VERSION);
        assert!(label_warnings(&ours).is_empty());
        let codes: Vec<_> = label_warnings("devlanes 0.0.0-other (linger off)")
            .into_iter()
            .map(|(code, _)| code)
            .collect();
        assert_eq!(codes, ["box_version", "box_linger_off"]);
    }

    /// A Windows box's tunnel says when the daemon it started stays inside
    /// the ssh session's job, in the words this check looks for.
    #[test]
    fn labels_report_a_daemon_bound_to_the_session() {
        let label = format!(
            "devlanes {} {}",
            devlanes_version::VERSION,
            devlanes_tunnel::SESSION_BOUND
        );
        let warnings = label_warnings(&label);
        let codes: Vec<_> = warnings.iter().map(|(code, _)| *code).collect();
        assert_eq!(codes, ["box_daemon_in_session"]);
        assert!(
            warnings[0].1.contains("devlanes daemon install"),
            "{warnings:?}"
        );
    }

    /// A transport that asks a password nobody answers, then fails with `error`.
    struct AsksThenFails {
        prompts: Arc<dyn RemotePrompter>,
        error: fn() -> TransportError,
    }

    #[async_trait]
    impl Transport for AsksThenFails {
        /// Asks, then fails whatever the answer.
        async fn connect(&self) -> Result<BoxDuplex, TransportError> {
            let _ = self
                .prompts
                .ask(RemotePrompt {
                    host: "box".into(),
                    prompt: "box's password: ".into(),
                    kind: "secret",
                })
                .await;
            Err((self.error)())
        }
    }

    /// [`AsksThenFails`] behind [`Attended`], with no CLI attached.
    fn unattended(error: fn() -> TransportError) -> Arc<Attended> {
        let board = Arc::new(PromptBoard::default());
        let prompts = Arc::new(HostPrompter {
            board: board.clone(),
        });
        Arc::new(Attended {
            inner: Arc::new(AsksThenFails { prompts, error }),
            board,
        })
    }

    /// After an unanswered prompt only an authentication failure waits for a
    /// sign-in; a transient failure is retried and a permanent one stays.
    #[tokio::test]
    async fn an_unanswered_prompt_reclassifies_only_auth_failures()
    -> Result<(), Box<dyn std::error::Error>> {
        let rejected = || TransportError::auth_failed("Permission denied (password).");
        let reset = || TransportError::transient("Connection reset by peer");
        let missing = || TransportError::permanent("devlanes is not installed");
        let Err(error) = unattended(rejected).connect().await else {
            panic!("connected");
        };
        assert!(error.is_needs_auth(), "{error}");
        let Err(error) = unattended(reset).connect().await else {
            panic!("connected");
        };
        assert!(!error.is_needs_auth() && !error.is_permanent(), "{error}");
        let Err(error) = unattended(missing).connect().await else {
            panic!("connected");
        };
        assert!(error.is_permanent() && !error.is_needs_auth(), "{error}");

        let link = Link::spawn(
            unattended(reset),
            SessionOptions::new("client"),
            devlanes_transport::Backoff::new(Duration::from_millis(10), Duration::from_millis(10)),
        );
        let mut state = link.state();
        let retrying = tokio::time::timeout(
            Duration::from_secs(5),
            state.wait_for(|state| matches!(state, LinkState::Retrying { .. })),
        )
        .await?
        .is_ok();
        assert!(retrying, "the link retries: {:?}", *link.state().borrow());
        link.shutdown().await;
        Ok(())
    }

    /// A prompt whose CLI detaches before it answers counts as unanswered,
    /// so the rejected login parks the link waiting for a sign-in, which a
    /// later `connect` resumes, instead of failing it.
    #[tokio::test]
    async fn a_prompt_left_by_its_cli_waits_for_sign_in() -> Result<(), Box<dyn std::error::Error>>
    {
        let board = Arc::new(PromptBoard::default());
        let (session, mut frames) = Session::local(1);
        let watching = board.watch(&session);
        let prompts = Arc::new(HostPrompter {
            board: board.clone(),
        });
        let transport = Arc::new(Attended {
            inner: Arc::new(AsksThenFails {
                prompts,
                error: || TransportError::auth_failed("Permission denied (password)."),
            }),
            board,
        });
        let link = Link::spawn(
            transport,
            SessionOptions::new("client"),
            devlanes_transport::Backoff::new(Duration::from_millis(10), Duration::from_millis(10)),
        );
        let asked = tokio::time::timeout(Duration::from_secs(5), frames.recv()).await?;
        assert!(asked.is_some(), "the prompt reached the CLI");
        // Esc in the TUI: the CLI goes away without answering.
        session.close();
        drop(watching);
        let mut state = link.state();
        let parked = tokio::time::timeout(
            Duration::from_secs(5),
            state.wait_for(|state| {
                matches!(
                    state,
                    LinkState::NeedsAuth { .. } | LinkState::Failed { .. }
                )
            }),
        )
        .await?
        .map(|state| matches!(*state, LinkState::NeedsAuth { .. }))?;
        assert!(parked, "needs a sign-in: {:?}", *link.state().borrow());
        link.shutdown().await;
        Ok(())
    }

    /// A prompt whose `ask` is dropped while it waits, as when the attempt
    /// times out, leaves no pending entry behind for a late answer.
    #[tokio::test]
    async fn a_dropped_prompt_is_forgotten() {
        let board = Arc::new(PromptBoard::default());
        let (session, mut frames) = Session::local(1);
        let _watching = board.watch(&session);
        let prompter = HostPrompter {
            board: board.clone(),
        };
        let mut asking = Box::pin(prompter.ask(RemotePrompt {
            host: "box".into(),
            prompt: "pairing code:".into(),
            kind: "code",
        }));
        let asked = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                _ = &mut asking => false,
                frame = frames.recv() => frame.is_some(),
            }
        })
        .await;
        assert_eq!(asked, Ok(true), "the prompt reached the session");
        assert_eq!(lock(&board.pending).len(), 1);
        drop(asking);
        assert!(lock(&board.pending).is_empty());
    }

    /// A connection to `box` whose link state is whatever the test sends.
    fn conn_with(state: watch::Receiver<LinkState>) -> Arc<HostConn> {
        conn_named("box", 0, state)
    }

    /// A connection to box `name`, connected `serial`th, whose link state is
    /// whatever the test sends.
    fn conn_named(name: &str, serial: u64, state: watch::Receiver<LinkState>) -> Arc<HostConn> {
        Arc::new(HostConn {
            name: name.into(),
            kind: "memory",
            state,
            registry: ClientRegistry::default(),
            link: tokio::sync::Mutex::new(None),
            cancel: CancellationToken::new(),
            board: Arc::default(),
            view: Mutex::new(mirror::View::default()),
            wake: tokio::sync::Notify::new(),
            mirror_task: Mutex::new(None),
            watch_task: Mutex::new(None),
            syncs: Mutex::new(BTreeMap::new()),
            hashes: Mutex::new(HashMap::new()),
            warnings: Mutex::new(BTreeMap::new()),
            serial,
            info: Mutex::new(BoxInfo::default()),
            label: Mutex::new(String::new()),
            proxy_port: Mutex::new(None),
            recorded: watch::Sender::new(false),
            applied: watch::Sender::new(0),
            connected_since: Mutex::new(None),
            changed: tokio::sync::Notify::new(),
            announce_task: Mutex::new(None),
            code: Arc::default(),
        })
    }

    /// A live session to an in-memory box with an empty registry.
    async fn session() -> Result<BoxSession, Box<dyn std::error::Error>> {
        let (client_io, box_io) = tokio::io::duplex(64 * 1024);
        let services = devlanes_transport::BoxServices {
            registry: Arc::new(devlanes_transport::SharedRegistry::new(
                devlanes_transport::RegistrySnapshot::default(),
                4,
            )),
            dialer: Arc::new(devlanes_transport::LoopbackDialer),
            sync: Arc::new(devlanes_transport::NoSync),
            pairing: Arc::new(devlanes_transport::NoPairing),
        };
        tokio::spawn(devlanes_transport::serve(
            box_io,
            services,
            devlanes_transport::ServeOptions::new("box"),
        ));
        Ok(BoxSession::establish(Box::new(client_io), SessionOptions::new("client")).await?)
    }

    /// A daemon is idle only with no box it would cut off and no pairing code
    /// outstanding: restoring, a code, or a box that is connected,
    /// reconnecting or waiting for a sign-in or pairing, keeps it up; a box
    /// whose link failed for good or stopped does not.
    #[tokio::test]
    async fn only_failed_and_stopped_boxes_leave_the_daemon_idle()
    -> Result<(), Box<dyn std::error::Error>> {
        let remotes = Remotes::default();
        assert!(remotes.is_idle(), "no box");
        remotes.restoring.store(true, Ordering::Relaxed);
        assert!(!remotes.is_idle(), "restoring");
        // A restore that panics still ends the restoring state.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _restored = ClearOnDrop(&remotes.restoring);
            panic!("restore failed");
        }));
        assert!(panicked.is_err());
        assert!(remotes.is_idle(), "restoring ended with the panic");
        // A code someone is about to type keeps the daemon up.
        lock(&remotes.codes)
            .mint(std::time::Instant::now())
            .map_err(|error| format!("{error:?}"))?;
        assert!(!remotes.is_idle(), "a pairing code is outstanding");
        *lock(&remotes.codes) = pairing::Codes::default();

        let (state, receiver) = watch::channel(LinkState::Connecting { attempt: 1 });
        lock(&remotes.hosts).insert("box".into(), conn_with(receiver));
        let session = session().await?;
        let busy = [
            LinkState::Connecting { attempt: 1 },
            LinkState::Connected(session.clone()),
            LinkState::Retrying {
                delay: Duration::from_secs(1),
                error: "Connection reset by peer".into(),
            },
            LinkState::NeedsAuth {
                error: "Permission denied (password).".into(),
            },
            LinkState::NeedsPairing {
                error: "the box serves only paired clients".into(),
                refused: false,
                retry_at: None,
            },
        ];
        for link in busy {
            let shown = describe(&link);
            state.send_replace(link);
            assert!(!remotes.is_idle(), "{shown}");
        }
        let done = [
            LinkState::Failed {
                error: "devlanes is not installed".into(),
            },
            LinkState::Stopped,
        ];
        for link in done {
            let shown = describe(&link);
            state.send_replace(link);
            assert!(remotes.is_idle(), "{shown}");
        }
        session.close();
        Ok(())
    }

    /// The code a `connect` gave goes when that request ends, unless a later
    /// `connect` replaced it meanwhile.
    #[test]
    fn a_given_code_goes_with_its_connect() {
        let slot: pairing::GivenCode = Arc::default();
        *lock(&slot) = Some(("K7QM-3XWD".into(), std::time::Instant::now()));
        drop(ForgetCode {
            slot: slot.clone(),
            code: "K7QM-3XWD".into(),
        });
        assert!(lock(&slot).is_none());
        *lock(&slot) = Some(("NEWR-CODE".into(), std::time::Instant::now()));
        drop(ForgetCode {
            slot: slot.clone(),
            code: "K7QM-3XWD".into(),
        });
        assert!(lock(&slot).is_some(), "a later connect's code stays");
    }

    /// The hand-over between two boxes: the box that won a lane waits for the
    /// one that lost it to release the port both use, wakes it once, and
    /// goes on as soon as the port is free. A port held for a lane its holder
    /// still mirrors is a plain conflict, so nothing waits for it, and a port
    /// never released ends the wait at [`RELEASE_WAIT`].
    #[tokio::test(start_paused = true)]
    async fn a_winner_waits_only_for_ports_the_loser_releases() {
        use futures_util::FutureExt as _;
        let (_state, receiver) = watch::channel(LinkState::Stopped);
        let winner = conn_named("fork-b", 2, receiver.clone());
        let loser = conn_named("fork-a", 1, receiver);
        let others = || vec![winner.clone(), loser.clone()];
        let mirroring = |host: &str| {
            let mut assignment = assign::Assignment::default();
            assignment
                .mirrored
                .insert((host.to_owned(), "acme@0".to_owned()));
            assignment
        };
        lock(&loser.view).bound.insert(3000, "acme@0:web".into());

        let start = tokio::time::Instant::now();
        assert!(mirror::wait_for_release(2, others, &[3000], &mirroring("fork-a")).await);
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "fork-a still mirrors acme@0"
        );
        assert!(mirror::wait_for_release(2, others, &[3001], &mirroring("fork-b")).await);
        assert_eq!(start.elapsed(), Duration::ZERO, "nobody holds 3001");

        let releasing = loser.clone();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            lock(&releasing.view).bound.remove(&3000);
        });
        assert!(mirror::wait_for_release(2, others, &[3000], &mirroring("fork-b")).await);
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_millis(100) && waited < Duration::from_millis(200),
            "{waited:?}"
        );
        assert!(
            loser.wake.notified().now_or_never().is_some(),
            "the loser was woken"
        );
        release.await.expect("release");

        lock(&loser.view).bound.insert(3000, "acme@0:web".into());
        let start = tokio::time::Instant::now();
        assert!(!mirror::wait_for_release(2, others, &[3000], &mirroring("fork-b")).await);
        assert!(start.elapsed() >= RELEASE_WAIT, "{:?}", start.elapsed());
    }

    /// Ranges overlap when they share one port; invalid ranges never parse.
    #[test]
    fn port_ranges_overlap_when_they_share_a_port() {
        let range = |text| parse_range(text).expect(text);
        assert!(ranges_overlap(range("20000-24999"), range("24999-29999")));
        assert!(ranges_overlap(range("20000-32767"), range("25000-26000")));
        assert!(!ranges_overlap(range("20000-24999"), range("25000-29999")));
        assert_eq!(parse_range("0-100"), None);
        assert_eq!(parse_range("300-200"), None);
        assert_eq!(parse_range("any"), None);
    }
}
