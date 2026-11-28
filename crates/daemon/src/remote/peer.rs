//! Box side of peer-to-peer connections: clients that paired by ticket
//! (`remote.pair` with `ticket`) and the endpoint that serves them.
//!
//! The endpoint's [`Admission`] decides in the handshake, before any
//! stream: a session ([`Alpn::Tunnel`]) only from a key pinned in
//! `p2p_clients`, a pairing connection ([`Alpn::Pair`]) from any key while
//! a ticket is outstanding. Wrong secrets pause nothing. A pairing
//! connection gets one bounded request and one answer
//! ([`answer_pairing`]); only a key the box pinned reaches
//! [`devlanes_transport::serve`], and there [`PeerGate`] serves it only
//! under the client id it paired with.
//!
//! The box listens while it has a client paired by ticket or a ticket is
//! outstanding, and never binds the endpoint otherwise: the rule is checked
//! when the daemon starts, when a ticket is minted, paired or revoked, and
//! when the last ticket expires.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use devlanes_proto::remote::{CLIENT_ID_PREFIX, RemoteClient, is_random_id};
use devlanes_state::{P2pClient, Store};
use devlanes_transport::{
    Admission, Alpn, BoxError, BoxInfo, BoxServerConnection, ClientInfo, PairAnswer, PairDesk,
    PairRequest, PairingCheck, PairingClaim, PairingGate, PairingReason, PeerEndpoint, PeerError,
    PeerId, PeerIncoming, ServeOptions, Timing, answer_pairing,
};
use tokio::{
    sync::Semaphore,
    task::{AbortHandle, JoinHandle},
};

use super::{
    feed::clean,
    identity, now_ms,
    pairing::{self, Codes},
};
use crate::{engine::Inner, integrations::BoxServing, model::lock};

/// Pairing connections answered at once. Any key may open one while a
/// ticket is out, so more are dropped at once instead of queued.
const PAIRING_SLOTS: usize = 4;

/// How long a pairing may take before its ticket is used up: reading the
/// user config, the first handshake label, which may run `loginctl`, and
/// waiting for a change of the listening rule or a bind. Past it the
/// client is refused and the ticket stays valid.
const DECIDE_WAIT: Duration = Duration::from_secs(3);

/// Wrong secrets logged one line each per [`WRONG_SECRET_WINDOW`]; the rest
/// are counted.
const LOGGED_WRONG_SECRETS: u32 = 3;
/// The window [`LOGGED_WRONG_SECRETS`] counts in.
const WRONG_SECRET_WINDOW: Duration = Duration::from_secs(10 * 60);

/// What a client is told for a wrong, expired or used secret. It does not
/// tell them apart, so a guesser learns nothing from it.
const WRONG_TICKET: &str = "the pairing ticket is wrong, expired or already used. Get a new one \
                            with `devlanes daemon pair` on the box";

/// What a client is told when its request has no usable client id.
const NO_CLIENT_ID: &str =
    "this machine sent no valid client id; update devlanes here and try again";

/// What a client is told when the box took too long before using the ticket.
const BUSY: &str = "the box took too long to answer; try again with the same ticket";

/// What a client is told when the box used the ticket but could not store
/// the pairing.
const STORE_FAILED: &str = "the box could not store the pairing; get a new ticket with `devlanes \
                            daemon pair` on the box and try again";

/// Least time between two lines about pairing connections dropped for
/// lack of a free slot.
const BUSY_LOG_EVERY: Duration = Duration::from_secs(60);

/// Counts pairing connections dropped for lack of a free slot between two
/// log lines.
#[derive(Default)]
struct BusyLog {
    /// When the last line was written.
    logged: Option<Instant>,
    /// Connections dropped since then.
    dropped: u32,
}

