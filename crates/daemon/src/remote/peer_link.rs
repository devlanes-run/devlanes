//! Client side of peer-to-peer connections: dialing a box paired by ticket
//! ([`PeerTransport`]) and how long its link keeps trying ([`PeerAttempts`]).
//!
//! Both wrap [`Transport`] for [`devlanes_transport::Link`], whose reconnect
//! rules stay as they are for every box: a P2P box only differs in that it
//! stops being retried after a day away, an hour for an ephemeral one, and
//! at once when the box no longer accepts this machine's key.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use devlanes_proto::{
    RpcError,
    remote::{PAIRING_CODE_TTL_MS, TICKET_PREFIX},
};
use devlanes_state::P2pBox;
use devlanes_transport::{
    Alpn, BoxConnection, BoxDuplex, Connection, ConnectionError, PairAnswer, PairRequest, PeerAddr,
    PeerEndpoint, PeerError, PeerId, Ticket, Transport, TransportError, request_pairing,
};
use futures_util::FutureExt as _;
use serde_json::json;
use tokio::{sync::OwnedMutexGuard, time::Instant};

use super::feed::clean;
use crate::{engine::Inner, model::lock};

/// How long a box paired by ticket may stay unreachable before its link
/// stops trying.
pub(super) const RETRY_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// [`RETRY_FOR`] for a box paired with an ephemeral ticket, which is then
/// forgotten.
pub(super) const EPHEMERAL_RETRY_FOR: Duration = Duration::from_secs(60 * 60);

/// The least a link the restore opens tries before it may give up, also
/// when the box was last reached longer ago than its window: the time this
/// machine was off says nothing about the box, and right after a start the
/// network may not be up yet.
pub(super) const RESTORE_GRACE: Duration = Duration::from_secs(10 * 60);

/// Why a link to a box paired by ticket stopped for good. Not a
/// [`PeerError`]: their texts are about one dial, and wrapping these in one
/// would say twice that the box is not reachable.
#[derive(Debug, thiserror::Error)]
pub(super) enum PeerLinkError {
    /// The box stayed unreachable for its window; `since` says for how
    /// long, as [`Unreached::text`] words it.
    #[error(
        "box {name} has not been reachable {since}, so devlanes stopped trying. Run `devlanes \
         remote connect {name}` once it is back."
    )]
    GaveUp {
        /// The box's name here.
        name: String,
        /// Such as "for 24 hours".
        since: String,
    },
    /// An ephemeral box stayed unreachable for its window, so it is
    /// forgotten.
    #[error(
        "box {name} has not been reachable {since}, so devlanes stopped trying and forgot it. To \
         use it again, run `devlanes daemon pair` on the box, then `devlanes remote connect \
         <ticket>` here."
    )]
    GaveUpEphemeral {
        /// The box's name here.
        name: String,
        /// Such as "for an hour".
        since: String,
    },
    /// The box refused this machine's key, as after `daemon revoke` there.
    #[error(
        "box {name} no longer accepts this machine. Run `devlanes daemon pair` on the box, then \
         `devlanes remote connect <ticket>` here."
    )]
    Refused {
        /// The box's name here.
        name: String,
    },
}

/// Longest text a box chose that this daemon passes on in a P2P message,
/// in bytes. More than [`clean`] keeps, which is sized for names and would
/// cut this daemon's own messages around such a text.
const MAX_REASON: usize = 1024;

/// `text`, part of which a box may have chosen (a close reason, a decode
/// error quoting what it sent), without the characters [`clean`] drops,
/// cut to [`MAX_REASON`] bytes. Each character goes through [`clean`], so
/// the two never differ in what they keep. `devlanes-transport-iroh`
/// cleans the close reasons it reports with its own list of format
/// characters, in its `text.rs`; that list and `clean`'s should stay the
/// same.
pub(super) fn clean_reason(text: &str) -> String {
    let mut cleaned = String::new();
    let mut buffer = [0u8; 4];
    for c in text.chars() {
        let kept = clean(c.encode_utf8(&mut buffer));
        if cleaned.len() + kept.len() > MAX_REASON {
            break;
        }
        cleaned.push_str(&kept);
    }
    cleaned
}

/// What [`PeerTransport`] remembers of the last connection it returned.
#[derive(Default)]
struct LastConnection {
    /// The serial of that connection; an older one writes nothing here.
    serial: u64,
    /// The box closed that connection because it refused this machine's key.
    refused: bool,
}

/// Dials one box paired by ticket, pinned to the key the pairing stored.
///
/// A box that revokes its last client paired by ticket closes that client's
/// session as refused and then stops listening, so the next dial finds no
/// listener, which is transient. So every connection this returns reports a
/// refusal it sees back here, and the next [`Transport::connect`] fails as
/// refused, for good, without dialing. Only the record is shared: a strong
/// reference to an earlier connection would keep that QUIC connection open.
pub(super) struct PeerTransport {
    endpoint: Arc<dyn PeerEndpoint>,
    addr: PeerAddr,
    last: Arc<Mutex<LastConnection>>,
}

impl PeerTransport {
    /// Dials `addr` through `endpoint`. A new transport, as a new link gets,
    /// remembers no refusal.
    pub(super) fn new(endpoint: Arc<dyn PeerEndpoint>, addr: PeerAddr) -> Self {
        Self {
            endpoint,
            addr,
            last: Arc::default(),
        }
    }
}

#[async_trait]
impl Transport for PeerTransport {
    /// A session connection to the box, or [`PeerError::Refused`] without a
    /// dial when the box refused the last one.
    async fn connect(&self) -> Result<BoxConnection, TransportError> {
        if lock(&self.last).refused {
            return Err(PeerError::Refused.into());
        }
        let inner = self.endpoint.connect(&self.addr, Alpn::Tunnel).await?;
        let serial = {
            let mut last = lock(&self.last);
            last.serial += 1;
            last.refused = false;
            last.serial
        };
        Ok(Box::new(Watched {
            inner,
            last: self.last.clone(),
            serial,
        }))
    }
}

