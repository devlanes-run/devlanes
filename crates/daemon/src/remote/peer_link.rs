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
use devlanes_proto::RpcError;
use devlanes_state::P2pBox;
use devlanes_transport::{
    Alpn, BoxConnection, BoxDuplex, Connection, ConnectionError, PairAnswer, PairRequest, PeerAddr,
    PeerEndpoint, PeerError, PeerId, Ticket, Transport, TransportError, request_pairing,
};
use futures_util::FutureExt as _;
use serde_json::json;
use tokio::time::Instant;

use super::feed::clean;
use crate::{engine::Inner, model::lock};

/// How long a box paired by ticket may stay unreachable before its link
/// stops trying.
pub(super) const RETRY_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// [`RETRY_FOR`] for a box paired with an ephemeral ticket, which is then
/// forgotten.
pub(super) const EPHEMERAL_RETRY_FOR: Duration = Duration::from_secs(60 * 60);

/// Why a link to a box paired by ticket stopped for good. Not a
/// [`PeerError`]: their texts are about one dial, and wrapping these in one
/// would say twice that the box is not reachable.
#[derive(Debug, thiserror::Error)]
pub(super) enum PeerLinkError {
    /// The box was unreachable for [`RETRY_FOR`].
    #[error(
        "box {name} has not been reachable for 24 hours, so devlanes stopped trying. Run \
         `devlanes remote connect {name}` once it is back."
    )]
    GaveUp {
        /// The box's name here.
        name: String,
    },
    /// An ephemeral box was unreachable for [`EPHEMERAL_RETRY_FOR`], so it
    /// is forgotten.
    #[error(
        "box {name} has not been reachable for an hour, so devlanes stopped trying and forgot \
         it. Pair with it again using a new ticket from `devlanes daemon pair` on the box."
    )]
    GaveUpEphemeral {
        /// The box's name here.
        name: String,
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

/// Bounds how long the link to a box paired by ticket keeps trying, around
/// its [`PeerTransport`].
///
/// A failed dial past the deadline, and a refused key at once, become
/// permanent [`PeerLinkError`]s, which end the link `failed`. The deadline
/// is on tokio's clock: it only measures time this daemon ran, and the
/// time a box was away before the daemon started comes in through
/// [`PeerAttempts::new`], from stored wall-clock times.
pub(super) struct PeerAttempts {
    inner: Arc<dyn Transport>,
    name: String,
    ephemeral: bool,
    /// When the next failed dial ends the link; `None` while the last dial
    /// connected, so the box was reachable then.
    deadline: Mutex<Option<Instant>>,
    /// Set once the link stopped trying because the box stayed away.
    gave_up: Arc<AtomicBool>,
}

impl PeerAttempts {
    /// Tries box `name` through `inner` for [`RETRY_FOR`], or
    /// [`EPHEMERAL_RETRY_FOR`] when `ephemeral`, less `away`: how long the
    /// box had not been reached when the link started (zero for a link a
    /// person started).
    pub(super) fn new(
        inner: Arc<dyn Transport>,
        name: &str,
        ephemeral: bool,
        away: Duration,
    ) -> Self {
        let window = Self::window(ephemeral);
        Self {
            inner,
            name: name.to_owned(),
            ephemeral,
            deadline: Mutex::new(Some(Instant::now() + window.saturating_sub(away))),
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
        let error = match self.inner.connect().await {
            Ok(connection) => {
                *lock(&self.deadline) = None;
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
        let now = Instant::now();
        // The first failure after a connection starts the window: the box
        // was reached until about then.
        let deadline = *lock(&self.deadline).get_or_insert(now + Self::window(self.ephemeral));
        if now < deadline {
            return Err(error);
        }
        self.gave_up.store(true, Ordering::SeqCst);
        tracing::info!(host = %self.name, %error, "stopped trying a box that stayed unreachable");
        Err(TransportError::permanent(if self.ephemeral {
            PeerLinkError::GaveUpEphemeral { name }
        } else {
            PeerLinkError::GaveUp { name }
        }))
    }
}

impl Inner {
    /// The bounded transport of box `paired`: a link a person started gets
    /// the whole window, a link the restore opens (`connected_before`) what
    /// is left of it since the box was last reached, from
    /// `remote_hosts.last_connected`, or the pairing when it never connected.
    pub(super) async fn peer_attempts(
        &self,
        paired: &P2pBox,
        connected_before: bool,
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
        let away = if connected_before {
            let last = self
                .store
                .remote_hosts()
                .await
                .map_err(crate::registry::store_error)?
                .into_iter()
                .find(|host| host.name == paired.name)
                .and_then(|host| host.last_connected)
                .unwrap_or(paired.paired_at);
            // Stored times are wall-clock milliseconds; a time ahead of the
            // clock, after the clock stepped back, counts as just now.
            let now = i64::try_from(super::now_ms()).unwrap_or(i64::MAX);
            Duration::from_millis(u64::try_from(now.saturating_sub(last)).unwrap_or(0))
        } else {
            Duration::ZERO
        };
        let transport = Arc::new(PeerTransport::new(endpoint, addr));
        Ok(PeerAttempts::new(
            transport,
            &paired.name,
            paired.ephemeral,
            away,
        ))
    }
}

/// Longest box name this client keeps for a box paired by ticket, in bytes.
const MAX_NAME: usize = 63;

/// What a refused pairing dial means: the box listens, but for sessions
/// of keys it paired only, since no ticket is outstanding there.
const NO_TICKET_OUT: &str = "the box takes no pairing ticket now: the ticket expired or was \
                             already used. Get a new one with `devlanes daemon pair` on the box";

/// Whether `name` may be given for a box paired by ticket: what
/// `remote.boxes` allows (letters, digits, `-` and `_`), at most
/// [`MAX_NAME`] bytes, and no leading `-`, which `remote connect` would
/// read as an option.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The name a box that calls itself `answered` gets here: lowercase, every
/// character but letters, digits, `-` and `_` turned into `-`, no leading
/// `-`, cut so that a `-9` suffix still fits in [`MAX_NAME`], and `box`
/// when nothing is left. The box chooses `answered`, so it is never shown
/// or stored as sent.
fn derived_name(answered: &str) -> String {
    let mut name: String = answered
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '-' | '_') => c,
            _ => '-',
        })
        .skip_while(|c| *c == '-')
        .take(MAX_NAME - 2)
        .collect();
    if name.is_empty() {
        name.push_str("box");
    }
    name
}

/// `bad_ticket`: text that is not a usable ticket.
fn bad_ticket(message: impl std::fmt::Display) -> RpcError {
    RpcError::new("bad_ticket", json!({"message": message.to_string()}), 2)
}

/// `pairing_refused`: the box answered the pairing with a refusal, whose
/// text says what to do, and how long to wait when it said so.
fn pairing_refused(message: &str, retry_after_ms: Option<u64>) -> RpcError {
    RpcError::new(
        "pairing_refused",
        json!({"message": clean(message), "retry_after_ms": retry_after_ms}),
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

impl Inner {
    /// `connect` with a ticket: pairs with the box in `text` and stores it,
    /// then returns the name it is kept under, which the caller connects.
    /// The name is `given` or the box's host name made safe, with `-2` to
    /// `-9` when that is taken; a box this client already paired with
    /// keeps its name. A `given` name another box has fails before the
    /// ticket is used. The secret goes only to the ticket's key over the
    /// pairing ALPN, on a direct path, and is neither logged nor stored; a
    /// pairing that fails stores nothing.
    pub(super) async fn pair_by_ticket(
        self: &Arc<Self>,
        text: &str,
        given: Option<String>,
    ) -> Result<String, RpcError> {
        let ticket = Ticket::parse(text).map_err(bad_ticket)?;
        let Some(endpoint) = self.integ.peer.clone() else {
            return Err(RpcError::new(
                "not_supported_yet",
                json!({"feature": "pairing by ticket"}),
                1,
            ));
        };
        let key = ticket.addr.id.to_string();
        if let Some(name) = &given {
            if !valid_name(name) {
                return Err(RpcError::new(
                    "bad_request",
                    json!({
                        "message": format!(
                            "a box name has at most {MAX_NAME} letters, digits, `-` and `_`, and \
                             does not start with `-`"
                        ),
                        "name": name,
                    }),
                    2,
                ));
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
        let label = given.clone().unwrap_or_else(|| super::p2p_address(&key));
        let connection = match endpoint.connect(&ticket.addr, Alpn::Pair).await {
            Ok(connection) => connection,
            Err(error) => {
                return Err(match error.downcast_ref::<PeerError>() {
                    Some(PeerError::InvalidId(_)) => bad_ticket(&error),
                    Some(PeerError::Refused) => pairing_refused(NO_TICKET_OUT, None),
                    _ => super::remote_failed(&label, &error),
                });
            }
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
                super::remote_failed(&label, format!("pairing with the box failed: {error}"))
            })?;
        let answered = match answer {
            PairAnswer::Paired { name, .. } => name,
            PairAnswer::Refused {
                message,
                retry_after_ms,
            } => return Err(pairing_refused(&message, retry_after_ms)),
        };
        // Under the connect lock, so no other connect takes the name
        // between the check and the row.
        let _connecting = self.remote.connecting.lock().await;
        let name = match given {
            Some(name) => match self.name_holder(&name, &key).await? {
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
            None => self.free_name(&derived_name(&answered), &key).await?,
        };
        let paired = P2pBox {
            name,
            peer_key: key,
            relay_url: ticket.addr.relay.clone(),
            direct_addrs: ticket.addr.direct.iter().map(ToString::to_string).collect(),
            ephemeral: ticket.ephemeral,
            paired_at: i64::try_from(super::now_ms()).unwrap_or(i64::MAX),
        };
        let name = self
            .store
            .add_p2p_box(paired)
            .await
            .map_err(crate::registry::store_error)?;
        tracing::info!(host = %name, peer = %ticket.addr.id.short(), ephemeral = ticket.ephemeral, "paired with a box by ticket");
        Ok(name)
    }

    /// Forgets the box paired by ticket under `name`, whose link with
    /// connect serial `serial` gave up: closes that link and deletes the
    /// box's `remote_hosts` row, its lane choices and its `p2p_boxes` row,
    /// and nothing else. A newer link of the name, which `remote connect`
    /// opened meanwhile, keeps everything. The box keeps its own record of
    /// this client until `daemon revoke` there.
    pub(super) async fn forget_peer_box(self: &Arc<Self>, name: &str, serial: u64) {
        if self.shutdown.is_cancelled() {
            return;
        }
        // Held throughout, so a connect or the restore never opens the
        // name halfway through.
        let connecting = self.remote.connecting.lock().await;
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
        if let Err(error) = self.store.delete_remote_host(name.to_owned()).await {
            tracing::warn!(%error, host = %name, "cannot forget the box's connection record");
        }
        self.forget_choices_of(name).await;
        if let Err(error) = self.store.delete_p2p_box(name.to_owned()).await {
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
    /// for the box with key `key`; `box_name_taken` when all are taken.
    async fn free_name(&self, base: &str, key: &str) -> Result<String, RpcError> {
        let mut holder = Holder::Paired;
        for candidate in
            std::iter::once(base.to_owned()).chain((2..=9).map(|n| format!("{base}-{n}")))
        {
            match self.name_holder(&candidate, key).await? {
                None => return Ok(candidate),
                Some(taken) => holder = taken,
            }
        }
        Err(name_taken(
            base,
            holder,
            format!(
                "the box is called {base}, and {base} and {base}-2 to {base}-9 are taken here, so \
                 this machine kept nothing of the pairing, which used the ticket. Get a new ticket \
                 with `devlanes daemon pair` on the box and choose a name with `--name`"
            ),
        ))
    }

    /// What already has box name `name` here, other than the box with key
    /// `key`: another box paired by ticket, a box reached by SSH or a
    /// command, or an entry of `remote.boxes` in the user config. `None`
    /// when the name is free.
    async fn name_holder(&self, name: &str, key: &str) -> Result<Option<Holder>, RpcError> {
        let paired = self
            .store
            .p2p_box(name.to_owned())
            .await
            .map_err(crate::registry::store_error)?;
        if let Some(paired) = paired {
            return Ok((paired.peer_key != key).then_some(Holder::Paired));
        }
        let recorded = self
            .store
            .remote_hosts()
            .await
            .map_err(crate::registry::store_error)?
            .into_iter()
            .any(|host| host.name == name && host.transport != super::P2P);
        let live = lock(&self.remote.hosts)
            .get(name)
            .is_some_and(|conn| conn.kind != super::P2P);
        if recorded || live {
            return Ok(Some(Holder::Reached));
        }
        let home = self.opts.home.clone();
        let configured = tokio::task::spawn_blocking({
            let name = name.to_owned();
            move || {
                let path = home.join("config.yaml");
                path.exists()
                    && devlanes_config::load_user_lenient(&path)
                        .0
                        .remote
                        .boxes
                        .contains_key(&name)
            }
        })
        .await
        .unwrap_or(false);
        Ok(configured.then_some(Holder::Configured))
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
            ("\u{1b}[31macme\n", "31macme-"),
            ("büro", "b-ro"),
            ("../..", "box"),
            ("", "box"),
            (long.as_str(), &long[..61]),
        ];
        for (answered, expected) in rows {
            let name = derived_name(answered);
            assert_eq!(name, expected, "{answered:?}");
            assert!(valid_name(&format!("{name}-9")), "{name}");
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

    /// A transport that answers each dial with the next queued outcome.
    struct Scripted {
        outcomes: Mutex<VecDeque<Outcome>>,
        memory: MemoryTransport,
    }

    #[async_trait]
    impl Transport for Scripted {
        /// The next outcome.
        async fn connect(&self) -> Result<BoxConnection, TransportError> {
            let outcome = lock(&self.outcomes).pop_front().flatten();
            match outcome {
                Some(error) => Err(error()),
                None => self.memory.connect().await,
            }
        }
    }

    /// A dial that finds no listener.
    fn unreachable() -> TransportError {
        PeerError::Unreachable("no endpoint with that id is listening".into()).into()
    }

    /// A dial the box refused.
    fn refused() -> TransportError {
        PeerError::Refused.into()
    }

    /// Failures stay transient until the window since the box was last
    /// reached is over, and the first one after it is permanent with the
    /// give-up text and sets `gave_up`; a refused key is permanent at once
    /// and gives up nothing.
    #[tokio::test(start_paused = true)]
    async fn attempts_give_up_once_the_box_stayed_away_too_long() {
        let hour = Duration::from_secs(60 * 60);
        let second = Duration::from_secs(1);
        type Row = (
            bool,
            Duration,
            Duration,
            fn() -> TransportError,
            Option<&'static str>,
            bool,
        );
        let rows: [Row; 6] = [
            // (ephemeral, away at start, then waited, dial, permanent text, gave up)
            (false, Duration::ZERO, 23 * hour, unreachable, None, false),
            (false, 23 * hour, hour - second, unreachable, None, false),
            (
                false,
                23 * hour,
                hour,
                unreachable,
                Some("for 24 hours"),
                true,
            ),
            (
                false,
                25 * hour,
                Duration::ZERO,
                unreachable,
                Some("for 24 hours"),
                true,
            ),
            (
                true,
                2 * hour,
                Duration::ZERO,
                unreachable,
                Some("forgot it"),
                true,
            ),
            (
                false,
                Duration::ZERO,
                Duration::ZERO,
                refused,
                Some("no longer accepts"),
                false,
            ),
        ];
        let (memory, _listener) = memory_transport();
        for (ephemeral, away, waited, dial, permanent, gave_up) in rows {
            let inner = Arc::new(Scripted {
                outcomes: Mutex::new(VecDeque::from([Some(dial)])),
                memory: memory.clone(),
            });
            let attempts = PeerAttempts::new(inner, "acme", ephemeral, away);
            tokio::time::advance(waited).await;
            let Err(error) = attempts.connect().await else {
                panic!("connected");
            };
            let row = format!("{ephemeral} {away:?} {waited:?}: {error}");
            assert_eq!(error.is_permanent(), permanent.is_some(), "{row}");
            if let Some(text) = permanent {
                assert!(error.to_string().contains(text), "{row}");
            }
            assert_eq!(attempts.gave_up().load(Ordering::SeqCst), gave_up, "{row}");
        }
    }

    /// A connection moves the deadline: a box reached 23 hours into its
    /// window gets a full day from its first failure after that.
    #[tokio::test(start_paused = true)]
    async fn a_connection_restarts_the_window() {
        let hour = Duration::from_secs(60 * 60);
        let (memory, _listener) = memory_transport();
        let inner = Arc::new(Scripted {
            outcomes: Mutex::new(VecDeque::from([
                None,
                Some(unreachable as fn() -> TransportError),
                Some(unreachable),
                Some(unreachable),
            ])),
            memory,
        });
        let attempts = PeerAttempts::new(inner, "acme", false, 23 * hour);
        assert!(attempts.connect().await.is_ok(), "the box answered");
        tokio::time::advance(2 * hour).await;
        let first = attempts.connect().await.err().expect("unreachable");
        assert!(!first.is_permanent(), "{first}");
        tokio::time::advance(23 * hour).await;
        let later = attempts.connect().await.err().expect("unreachable");
        assert!(
            !later.is_permanent(),
            "23 hours after the first failure: {later}"
        );
        tokio::time::advance(hour).await;
        let last = attempts.connect().await.err().expect("unreachable");
        assert!(last.is_permanent(), "a day after the first failure: {last}");
    }
}