impl BusyLog {
    /// Counts one dropped connection at `now`; how many to report when a
    /// line is due. Drops counted since the last line are reported by the
    /// next drop after the minute, so a pending count is never written if
    /// no further drop arrives.
    fn note(&mut self, now: Instant) -> Option<u32> {
        self.dropped = self.dropped.saturating_add(1);
        let due = self
            .logged
            .is_none_or(|at| now.saturating_duration_since(at) >= BUSY_LOG_EVERY);
        if !due {
            return None;
        }
        self.logged = Some(now);
        Some(std::mem::take(&mut self.dropped))
    }
}

/// How [`Inner::note_wrong_secret`] logs one wrong secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Logged {
    /// On a line of its own.
    Each,
    /// On a line of its own that says the next ones are counted.
    Last,
    /// Only counted.
    Counted,
}

/// Counts wrong secrets per window, so they cannot fill the log.
#[derive(Default)]
struct WrongSecrets {
    /// When the current window began.
    since: Option<Instant>,
    /// Wrong secrets in it.
    seen: u32,
}

impl WrongSecrets {
    /// Counts one wrong secret at `now`: how to log it, and how many the
    /// window that just ended counted without logging.
    fn note(&mut self, now: Instant) -> (Logged, u32) {
        let mut quiet = 0;
        if self
            .since
            .is_none_or(|since| now.saturating_duration_since(since) >= WRONG_SECRET_WINDOW)
        {
            quiet = self.seen.saturating_sub(LOGGED_WRONG_SECRETS);
            self.since = Some(now);
            self.seen = 0;
        }
        self.seen = self.seen.saturating_add(1);
        let logged = match self.seen.cmp(&LOGGED_WRONG_SECRETS) {
            std::cmp::Ordering::Less => Logged::Each,
            std::cmp::Ordering::Equal => Logged::Last,
            std::cmp::Ordering::Greater => Logged::Counted,
        };
        (logged, quiet)
    }
}

/// Pinned keys and the client id each was paired under.
type Pinned = Arc<Mutex<HashMap<PeerId, String>>>;

/// Tasks answering peers, by serial: what they are for and how to end them.
type PeerTasks = Arc<Mutex<BTreeMap<u64, (PeerId, Alpn, AbortHandle)>>>;

/// The box's peer-to-peer state, part of [`super::Remotes`].
pub(crate) struct PeerState {
    pinned: Pinned,
    /// Whether the endpoint listens, which keeps the daemon from exiting idle.
    listening: AtomicBool,
    /// Held while the listening rule is applied and while the pinned keys
    /// change, so a change and the listening state it implies are applied
    /// in the order they happened.
    rule: tokio::sync::Mutex<()>,
    /// The accept loop, started the first time the endpoint listens.
    accept: Mutex<Option<JoinHandle<()>>>,
    /// Loads the pinned keys at daemon start; awaited on shutdown, since it
    /// reads the store.
    load: Mutex<Option<JoinHandle<()>>>,
    /// When the last outstanding ticket expires, and the timer that applies
    /// the listening rule then.
    expiry: Mutex<Option<(Instant, JoinHandle<()>)>>,
    tasks: PeerTasks,
    next_task: AtomicU64,
    pairing_slots: Arc<Semaphore>,
    wrong_secrets: Mutex<WrongSecrets>,
    busy: Mutex<BusyLog>,
}

impl Default for PeerState {
    /// Nothing pinned, not listening, [`PAIRING_SLOTS`] free.
    fn default() -> Self {
        Self {
            pinned: Pinned::default(),
            listening: AtomicBool::new(false),
            rule: tokio::sync::Mutex::new(()),
            accept: Mutex::new(None),
            load: Mutex::new(None),
            expiry: Mutex::new(None),
            tasks: PeerTasks::default(),
            next_task: AtomicU64::new(0),
            pairing_slots: Arc::new(Semaphore::new(PAIRING_SLOTS)),
            wrong_secrets: Mutex::default(),
            busy: Mutex::default(),
        }
    }
}

impl PeerState {
    /// Whether the endpoint listens for peers.
    pub(super) fn is_listening(&self) -> bool {
        self.listening.load(Ordering::SeqCst)
    }
}