/// A connection [`PeerTransport`] returned, which writes a refusal it sees
/// into the transport's record.
///
/// The session asks [`Connection::failure_within`] when the connection
/// closes, on a read error and on a failed handshake, but not when it ends
/// through a failed ping or report write, which can coincide with the
/// box's refusal. Every ending calls [`Connection::shutdown`], so that and
/// [`Connection::closed`] look for the refusal too.
struct Watched {
    inner: BoxConnection,
    last: Arc<Mutex<LastConnection>>,
    serial: u64,
}

impl Watched {
    /// Records `reason` as a refusal when it is the box refusing this
    /// machine's key and this is still the transport's newest connection.
    fn note(&self, reason: Option<&str>) {
        // `contains`: the memory endpoint's reason comes wrapped in yamux's.
        let refused = PeerError::Refused.to_string();
        if reason.is_some_and(|reason| reason.contains(&refused)) {
            let mut last = lock(&self.last);
            if last.serial == self.serial {
                last.refused = true;
            }
        }
    }

    /// Records the failure reason of a connection that is already closed,
    /// without waiting. Runs from `shutdown`, which is not async and may run
    /// from a drop: only a future that is ready is polled, so no timer is
    /// waited on, and nothing runs outside a runtime, where creating one
    /// would panic.
    fn note_if_closed(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.inner.closed().now_or_never().is_none() {
            return;
        }
        if let Some(reason) = self.inner.failure_within(Duration::ZERO).now_or_never() {
            self.note(reason.as_deref());
        }
    }
}

#[async_trait]
impl Connection for Watched {
    /// Opens a stream on the connection.
    async fn open(&self) -> Result<BoxDuplex, ConnectionError> {
        self.inner.open().await
    }

    /// When the box last sent data.
    fn last_inbound(&self) -> Instant {
        self.inner.last_inbound()
    }

    /// The connection's failure reason, recorded when it is a refusal.
    async fn failure_within(&self, limit: Duration) -> Option<String> {
        let reason = self.inner.failure_within(limit).await;
        self.note(reason.as_deref());
        reason
    }

    /// Looks for a refusal before closing: after this side's own close the
    /// box's reason could no longer be told apart.
    fn shutdown(&self) {
        self.note_if_closed();
        self.inner.shutdown();
    }

    /// Resolves once closed, after recording a refusal.
    async fn closed(&self) {
        self.inner.closed().await;
        let reason = self.inner.failure_within(Duration::ZERO).await;
        self.note(reason.as_deref());
    }
}

/// How long before a restored link started its box was last reached, from
/// the stored times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Unreached {
    /// The time between that and the start of the link.
    pub(super) away: Duration,
    /// It was the pairing, not a connection.
    pub(super) paired: bool,
}

impl Unreached {
    /// From the box's stored `last_connected` and `paired_at` at `now`,
    /// all Unix milliseconds: the later of the two, since a pairing made
    /// after the last connection reached the box too. A time ahead of the
    /// clock, after the clock stepped back, counts as now.
    pub(super) fn at(last_connected: Option<i64>, paired_at: i64, now: i64) -> Self {
        let (last, paired) = match last_connected {
            Some(connected) if connected > paired_at => (connected, false),
            _ => (paired_at, true),
        };
        let away = u64::try_from(now.saturating_sub(last)).unwrap_or(0);
        Self {
            away: Duration::from_millis(away),
            paired,
        }
    }

    /// Since when the box was not reached, when `more` passed after the
    /// link started: "since it was last connected 3 days ago".
    fn text(self, more: Duration) -> String {
        let what = if self.paired {
            "it was paired"
        } else {
            "it was last connected"
        };
        format!("since {what} {} ago", span(self.away + more))
    }
}

/// `duration` in words, rounded down to whole days from two days, hours
/// from one hour, or minutes.
fn span(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    let (hours, days) = (minutes / 60, minutes / (24 * 60));
    match (days, hours, minutes) {
        (2.., _, _) => format!("{days} days"),
        (_, 1, _) => "an hour".to_owned(),
        (_, 2.., _) => format!("{hours} hours"),
        (_, _, 0..=1) => "a minute".to_owned(),
        _ => format!("{minutes} minutes"),
    }
}

/// Bounds how long the link to a box paired by ticket keeps trying, around
/// its [`PeerTransport`].
///
/// A failed dial past the deadline, and a refused key at once, become
/// permanent [`PeerLinkError`]s, which end the link `failed`. The deadline
/// is on tokio's clock. The time a box was away before the daemon started
/// comes in through [`PeerAttempts::new`] from stored wall-clock times, and
/// never cuts the link's own attempts below [`RESTORE_GRACE`].
///
/// tokio's clock is std's `Instant`, which on Windows reads
/// `QueryPerformanceCounter` and, by every account of it, keeps counting
/// while the machine sleeps; std leaves it unspecified. So a gap between
/// two attempts much longer than the backoff allows ([`ASLEEP_AFTER`])
/// means the daemon was not running them, and the deadline moves to at
/// least [`RESTORE_GRACE`] after the attempt that follows: the first dial
/// after waking often fails before the network is back. Where the clock
/// stops during sleep no such gap shows, and nothing changes. A daemon that
/// restarts in a loop likewise grants a fresh grace at each start, so such
/// a box is retried for good and never forgotten, which errs on keeping a
/// pairing rather than deleting it.
pub(super) struct PeerAttempts {
    inner: Arc<dyn Transport>,
    name: String,
    ephemeral: bool,
    /// When the next failed dial ends the link; `None` while the last dial
    /// connected, so the box was reachable then.
    deadline: Mutex<Option<Instant>>,
    /// When the last attempt ended, on tokio's clock.
    last_attempt: Mutex<Option<Instant>>,
    /// For a restored link that has not connected yet: when it started, on
    /// tokio's clock and in Unix milliseconds, and how long its box had
    /// been away then, for the give-up text.
    restored: Mutex<Option<(Instant, i64, Unreached)>>,
    /// Set once the link stopped trying because the box stayed away.
    gave_up: Arc<AtomicBool>,
}

/// A gap between two attempts longer than this means the daemon was not
/// running them, as across a sleep: twice the longest wait
/// `Backoff::default()` (`devlanes_transport`'s `reconnect.rs`) puts
/// between attempts, 2 minutes, which the type does not expose.
const ASLEEP_AFTER: Duration = Duration::from_secs(2 * 2 * 60);

impl PeerAttempts {
    /// Tries box `name` through `inner` for [`RETRY_FOR`], or
    /// [`EPHEMERAL_RETRY_FOR`] when `ephemeral`. A link the restore opened
    /// (`restored`) gets what is left of that since the box was last
    /// reached, and at least [`RESTORE_GRACE`]; one a person started gets it
    /// all.
    pub(super) fn new(
        inner: Arc<dyn Transport>,
        name: &str,
        ephemeral: bool,
        restored: Option<Unreached>,
    ) -> Self {
        let window = Self::window(ephemeral);
        let now = Instant::now();
        let left = match restored {
            Some(unreached) => window.saturating_sub(unreached.away).max(RESTORE_GRACE),
            None => window,
        };
        Self {
            inner,
            name: name.to_owned(),
            ephemeral,
            deadline: Mutex::new(Some(now + left)),
            last_attempt: Mutex::new(None),
            restored: Mutex::new(restored.map(|unreached| {
                let wall = i64::try_from(super::now_ms()).unwrap_or(i64::MAX);
                (now, wall, unreached)
            })),
            gave_up: Arc::default(),
        }
    }

    /// How long a box may stay unreachable.
    fn window(ephemeral: bool) -> Duration {
        if ephemeral {
            EPHEMERAL_RETRY_FOR
        } else {
            RETRY_FOR
        }
    }

    /// Set once the link stopped trying because the box stayed away.
    pub(super) fn gave_up(&self) -> Arc<AtomicBool> {
        self.gave_up.clone()
    }
}

#[async_trait]
impl Transport for PeerAttempts {
    /// Dials through the inner transport and applies the bounds.
    async fn connect(&self) -> Result<BoxConnection, TransportError> {
        // From the end of the last attempt to the start of this one is only
        // the backoff, however long a dial takes.
        let started = Instant::now();
        let gap = lock(&self.last_attempt).map(|ended| started.saturating_duration_since(ended));
        let result = self.inner.connect().await;
        let now = Instant::now();
        *lock(&self.last_attempt) = Some(now);
        let error = match result {
            Ok(connection) => {
                *lock(&self.deadline) = None;
                *lock(&self.restored) = None;
                return Ok(connection);
            }
            Err(error) => error,
        };
        let name = self.name.clone();
        if matches!(error.downcast_ref(), Some(PeerError::Refused)) {
            return Err(TransportError::permanent(PeerLinkError::Refused { name }));
        }
        if error.is_permanent() {
            return Err(error);
        }
        let deadline = {
            let mut deadline = lock(&self.deadline);
            // The first failure after a connection starts the window: the
            // box was reached until about then.
            let at = deadline.get_or_insert(now + Self::window(self.ephemeral));
            if gap.is_some_and(|gap| gap > ASLEEP_AFTER) {
                *at = (*at).max(now + RESTORE_GRACE);
            }
            *at
        };
        if now < deadline {
            return Err(error);
        }
        self.gave_up.store(true, Ordering::SeqCst);
        let last = clean_reason(&error.to_string());
        tracing::info!(host = %self.name, error = %last, "stopped trying a box that stayed unreachable");
        let since = match *lock(&self.restored) {
            // The larger of the two clocks, so the text stays true where
            // tokio's stops while the machine sleeps.
            Some((started, wall, unreached)) => {
                let now_wall = i64::try_from(super::now_ms()).unwrap_or(i64::MAX);
                let walled = u64::try_from(now_wall.saturating_sub(wall)).unwrap_or(0);
                unreached.text((now - started).max(Duration::from_millis(walled)))
            }
            // At least the window passed since the first failure, also
            // after a gap that granted more.
            None => format!("for {}", span(Self::window(self.ephemeral))),
        };
        Err(TransportError::permanent(if self.ephemeral {
            PeerLinkError::GaveUpEphemeral { name, since }
        } else {
            PeerLinkError::GaveUp { name, since }
        }))
    }
}

impl Inner {
    /// The bounded transport of box `paired`: a link a person started gets
    /// the whole window, a link the restore opens (`restored`) what is left
    /// of it since the box was last reached ([`Unreached::at`]), at least
    /// [`RESTORE_GRACE`].
    pub(super) async fn peer_attempts(
        &self,
        paired: &P2pBox,
        restored: bool,
    ) -> Result<PeerAttempts, RpcError> {
        let Some(endpoint) = self.integ.peer.clone() else {
            return Err(RpcError::new(
                "not_supported_yet",
                json!({"feature": "boxes paired by ticket"}),
                1,
            ));
        };
        let Ok(id) = paired.peer_key.parse::<PeerId>() else {
            tracing::warn!(host = %paired.name, "the stored key of a box paired by ticket is damaged");
            return Err(super::pairing_missing(&paired.name));
        };
        let direct = paired
            .direct_addrs
            .iter()
            .filter_map(|addr| match addr.parse::<SocketAddr>() {
                Ok(addr) => Some(addr),
                Err(_) => {
                    tracing::debug!(host = %paired.name, %addr, "skipping a stored box address that does not parse");
                    None
                }
            })
            .collect();
        let addr = PeerAddr {
            id,
            relay: paired.relay_url.clone(),
            direct,
        };
        let unreached = if restored {
            let last = self
                .store
                .remote_hosts()
                .await
                .map_err(crate::registry::store_error)?
                .into_iter()
                .find(|host| host.name == paired.name && host.transport == super::P2P)
                .and_then(|host| host.last_connected);
            let now = i64::try_from(super::now_ms()).unwrap_or(i64::MAX);
            Some(Unreached::at(last, paired.paired_at, now))
        } else {
            None
        };
        let transport = Arc::new(PeerTransport::new(endpoint, addr));
        Ok(PeerAttempts::new(
            transport,
            &paired.name,
            paired.ephemeral,
            unreached,
        ))
    }
}

/// Longest box name this client keeps for a box paired by ticket, in bytes.
const MAX_NAME: usize = 63;