/// The listening rule: a box listens while a client paired by ticket may
/// dial it (`pinned`) or a ticket is outstanding (`tickets_until`), and
/// never otherwise, so a box nobody pairs with by ticket opens no socket.
fn should_listen(pinned: bool, tickets_until: Option<Instant>) -> bool {
    pinned || tickets_until.is_some()
}

/// Who the endpoint lets in, read on its accept path. Only memory is read,
/// under locks the daemon never holds across an await.
struct PeerAdmission {
    pinned: Pinned,
    codes: Arc<Mutex<Codes>>,
}

impl Admission for PeerAdmission {
    /// A session from a pinned key; a pairing connection from any key while
    /// a ticket is outstanding, also while wrong codes pause codes.
    fn admit(&self, peer: &PeerId, alpn: Alpn) -> bool {
        match alpn {
            Alpn::Tunnel => lock(&self.pinned).contains_key(peer),
            Alpn::Pair => lock(&self.codes).takes_tickets(Instant::now()),
        }
    }
}

/// The pairing gate of a session with a pinned key: it serves the client
/// only under the client id the key was paired with, so a paired machine
/// cannot appear as another client in `daemon clients` or a revoke. The
/// pinned set is read when the session starts, so a key revoked since its
/// handshake is refused too.
struct PeerGate {
    pinned: Pinned,
    peer: PeerId,
    store: Store,
}

#[async_trait]
impl PairingGate for PeerGate {
    /// Allowed for the pinned client id, which records the session; else
    /// [`PairingReason::Invalid`], which a client paired by ticket shows as
    /// needing a new ticket.
    async fn check(&self, client: &ClientInfo, _: Option<&str>) -> Result<PairingCheck, BoxError> {
        let expected = lock(&self.pinned).get(&self.peer).cloned();
        if expected.as_deref() != Some(client.id.as_str()) {
            tracing::warn!(
                peer = %self.peer.short(), client_id = %clean(&client.id),
                "refused a session whose client id is not the one its key paired with"
            );
            return Ok(PairingCheck::Required(PairingReason::Invalid));
        }
        let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        let touched = self
            .store
            .touch_p2p_client(self.peer.to_string(), clean(&client.name), at)
            .await;
        if let Err(error) = touched {
            tracing::warn!(%error, peer = %self.peer.short(), "cannot record when a client was last seen");
        }
        Ok(PairingCheck::Allowed)
    }

    /// Refused: a client paired by ticket pairs again with a new ticket,
    /// never with a code over its session.
    async fn claim(&self, _: &ClientInfo, _: &str) -> Result<PairingClaim, BoxError> {
        Ok(PairingClaim::Refused {
            message: "this machine pairs with the box by ticket; get a new ticket with `devlanes \
                      daemon pair` on the box"
                .into(),
            retry_after: None,
        })
    }
}

/// Answers one pairing connection from `peer`.
struct Desk {
    engine: Arc<Inner>,
    peer: PeerId,
}

#[async_trait]
impl PairDesk for Desk {
    /// [`Inner::decide_pairing`] for this connection's key.
    async fn decide(&self, request: &PairRequest) -> PairAnswer {
        self.engine.decide_pairing(self.peer, request).await
    }
}

/// A refusal with `message`.
fn refused(message: &str, retry_after_ms: Option<u64>) -> PairAnswer {
    PairAnswer::Refused {
        message: message.to_owned(),
        retry_after_ms,
    }
}

/// Why connections are refused, in the words `devlanes tunnel --stdio`
/// uses for the same setting (`connections_refusal` in `bins/devlanes`), so
/// a client shows the same reason over either transport.
fn connections_refusal(invalid: bool) -> &'static str {
    if invalid {
        "the box does not accept connections because the remote settings in its user config \
         are invalid. Run devlanes doctor on the box to see them"
    } else {
        "the box does not accept connections. Set remote.allow_connections to true there"
    }
}