/// What a pairing dial the box refuses in the handshake means: it listens,
/// but takes no pairing because it has no ticket open, which its admission
/// says the same way for every cause.
const NO_TICKET_OPEN: &str = "the box has no pairing ticket open: the ticket expired, was already \
                              used, or the box's daemon restarted since it was printed. Get a new \
                              one with `devlanes daemon pair` on the box";

/// Longest wait before pairing again that a box may ask for, in
/// milliseconds; a longer one is passed on as this.
const MAX_RETRY_AFTER_MS: u64 = 24 * 60 * 60 * 1000;

/// Whether `name` starts with the ticket prefix, in any case, which the
/// CLI would take for a ticket.
fn looks_like_ticket(name: &str) -> bool {
    name.get(..TICKET_PREFIX.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(TICKET_PREFIX))
}

/// Whether `name` may be given for a box paired by ticket: what
/// `remote.boxes` allows (letters, digits, `-` and `_`), at most
/// [`MAX_NAME`] bytes, no leading `-`, which `remote connect` would read as
/// an option, and not the ticket prefix.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && !looks_like_ticket(name)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The name a box that calls itself `answered` gets here: lowercase, every
/// character but letters, digits, `-` and `_` turned into `-`, no `-` at
/// either end, `box-` before a name that starts like a ticket, cut so that
/// a `-9` suffix still fits in [`MAX_NAME`], and `box` when nothing is
/// left. The box chooses `answered`, so it is never shown or stored as sent.
fn derived_name(answered: &str) -> String {
    let mapped: String = answered
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '-' | '_') => c,
            _ => '-',
        })
        .skip_while(|c| *c == '-')
        .take(MAX_NAME)
        .collect();
    let mut name = if looks_like_ticket(&mapped) {
        format!("box-{mapped}")
    } else {
        mapped
    };
    // ASCII only by now, so any byte is a character boundary.
    name.truncate(MAX_NAME - 2);
    let name = name.trim_end_matches('-');
    if name.is_empty() {
        "box".to_owned()
    } else {
        name.to_owned()
    }
}

/// `bad_ticket`: text that is not a usable ticket.
fn bad_ticket(message: impl std::fmt::Display) -> RpcError {
    RpcError::new("bad_ticket", json!({"message": message.to_string()}), 2)
}

/// `pairing_refused`: the box answered the pairing with a refusal, whose
/// text says what to do, and how long to wait when it said so.
fn pairing_refused(message: &str, retry_after_ms: Option<u64>) -> RpcError {
    let retry_after_ms = retry_after_ms.map(|wait| wait.min(MAX_RETRY_AFTER_MS));
    RpcError::new(
        "pairing_refused",
        json!({"message": clean_reason(message), "retry_after_ms": retry_after_ms}),
        4,
    )
}

/// What already has a box name here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// Another box paired by ticket.
    Paired,
    /// A box reached over SSH or by a command, recorded or connecting.
    Reached,
    /// An entry of `remote.boxes` in the user config.
    Configured,
}

impl Holder {
    /// The `taken_by` field of `box_name_taken`.
    fn code(self) -> &'static str {
        match self {
            Self::Paired => "p2p",
            Self::Reached => "ssh_or_command",
            Self::Configured => "user_config",
        }
    }

    /// What has the name, after "taken by".
    fn text(self) -> &'static str {
        match self {
            Self::Paired => "another box paired by ticket",
            Self::Reached => "a box reached over SSH or by a command",
            Self::Configured => "an entry of remote.boxes in the user config",
        }
    }
}

/// `box_name_taken`: `holder` already has box name `name` here.
fn name_taken(name: &str, holder: Holder, message: String) -> RpcError {
    RpcError::new(
        "box_name_taken",
        json!({"name": name, "taken_by": holder.code(), "message": message}),
        1,
    )
}

/// A box [`Inner::pair_by_ticket`] paired with, and the connect lock it
/// still holds.
pub(super) struct Paired {
    /// The name the box is kept under.
    pub(super) name: String,
    /// The box was paired with before, so it kept that pairing's name.
    pub(super) kept: bool,
    /// [`Remotes::connecting`](super::Remotes), held from writing the row
    /// until the caller opened the link, so no forget or restore runs in
    /// between.
    pub(super) connecting: OwnedMutexGuard<()>,
}

/// The error of a pairing dial that failed, for the box called `label`
/// here. An unreachable box is most often one whose ticket expired, after
/// which it stops listening, so that text says a ticket expires too.
fn pairing_dial_failed(label: &str, error: &TransportError) -> RpcError {
    let reason = clean_reason(&error.to_string());
    match error.downcast_ref::<PeerError>() {
        Some(PeerError::InvalidId(_)) => bad_ticket(reason),
        Some(PeerError::Refused) => pairing_refused(NO_TICKET_OPEN, None),
        Some(PeerError::Unreachable(_)) => super::remote_failed(
            label,
            format!(
                "{reason}. Also check that the ticket was printed less than {} minutes ago.",
                PAIRING_CODE_TTL_MS / 60_000
            ),
        ),
        _ => super::remote_failed(label, reason),
    }
}

/// The `bad_request` for a name a box cannot be given.
fn bad_name(name: &str) -> RpcError {
    RpcError::new(
        "bad_request",
        json!({
            "message": format!(
                "a box name has at most {MAX_NAME} letters, digits, `-` and `_`, does not start \
                 with `-` and does not start with `{TICKET_PREFIX}`"
            ),
            "name": name,
        }),
        2,
    )
}