/// `serving`'s handshake label, read on a blocking thread since reading it
/// may run a program.
async fn label_of(serving: Arc<dyn BoxServing>) -> String {
    tokio::task::spawn_blocking(move || serving.label())
        .await
        .unwrap_or_else(|_| format!("devlanes {}", devlanes_version::VERSION))
}

/// Forgets a finished task in [`PeerState::tasks`], however it ends.
struct TaskGuard {
    tasks: PeerTasks,
    serial: u64,
}

impl Drop for TaskGuard {
    /// Removes the task's entry.
    fn drop(&mut self) {
        lock(&self.tasks).remove(&self.serial);
    }
}

impl Inner {
    /// The endpoint and the services it serves with; `None` unless the
    /// composition root wired both.
    fn peer_wiring(&self) -> Option<(Arc<dyn PeerEndpoint>, Arc<dyn BoxServing>)> {
        Some((self.integ.peer.clone()?, self.integ.box_serving.clone()?))
    }

    /// At daemon start: loads the keys of the clients paired by ticket and
    /// listens when there are any. In the background; a request that mints
    /// a ticket meanwhile applies the same rule.
    pub(crate) fn start_peer_listener(self: &Arc<Self>) {
        if self.peer_wiring().is_none() {
            return;
        }
        let engine = self.clone();
        let task = tokio::spawn(async move {
            {
                // Rows and pins change together under this lock, so the
                // pins loaded here match the rows read.
                let _rule = engine.pairing_rule().await;
                match engine.store.p2p_clients().await {
                    Ok(clients) => {
                        let mut pinned = lock(&engine.remote.peer.pinned);
                        for client in clients {
                            match client.peer_key.parse::<PeerId>() {
                                Ok(key) => {
                                    pinned.insert(key, client.client_id);
                                }
                                Err(_) => tracing::warn!(
                                    client_id = %client.client_id,
                                    "skipping a client paired by ticket whose stored key is damaged"
                                ),
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "cannot read the clients paired by ticket");
                    }
                }
            }
            if let Err(error) = engine.refresh_listening().await {
                tracing::warn!(%error, "cannot listen for clients paired by ticket");
            }
        });
        *lock(&self.remote.peer.load) = Some(task);
    }

    /// Applies the listening rule ([`should_listen`]) to the endpoint and
    /// sets the timer for the last outstanding ticket's expiry. Fails when
    /// the endpoint could not start listening.
    pub(super) async fn refresh_listening(self: &Arc<Self>) -> Result<(), PeerError> {
        let Some((endpoint, _)) = self.peer_wiring() else {
            return Ok(());
        };
        let state = &self.remote.peer;
        let _rule = state.rule.lock().await;
        let now = Instant::now();
        let pinned = lock(&state.pinned).len();
        let until = lock(&self.remote.codes).tickets_until(now);
        let listening = state.is_listening();
        if should_listen(pinned > 0, until) && !listening {
            let admission = Arc::new(PeerAdmission {
                pinned: state.pinned.clone(),
                codes: self.remote.codes.clone(),
            });
            endpoint.listen(Some(admission)).await?;
            state.listening.store(true, Ordering::SeqCst);
            self.start_accepting(endpoint);
            tracing::info!(
                clients = pinned,
                ticket = until.is_some(),
                "listening for peer-to-peer connections"
            );
        } else if !should_listen(pinned > 0, until) && listening {
            endpoint.listen(None).await?;
            state.listening.store(false, Ordering::SeqCst);
            tracing::info!(
                "stopped listening for peer-to-peer connections: no client is paired by ticket \
                 and no ticket is outstanding"
            );
        }
        self.schedule_expiry(until);
        Ok(())
    }

    /// Applies the listening rule again at `until`, when the last
    /// outstanding ticket expires. Called under the rule lock. Only a later
    /// deadline replaces the timer: the timer applies the rule itself, and
    /// replacing it from there would abort the running task.
    fn schedule_expiry(self: &Arc<Self>, until: Option<Instant>) {
        let Some(until) = until else {
            return;
        };
        let mut slot = lock(&self.remote.peer.expiry);
        if slot.as_ref().is_some_and(|(at, _)| *at >= until) {
            return;
        }
        if let Some((_, old)) = slot.take() {
            // Not in the middle of the rule: it applies it under the rule
            // lock this caller holds, so it is done or has not started.
            old.abort();
        }
        let engine = Arc::downgrade(self);
        let timer = tokio::spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(until)).await;
            if let Some(engine) = engine.upgrade()
                && let Err(error) = engine.refresh_listening().await
            {
                tracing::debug!(%error, "cannot stop listening after the last ticket expired");
            }
        });
        *slot = Some((until, timer));
    }

    /// Starts the loop that takes admitted connections, once. It outlives
    /// a `listen(None)`, since `accept` waits while the endpoint does not
    /// listen, and ends when the endpoint closes.
    fn start_accepting(self: &Arc<Self>, endpoint: Arc<dyn PeerEndpoint>) {
        let mut slot = lock(&self.remote.peer.accept);
        if slot.is_some() {
            return;
        }
        let engine = Arc::downgrade(self);
        *slot = Some(tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let Some(engine) = engine.upgrade() else {
                    return;
                };
                engine.handle_incoming(incoming);
            }
        }));
    }

    /// Hands `incoming` to a task of its own at once: the endpoint drops
    /// admitted connections while `accept` falls behind.
    fn handle_incoming(self: &Arc<Self>, incoming: PeerIncoming) {
        let PeerIncoming {
            peer,
            alpn,
            connection,
        } = incoming;
        match alpn {
            Alpn::Pair => {
                let Ok(slot) = self.remote.peer.pairing_slots.clone().try_acquire_owned() else {
                    // At info, so the owner sees why their pairing failed;
                    // one line a minute, since any key can cause it.
                    let dropped = lock(&self.remote.peer.busy).note(Instant::now());
                    if let Some(dropped) = dropped {
                        tracing::info!(
                            peer = %peer.short(), dropped,
                            "dropped pairing connections because all {PAIRING_SLOTS} pairing \
                             slots were busy; `dropped` counts them since the last such line"
                        );
                    }
                    return;
                };
                let desk = Desk {
                    engine: self.clone(),
                    peer,
                };
                self.spawn_peer_task(peer, alpn, async move {
                    let _slot = slot;
                    if let Err(error) = answer_pairing(connection, &desk).await {
                        tracing::debug!(%error, peer = %peer.short(), "a pairing connection failed");
                    }
                });
            }
            Alpn::Tunnel => {
                let engine = self.clone();
                self.spawn_peer_task(peer, alpn, async move {
                    engine.serve_peer(peer, connection).await;
                });
            }
        }
    }

    /// Runs `work` for `peer` in a task listed in [`PeerState::tasks`]
    /// until it ends, so a revoke and the shutdown can end it.
    fn spawn_peer_task(
        &self,
        peer: PeerId,
        alpn: Alpn,
        work: impl Future<Output = ()> + Send + 'static,
    ) {
        let tasks = self.remote.peer.tasks.clone();
        let serial = self.remote.peer.next_task.fetch_add(1, Ordering::Relaxed);
        let guard = TaskGuard {
            tasks: tasks.clone(),
            serial,
        };
        // Listed under the lock the guard takes, so a task that ends at
        // once is listed before its guard forgets it.
        let mut listed = lock(&tasks);
        let task = tokio::spawn(async move {
            let _guard = guard;
            work.await;
        });
        listed.insert(serial, (peer, alpn, task.abort_handle()));
    }

    /// The lock under which `p2p_clients` rows and pinned keys change
    /// together: whoever writes rows holds it until the pins match them, so
    /// a pairing, a revoke, a reset and the startup load never leave a key
    /// pinned without a row, which nothing could revoke, or a row without
    /// its pin. Not re-entrant: [`Inner::refresh_listening`] takes it too,
    /// so callers drop it first.
    pub(crate) async fn pairing_rule(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.remote.peer.rule.lock().await
    }

    /// Removes the pinned keys that `picked` chooses by key and client id
    /// from the admission and returns them. `_rule` is the guard of
    /// [`Inner::pairing_rule`], held since the matching rows were deleted.
    pub(super) fn unpin_peers(
        &self,
        _rule: &tokio::sync::MutexGuard<'_, ()>,
        picked: impl Fn(&PeerId, &str) -> bool,
    ) -> Vec<PeerId> {
        let mut pinned = lock(&self.remote.peer.pinned);
        let keys: Vec<PeerId> = pinned
            .iter()
            .filter(|(key, id)| picked(key, id))
            .map(|(key, _)| *key)
            .collect();
        for key in &keys {
            pinned.remove(key);
        }
        keys
    }

    /// Closes the sessions of `keys`, which [`Inner::unpin_peers`] removed
    /// first: the endpoint closes them with the refused-key code, so their
    /// clients show that reason, and their serving tasks end.
    pub(super) fn close_peers(&self, keys: &[PeerId]) {
        if keys.is_empty() {
            return;
        }
        if let Some(endpoint) = &self.integ.peer {
            for key in keys {
                endpoint.close_peer(key);
            }
        }
        self.end_peer_sessions(|key| keys.contains(key));
        for key in keys {
            tracing::info!(peer = %key.short(), "closed the sessions of a key that is no longer paired");
        }
    }

    /// Ends the sessions of the keys `ended` picks: their serving tasks stop
    /// and drop their connections.
    fn end_peer_sessions(&self, ended: impl Fn(&PeerId) -> bool) {
        let handles: Vec<AbortHandle> = lock(&self.remote.peer.tasks)
            .values()
            .filter(|(peer, alpn, _)| *alpn == Alpn::Tunnel && ended(peer))
            .map(|(_, _, handle)| handle.clone())
            .collect();
        // Aborted outside the lock, which each task's guard takes.
        for handle in handles {
            handle.abort();
        }
    }

    /// Whether `remote.allow_connections` lets clients in, and whether a
    /// problem in the user config made it false.
    async fn connections_allowed(&self) -> (bool, bool) {
        let home = self.opts.home.clone();
        tokio::task::spawn_blocking(move || pairing::allow_connections(&home))
            .await
            .unwrap_or((false, true))
    }

    /// Serves a session with the pinned key `peer`, or turns it away with
    /// the reason when `remote.allow_connections` is off, as `devlanes
    /// tunnel --stdio` does. The setting is read for every session.
    async fn serve_peer(self: Arc<Self>, peer: PeerId, connection: BoxServerConnection) {
        let Some((_, serving)) = self.peer_wiring() else {
            return;
        };
        let (allowed, invalid) = self.connections_allowed().await;
        if !allowed {
            tracing::info!(peer = %peer.short(), "turned a client paired by ticket away: connections are off");
            let reason = connections_refusal(invalid).to_owned();
            if let Err(error) =
                devlanes_transport::refuse(connection, reason, Timing::default()).await
            {
                tracing::debug!(%error, peer = %peer.short(), "the turned-away client did not read why");
            }
            return;
        }
        let label = label_of(serving.clone()).await;
        let mut services = serving.services(RemoteClient {
            pid: std::process::id(),
            ssh: None,
            since_ms: now_ms(),
            ..RemoteClient::default()
        });
        services.pairing = Arc::new(PeerGate {
            pinned: self.remote.peer.pinned.clone(),
            peer,
            store: self.store.clone(),
        });
        let mut options = ServeOptions::new(label);
        // `PeerGate` takes no code, so a session it refuses gets no time to
        // type one.
        options.pairing_timeout = options.timing.handshake_timeout;
        // What `daemon.status` answers the tunnel with, from this daemon itself.
        options.box_info = BoxInfo {
            https_port: self.https_port,
            ports_range: Some(self.opts.user.ports.range.clone()),
        };
        if let Err(error) = devlanes_transport::serve(connection, services, options).await {
            tracing::debug!(%error, peer = %peer.short(), "a session with a client paired by ticket ended");
        }
    }

    /// Decides the pairing request `request` of key `peer` and answers with
    /// this box's name. In this order: the client id; the secret, compared
    /// without using it up, so a peer without a valid ticket learns nothing
    /// else, not even whether connections are off; within [`DECIDE_WAIT`],
    /// the setting, the label and [`Inner::pairing_rule`]; then the ticket
    /// is used up and the key stored and pinned under that lock. Nothing
    /// after the claim may be cancelled or time out, since a ticket used up
    /// with nothing pinned is lost to its owner. The secret is never logged.
    async fn decide_pairing(self: &Arc<Self>, peer: PeerId, request: &PairRequest) -> PairAnswer {
        let Some((_, serving)) = self.peer_wiring() else {
            return refused("this box does not pair by ticket", None);
        };
        let id = clean(&request.client_id);
        let name = clean(&request.client_name);
        if !is_random_id(&id, CLIENT_ID_PREFIX) {
            return refused(NO_CLIENT_ID, None);
        }
        let secret = request.secret();
        let secret = secret.as_ref().map(|secret| &secret.0);
        if !lock(&self.remote.codes).holds_ticket(secret, Instant::now()) {
            self.note_wrong_secret(peer, &id, &name);
            return refused(WRONG_TICKET, None);
        }
        let ready = tokio::time::timeout(DECIDE_WAIT, async {
            let allowed = self.connections_allowed().await;
            let label = label_of(serving.clone()).await;
            (allowed, label, self.pairing_rule().await)
        })
        .await;
        let Ok(((allowed, invalid), label, rule)) = ready else {
            tracing::warn!(peer = %peer.short(), client_id = %id, "a pairing took too long; its ticket is still valid");
            return refused(BUSY, None);
        };
        if !allowed {
            return refused(connections_refusal(invalid), None);
        }
        let Some(ephemeral) = lock(&self.remote.codes).claim_ticket(secret, Instant::now()) else {
            // Expired, or used by another connection since the check.
            return refused(WRONG_TICKET, None);
        };
        let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        let stored = self
            .store
            .add_p2p_client(
                P2pClient {
                    peer_key: peer.to_string(),
                    client_id: id.clone(),
                    name: name.clone(),
                    paired_at: at,
                    last_seen: Some(at),
                },
                pairing::MAX_PAIRINGS,
            )
            .await;
        let dropped = match stored {
            Ok(dropped) => dropped,
            Err(error) => {
                drop(rule);
                tracing::warn!(%error, peer = %peer.short(), client_id = %id, "cannot store a pairing by ticket");
                // The used ticket may have been the last reason to listen.
                if let Err(error) = self.refresh_listening().await {
                    tracing::debug!(%error, "cannot apply the listening rule after a failed pairing");
                }
                return refused(STORE_FAILED, None);
            }
        };
        let closing = {
            let mut pinned = lock(&self.remote.peer.pinned);
            let earlier = pinned.insert(peer, id.clone());
            let mut closing: Vec<PeerId> = dropped
                .iter()
                .filter_map(|key| key.parse::<PeerId>().ok())
                .filter(|key| *key != peer)
                .collect();
            for key in &closing {
                pinned.remove(key);
            }
            // A key paired again under another client id keeps no session
            // that still runs under the old one.
            if earlier.is_some_and(|earlier| earlier != id) {
                closing.push(peer);
            }
            closing
        };
        drop(rule);
        self.close_peers(&closing);
        tracing::info!(peer = %peer.short(), client_id = %id, %name, ephemeral, "paired a client by ticket");
        self.clients_changed();
        if let Err(error) = self.refresh_listening().await {
            tracing::warn!(%error, "cannot listen for clients paired by ticket");
        }
        PairAnswer::Paired {
            name: identity::host_name(),
            server: label,
        }
    }

    /// Logs a wrong secret from `peer`, one line each for the first few of
    /// a window and then a count, since anyone who knows the box's id can
    /// send them. The count of a window is written when the next wrong
    /// secret opens a new one, so it is never written if none arrives.
    fn note_wrong_secret(&self, peer: PeerId, id: &str, name: &str) {
        let (each, quiet) = lock(&self.remote.peer.wrong_secrets).note(Instant::now());
        if quiet > 0 {
            tracing::warn!(
                quiet,
                "refused {quiet} more pairing tickets in the previous 10-minute window without \
                 logging them"
            );
        }
        match each {
            Logged::Each => {
                tracing::warn!(peer = %peer.short(), client_id = %id, %name, "refused a pairing ticket")
            }
            Logged::Last => tracing::warn!(
                peer = %peer.short(), client_id = %id, %name,
                "refused a pairing ticket; more until this 10-minute window ends are counted, not \
                 logged"
            ),
            Logged::Counted => {}
        }
    }

    /// On daemon shutdown: closes the endpoint, so clients see a clean
    /// close, and ends every task serving a peer.
    pub(super) async fn peer_shutdown(&self) {
        let load = lock(&self.remote.peer.load).take();
        if let Some(load) = load {
            let _ = load.await;
        }
        if let Some(endpoint) = &self.integ.peer {
            endpoint.close().await;
        }
        let accept = lock(&self.remote.peer.accept).take();
        let expiry = lock(&self.remote.peer.expiry).take();
        for task in accept.into_iter().chain(expiry.map(|(_, timer)| timer)) {
            task.abort();
        }
        let handles: Vec<AbortHandle> = lock(&self.remote.peer.tasks)
            .values()
            .map(|(_, _, handle)| handle.clone())
            .collect();
        for handle in handles {
            handle.abort();
        }
        self.remote.peer.listening.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use devlanes_transport::{PeerAddr, Ticket, TicketSecret};

    use super::*;

    /// Wrong secrets get a line each up to the third, which says the rest
    /// of its window is only counted; the next window starts with the
    /// count of the one before.
    #[test]
    fn wrong_secrets_are_logged_then_counted() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        let rows = [
            (Duration::ZERO, Logged::Each, 0),
            (second, Logged::Each, 0),
            (second * 2, Logged::Last, 0),
            (second * 3, Logged::Counted, 0),
            (WRONG_SECRET_WINDOW - second, Logged::Counted, 0),
            (WRONG_SECRET_WINDOW, Logged::Each, 2),
            (WRONG_SECRET_WINDOW + second, Logged::Each, 0),
        ];
        let mut secrets = WrongSecrets::default();
        for (after, logged, quiet) in rows {
            assert_eq!(secrets.note(start + after), (logged, quiet), "{after:?}");
        }
    }

    /// The first dropped pairing connection is logged at once, drops within
    /// the next minute are only counted, and the first drop after it
    /// reports them with itself.
    #[test]
    fn dropped_pairings_are_logged_once_a_minute() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        let rows = [
            (Duration::ZERO, Some(1)),
            (second, None),
            (BUSY_LOG_EVERY - second, None),
            (BUSY_LOG_EVERY, Some(3)),
            (BUSY_LOG_EVERY + second, None),
        ];
        let mut busy = BusyLog::default();
        for (after, reported) in rows {
            assert_eq!(busy.note(start + after), reported, "{after:?}");
        }
    }

    /// The smallest ticket the box mints, with no relay and no address, is
    /// hidden from output: redaction keeps its own copy of the prefix and
    /// its own idea of a ticket's length.
    #[test]
    fn output_hides_the_shortest_ticket() {
        let ticket = Ticket {
            addr: PeerAddr {
                id: PeerId([7; 32]),
                relay: None,
                direct: Vec::new(),
            },
            secret: TicketSecret([9; 16]),
            ephemeral: false,
        };
        let text = format!("run devlanes remote connect {} there", ticket.encode());
        let shown =
            devlanes_core::redact::Redactor::screen(std::iter::empty::<(&str, &str)>()).text(&text);
        assert_eq!(shown, "run devlanes remote connect [redacted] there");
    }
}