impl Inner {
    /// `connect` with a ticket: pairs with the box in `text` and stores it.
    /// A box paired with before keeps its name and ignores `given`; a new
    /// one is kept under `given` or its host name made safe, with `-2` to
    /// `-9` when that is taken, and a `given` name another box has fails
    /// before the ticket is used. The secret goes only to the ticket's key
    /// over the pairing ALPN, on a direct path, and is neither logged nor
    /// stored; a pairing that fails stores nothing. Text the box chose
    /// reaches the error messages cleaned.
    pub(super) async fn pair_by_ticket(
        self: &Arc<Self>,
        text: &str,
        given: Option<String>,
    ) -> Result<Paired, RpcError> {
        let ticket = Ticket::parse(text).map_err(bad_ticket)?;
        let Some(endpoint) = self.integ.peer.clone() else {
            return Err(RpcError::new(
                "not_supported_yet",
                json!({"feature": "pairing by ticket"}),
                1,
            ));
        };
        let key = ticket.addr.id.to_string();
        let stored = self.paired_key(&key).await?;
        // A box paired with before keeps its name, so a name given for it
        // is no reason to fail.
        let given = given.filter(|_| stored.is_none());
        if let Some(name) = &given {
            if !valid_name(name) {
                return Err(bad_name(name));
            }
            if let Some(holder) = self.name_holder(name, &key).await? {
                return Err(name_taken(
                    name,
                    holder,
                    format!(
                        "the box name {name} is taken by {}; choose another with `--name`",
                        holder.text()
                    ),
                ));
            }
        }
        let identity = self.client_identity().await?;
        // Named after the pairing's box until it has a name here.
        let label = stored
            .as_ref()
            .map(|stored| stored.name.clone())
            .or_else(|| given.clone())
            .unwrap_or_else(|| super::p2p_address(&key));
        let connection = match endpoint.connect(&ticket.addr, Alpn::Pair).await {
            Ok(connection) => connection,
            Err(error) => return Err(pairing_dial_failed(&label, &error)),
        };
        let request = PairRequest::new(
            &ticket.secret,
            identity.id,
            identity.name,
            format!("devlanes {}", devlanes_version::VERSION),
        );
        let answer = request_pairing(connection, request)
            .await
            .map_err(|error| {
                let reason = clean_reason(&error.to_string());
                super::remote_failed(&label, format!("pairing with the box failed: {reason}"))
            })?;
        let answered = match answer {
            PairAnswer::Paired { name, .. } => name,
            PairAnswer::Refused {
                message,
                retry_after_ms,
            } => return Err(pairing_refused(&message, retry_after_ms)),
        };
        // The box pinned this machine's key and used the ticket up, so the
        // rest runs in a task of its own: a request dropped from here on, as
        // by Ctrl-C while it waits for the lock, still keeps the pairing.
        let engine = self.clone();
        let (addr, ephemeral) = (ticket.addr.clone(), ticket.ephemeral);
        tokio::spawn(async move {
            engine
                .keep_pairing(key, addr, ephemeral, given, answered)
                .await
        })
        .await
        .map_err(|error| RpcError::new("internal", json!({"message": error.to_string()}), 1))?
    }

    /// [`Inner::pair_by_ticket`]'s work after the box paired with this
    /// machine: picks the name and stores the pairing with the box `key` at
    /// `addr`, `answered` being the name the box gave itself.
    async fn keep_pairing(
        self: Arc<Self>,
        key: String,
        addr: PeerAddr,
        ephemeral: bool,
        given: Option<String>,
        answered: String,
    ) -> Result<Paired, RpcError> {
        // Held until the caller opened the link: no other connect takes the
        // name between the check and the row, and no forget of an earlier
        // link deletes the row before the new link reads it.
        let connecting = self.remote.connecting.clone().lock_owned().await;
        // Looked up again under the lock: a pairing or a forget since the
        // first look decides which name rule applies.
        let stored = self.paired_key(&key).await?;
        let name = match (&stored, given) {
            (Some(stored), _) => stored.name.clone(),
            (None, Some(name)) => match self.name_holder(&name, &key).await? {
                None => name,
                Some(holder) => {
                    return Err(name_taken(
                        &name,
                        holder,
                        format!(
                            "the box name {name} was taken by {} while this machine paired with \
                             the box, which used the ticket. Get a new ticket with `devlanes \
                             daemon pair` on the box and choose another name with `--name`",
                            holder.text()
                        ),
                    ));
                }
            },
            (None, None) => self.free_name(&derived_name(&answered), &key).await?,
        };
        let paired = P2pBox {
            name,
            peer_key: key,
            relay_url: addr.relay.clone(),
            direct_addrs: addr.direct.iter().map(ToString::to_string).collect(),
            ephemeral,
            paired_at: i64::try_from(super::now_ms()).unwrap_or(i64::MAX),
        };
        let row = self
            .store
            .add_p2p_box(paired)
            .await
            .map_err(crate::registry::store_error)?;
        tracing::info!(host = %row.name, peer = %addr.id.short(), ephemeral = row.ephemeral, "paired with a box by ticket");
        Ok(Paired {
            name: row.name,
            kept: stored.is_some(),
            connecting,
        })
    }

    /// Whether `host` names a box paired by ticket, in any case.
    pub(super) async fn is_peer_box(&self, host: &str) -> Result<bool, RpcError> {
        Ok(self
            .store
            .p2p_boxes()
            .await
            .map_err(crate::registry::store_error)?
            .iter()
            .any(|paired| paired.name.eq_ignore_ascii_case(host)))
    }

    /// The stored spelling of the box paired by ticket that `host` names in
    /// another case, which must not become an SSH destination: on a file
    /// system that ignores case the two would share a sync directory.
    pub(super) async fn peer_box_spelled(&self, host: &str) -> Result<Option<String>, RpcError> {
        Ok(self
            .store
            .p2p_boxes()
            .await
            .map_err(crate::registry::store_error)?
            .into_iter()
            .find(|paired| paired.name != host && paired.name.eq_ignore_ascii_case(host))
            .map(|paired| paired.name))
    }

    /// The stored pairing with the box whose key is `key`.
    async fn paired_key(&self, key: &str) -> Result<Option<P2pBox>, RpcError> {
        self.store
            .p2p_box_by_key(key.to_owned())
            .await
            .map_err(crate::registry::store_error)
    }

    /// Forgets the ephemeral box paired by ticket under `name`, whose link
    /// with connect serial `serial`, for the pairing `pairing`, gave up:
    /// closes that link and deletes the box's `p2p` row in `remote_hosts`,
    /// its lane choices and that pairing's `p2p_boxes` row, and nothing
    /// else. A pairing made again since, or a newer link of the name, keeps
    /// everything: the connect lock, which pairing by ticket holds from its
    /// row to the new link, orders this against them. The box keeps its own
    /// record of this client until `daemon revoke` there.
    pub(super) async fn forget_peer_box(
        self: &Arc<Self>,
        name: &str,
        serial: u64,
        pairing: &super::PeerPairing,
    ) {
        if self.shutdown.is_cancelled() {
            return;
        }
        // Held throughout, so a connect or the restore never opens the
        // name halfway through.
        let connecting = self.remote.connecting.lock().await;
        let current = match self.store.p2p_box(name.to_owned()).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(%error, host = %name, "cannot read the pairing to forget");
                return;
            }
        };
        if !current
            .is_some_and(|row| row.peer_key == pairing.key && row.paired_at == pairing.paired_at)
        {
            tracing::debug!(host = %name, "not forgetting a box paired again since its link gave up");
            return;
        }
        let conn = {
            let mut hosts = lock(&self.remote.hosts);
            match hosts.get(name) {
                Some(conn) if conn.serial != serial => return,
                Some(_) => hosts.remove(name),
                None => None,
            }
        };
        if let Some(conn) = conn {
            conn.close().await;
            self.remote.wake_others(conn.serial);
        }
        // `remote_hosts` first, so a daemon that stops halfway never
        // restores a box whose pairing is gone, and `p2p_boxes` last, so
        // what is left is a pairing `remote ls` lists and that can be
        // connected or forgotten again.
        let deleted = self
            .store
            .delete_remote_host(name.to_owned(), super::P2P.to_owned())
            .await;
        if let Err(error) = deleted {
            tracing::warn!(%error, host = %name, "cannot forget the box's connection record");
        }
        self.forget_choices_of(name).await;
        let deleted = self
            .store
            .delete_p2p_pairing(name.to_owned(), pairing.key.clone(), pairing.paired_at)
            .await;
        if let Err(error) = deleted {
            tracing::warn!(%error, host = %name, "cannot forget the pairing with the box");
        }
        lock(&self.remote.restore_errors).remove(name);
        drop(connecting);
        tracing::info!(host = %name, "forgot an ephemeral box that stayed unreachable");
        self.bus.emit(
            devlanes_proto::remote::STATE_EVENT,
            json!({"host": name, "state": "stopped"}),
        );
        self.announce_findings();
        self.reconcile_box_ports().await;
        self.reassign();
        self.publish_routes();
    }

    /// `base`, or the first of `base-2` to `base-9` no other box here has,
    /// for the box with key `key`. When all are taken, `box_name_taken`
    /// with `taken_by` the holder of `base` itself, the name the box asked
    /// for.
    async fn free_name(&self, base: &str, key: &str) -> Result<String, RpcError> {
        let Some(holder) = self.name_holder(base, key).await? else {
            return Ok(base.to_owned());
        };
        for candidate in (2..=9).map(|n| format!("{base}-{n}")) {
            if self.name_holder(&candidate, key).await?.is_none() {
                return Ok(candidate);
            }
        }
        Err(name_taken(
            base,
            holder,
            format!(
                "this machine would keep the box as {base}, but {base} and {base}-2 to {base}-9 \
                 are all taken here, so it did not keep the pairing, and the ticket is used up. \
                 Get a new ticket with `devlanes daemon pair` on the box, then run `devlanes \
                 remote connect <ticket> --name NAME` with a free name."
            ),
        ))
    }

    /// What already has box name `name` here, in any case, other than the
    /// box with key `key`: another box paired by ticket, a box reached by
    /// SSH or a command, or an entry of `remote.boxes` in the user config.
    /// `None` when the name is free. Case is ignored because `Acme` and
    /// `acme` share a sync directory on a file system that ignores case.
    async fn name_holder(&self, name: &str, key: &str) -> Result<Option<Holder>, RpcError> {
        let same = |other: &str| other.eq_ignore_ascii_case(name);
        let paired = self
            .store
            .p2p_boxes()
            .await
            .map_err(crate::registry::store_error)?;
        if paired
            .iter()
            .any(|paired| same(&paired.name) && paired.peer_key != key)
        {
            return Ok(Some(Holder::Paired));
        }
        let recorded = self
            .store
            .remote_hosts()
            .await
            .map_err(crate::registry::store_error)?
            .into_iter()
            .any(|host| same(&host.name) && host.transport != super::P2P);
        let live = lock(&self.remote.hosts)
            .iter()
            .any(|(other, conn)| same(other) && conn.kind != super::P2P);
        if recorded || live {
            return Ok(Some(Holder::Reached));
        }
        Ok(self
            .configured_box(name)
            .await
            .then_some(Holder::Configured))
    }

    /// Whether `remote.boxes` in the user config has an entry named `name`,
    /// in any case. Read on a blocking thread, leniently, so a problem
    /// elsewhere in the file hides no entry.
    pub(super) async fn configured_box(&self, name: &str) -> bool {
        let home = self.opts.home.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            let path = home.join("config.yaml");
            path.exists()
                && devlanes_config::load_user_lenient(&path)
                    .0
                    .remote
                    .boxes
                    .keys()
                    .any(|entry| entry.eq_ignore_ascii_case(&name))
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use devlanes_transport::{
        Admission, PeerId,
        memory::{MemoryPeers, MemoryTransport, memory_transport},
    };

    use super::*;

    /// The name a box gives itself becomes one `remote connect` takes back:
    /// no control characters, separators or option-like start, and short
    /// enough for a suffix.
    #[test]
    fn a_box_name_is_made_safe() {
        let long = "x".repeat(10_000);
        let rows = [
            ("Acme-Box_2", "acme-box_2"),
            ("acme.local", "acme-local"),
            ("--acme", "acme"),
            ("\u{1b}[31macme\n", "31macme"),
            ("büro", "b-ro"),
            ("../..", "box"),
            ("", "box"),
            ("DLP1-desk", "box-dlp1-desk"),
            (long.as_str(), &long[..61]),
        ];
        for (answered, expected) in rows {
            let name = derived_name(answered);
            assert_eq!(name, expected, "{answered:?}");
            assert!(valid_name(&format!("{name}-9")), "{name}");
        }
        // A given name is held to the same rules.
        for given in ["dlp1desk", "Dlp1", "-acme", "acme box", ""] {
            assert!(!valid_name(given), "{given:?}");
        }
    }

    /// Lets every peer in, for both ALPNs.
    struct AdmitAll;

    impl Admission for AdmitAll {
        /// Always true.
        fn admit(&self, _: &PeerId, _: Alpn) -> bool {
            true
        }
    }

    /// How a test session ends.
    #[derive(Debug, Clone, Copy)]
    enum Ending {
        /// The session asked why the connection failed.
        AskedWhy,
        /// The session waited for the close.
        AwaitedClose,
        /// The session only closed its side, as after a failed ping write.
        ShutDown,
    }

    /// A box that refused this machine's key and stopped listening fails
    /// the next dial as refused, for good, whichever way the session saw
    /// the close; a box that only stopped listening is unreachable.
    #[tokio::test]
    async fn a_refusal_on_the_last_connection_refuses_the_next_dial() {
        let rows = [
            (Ending::AskedWhy, true, true),
            (Ending::AwaitedClose, true, true),
            (Ending::ShutDown, true, true),
            (Ending::ShutDown, false, false),
        ];
        for (ending, revoked, refused) in rows {
            let peers = MemoryPeers::new();
            let (listener, dialer) = (peers.endpoint(), peers.endpoint());
            listener
                .listen(Some(Arc::new(AdmitAll)))
                .await
                .expect("listen");
            let addr = listener.addr().await.expect("addr");
            let transport = PeerTransport::new(dialer.clone(), addr);
            let connection = transport.connect().await.expect("connect");
            if revoked {
                listener.close_peer(&dialer.id().await.expect("id"));
            }
            listener.listen(None).await.expect("stop listening");
            let closed = tokio::time::timeout(Duration::from_secs(5), async {
                match ending {
                    Ending::AskedWhy => {
                        connection.failure_within(Duration::from_secs(5)).await;
                    }
                    Ending::AwaitedClose => connection.closed().await,
                    // Polls the connection through a call that records
                    // nothing until it is gone, then only shuts it down.
                    Ending::ShutDown => {
                        if revoked {
                            while connection.open().await.is_ok() {
                                tokio::task::yield_now().await;
                            }
                        }
                        connection.shutdown();
                    }
                }
            })
            .await;
            assert!(closed.is_ok(), "{ending:?}: the connection never closed");
            let Err(error) = transport.connect().await else {
                panic!("{ending:?}: dialed a box that stopped listening");
            };
            let was_refused = matches!(error.downcast_ref(), Some(PeerError::Refused));
            assert_eq!(was_refused, refused, "{ending:?} {revoked}: {error}");
            assert_eq!(
                error.is_permanent(),
                refused,
                "{ending:?} {revoked}: {error}"
            );
        }
    }

    /// What a scripted dial does: connect for `None`, else fail with the
    /// error the function makes.
    type Outcome = Option<fn() -> TransportError>;

    /// A transport that answers each dial with the next queued outcome,
    /// and once they ran out as an unreachable box.
    struct Scripted {
        outcomes: Mutex<VecDeque<Outcome>>,
        memory: MemoryTransport,
    }

    #[async_trait]
    impl Transport for Scripted {
        /// The next outcome.
        async fn connect(&self) -> Result<BoxConnection, TransportError> {
            let outcome = lock(&self.outcomes)
                .pop_front()
                .unwrap_or(Some(unreachable));
            match outcome {
                Some(error) => Err(error()),
                None => self.memory.connect().await,
            }
        }
    }

    /// Dials `attempts` every 2 minutes, as the longest backoff does, until
    /// a dial ends the link or `limit` passed; how long that took, and the
    /// error that ended it.
    async fn dial_until_stopped(
        attempts: &PeerAttempts,
        limit: Duration,
    ) -> (Duration, Option<TransportError>) {
        let step = Duration::from_secs(2 * 60);
        let mut waited = Duration::ZERO;
        while waited <= limit {
            match attempts.connect().await {
                Err(error) if error.is_permanent() => return (waited, Some(error)),
                _ => {}
            }
            tokio::time::advance(step).await;
            waited += step;
        }
        (waited, None)
    }

    /// A dial that finds no listener.
    fn unreachable() -> TransportError {
        PeerError::Unreachable("no endpoint with that id is listening".into()).into()
    }

    /// A dial the box refused.
    fn refused() -> TransportError {
        PeerError::Refused.into()
    }

    /// What a dial in [`attempts_give_up_once_the_box_stayed_away_too_long`]
    /// ends in.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ends {
        /// A connection.
        Connected,
        /// A failure that is retried.
        Retried,
        /// A failure that ends the link with this text, and whether it set
        /// `gave_up`.
        Stopped(&'static str, bool),
    }

    /// Failures stay transient until the window since the box was last
    /// reached is over, and the first one after it is permanent with the
    /// give-up text saying since when and sets `gave_up`. A restored link
    /// tries for at least `RESTORE_GRACE` of its own, so a machine that was
    /// off for a weekend connects to a box that is back and gives up only
    /// after the grace on one that is gone. A refused key is permanent at
    /// once and gives up nothing.
    #[tokio::test(start_paused = true)]
    async fn attempts_give_up_once_the_box_stayed_away_too_long() {
        let hour = Duration::from_secs(60 * 60);
        let second = Duration::from_secs(1);
        let weekend = 60 * hour;
        let connected = |away| {
            Some(Unreached {
                away,
                paired: false,
            })
        };
        let paired = |away| Some(Unreached { away, paired: true });
        let fails = Some(unreachable as fn() -> TransportError);
        type Row = (bool, Option<Unreached>, Duration, Outcome, Ends);
        let rows: [Row; 12] = [
            // (ephemeral, restored, then waited, dial, ends)
            (false, None, 23 * hour, fails, Ends::Retried),
            (
                false,
                None,
                24 * hour,
                fails,
                Ends::Stopped("for 24 hours", true),
            ),
            (
                true,
                None,
                hour,
                fails,
                Ends::Stopped(
                    "for an hour, so devlanes stopped trying and forgot it",
                    true,
                ),
            ),
            (
                false,
                connected(23 * hour),
                hour - second,
                fails,
                Ends::Retried,
            ),
            (
                false,
                connected(23 * hour),
                hour,
                fails,
                Ends::Stopped("since it was last connected 24 hours ago", true),
            ),
            (
                false,
                connected(weekend),
                Duration::ZERO,
                None,
                Ends::Connected,
            ),
            (
                false,
                connected(weekend),
                Duration::ZERO,
                fails,
                Ends::Retried,
            ),
            (
                false,
                connected(weekend),
                RESTORE_GRACE - second,
                fails,
                Ends::Retried,
            ),
            (
                false,
                connected(weekend),
                RESTORE_GRACE,
                fails,
                Ends::Stopped("since it was last connected 2 days ago", true),
            ),
            (
                true,
                paired(2 * hour),
                RESTORE_GRACE - second,
                fails,
                Ends::Retried,
            ),
            (
                true,
                paired(2 * hour),
                RESTORE_GRACE,
                fails,
                Ends::Stopped(
                    "since it was paired 2 hours ago, so devlanes stopped trying and forgot it",
                    true,
                ),
            ),
            (
                false,
                None,
                Duration::ZERO,
                Some(refused),
                Ends::Stopped("no longer accepts", false),
            ),
        ];
        let (memory, _listener) = memory_transport();
        for (ephemeral, restored, waited, dial, ends) in rows {
            let inner = Arc::new(Scripted {
                outcomes: Mutex::new(VecDeque::from([dial])),
                memory: memory.clone(),
            });
            let attempts = PeerAttempts::new(inner, "acme", ephemeral, restored);
            tokio::time::advance(waited).await;
            let row = format!("{ephemeral} {restored:?} {waited:?}");
            let gave_up = || attempts.gave_up().load(Ordering::SeqCst);
            match (attempts.connect().await, ends) {
                (Ok(_), Ends::Connected) => assert!(!gave_up(), "{row}"),
                (Err(error), Ends::Retried) => {
                    assert!(!error.is_permanent() && !gave_up(), "{row}: {error}");
                }
                (Err(error), Ends::Stopped(text, stopped)) => {
                    assert!(error.is_permanent(), "{row}: {error}");
                    assert!(error.to_string().contains(text), "{row}: {error}");
                    assert_eq!(gave_up(), stopped, "{row}: {error}");
                }
                (Ok(_), _) => panic!("{row}: connected"),
                (Err(error), Ends::Connected) => panic!("{row}: {error}"),
            }
        }
    }

    /// "Last reached" is the later of the last connection and the pairing,
    /// and a stored time ahead of the clock counts as now.
    #[test]
    fn a_restored_box_counts_from_when_it_was_last_reached() {
        let hour = 60 * 60 * 1000;
        let now = 100 * hour;
        let rows = [
            // (last connected, paired at, away in hours, it was the pairing)
            (Some(now - 23 * hour), now - 30 * hour, 23, false),
            (None, now - 30 * hour, 30, true),
            (Some(now - 30 * hour), now - 2 * hour, 2, true),
            (Some(now + hour), now - 30 * hour, 0, false),
        ];
        for (last, paired_at, hours, paired) in rows {
            let unreached = Unreached::at(last, paired_at, now);
            let expected = Unreached {
                away: Duration::from_secs(hours * 60 * 60),
                paired,
            };
            assert_eq!(unreached, expected, "{last:?} {paired_at}");
        }
    }

    /// A connection moves the deadline: a box reached 23 hours into its
    /// window gets a full day from its first failure after that.
    #[tokio::test(start_paused = true)]
    async fn a_connection_restarts_the_window() {
        let hour = Duration::from_secs(60 * 60);
        let (memory, _listener) = memory_transport();
        let inner = Arc::new(Scripted {
            outcomes: Mutex::new(VecDeque::from([None])),
            memory,
        });
        let restored = Unreached {
            away: 23 * hour,
            paired: false,
        };
        let attempts = PeerAttempts::new(inner, "acme", false, Some(restored));
        assert!(attempts.connect().await.is_ok(), "the box answered");
        tokio::time::advance(2 * hour).await;
        let (after, error) = dial_until_stopped(&attempts, 25 * hour).await;
        let error = error.expect("the link stops");
        assert_eq!(after, 24 * hour, "{error}");
        assert!(error.to_string().contains("for 24 hours"), "{error}");
    }

    /// Hours without a dial, as across a sleep, give the link the grace
    /// from the next failed dial instead of ending it there, and the
    /// give-up after the grace still says how long the box was away.
    #[tokio::test(start_paused = true)]
    async fn a_gap_without_attempts_grants_the_grace() {
        let (memory, _listener) = memory_transport();
        let inner = Arc::new(Scripted {
            outcomes: Mutex::new(VecDeque::new()),
            memory,
        });
        let attempts = PeerAttempts::new(inner, "acme", true, None);
        let first = attempts.connect().await.err().expect("unreachable");
        assert!(!first.is_permanent(), "{first}");
        tokio::time::advance(Duration::from_secs(3 * 60 * 60)).await;
        let (after, error) = dial_until_stopped(&attempts, 2 * RESTORE_GRACE).await;
        let error = error.expect("the link stops after the grace");
        assert_eq!(after, RESTORE_GRACE, "{error}");
        assert!(
            error
                .to_string()
                .contains("for an hour, so devlanes stopped trying and forgot it"),
            "{error}"
        );
        assert!(attempts.gave_up().load(Ordering::SeqCst));
    }

    /// `span` rounds down at its boundaries.
    #[test]
    fn spans_round_down_to_minutes_hours_and_days() {
        let minute = Duration::from_secs(60);
        let rows = [
            (59 * minute, "59 minutes"),
            (119 * minute, "an hour"),
            (47 * 60 * minute, "47 hours"),
        ];
        for (duration, text) in rows {
            assert_eq!(span(duration), text, "{duration:?}");
        }
    }
}
