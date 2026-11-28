//! Pairing. On a box: one-time codes for `devlanes daemon pair`, kept in
//! memory with the failed attempts that limit them, the remote tokens they
//! are traded for (only their SHA-256 is stored, in `remote_pairings`), and
//! `daemon clients` and `daemon revoke`. On a client: the token each box gave
//! this machine (`remote_tokens`) and the code a person types.
//!
//! Codes are 8 characters of Crockford's base32 alphabet (no I, L, O or U),
//! shown as `XXXX-XXXX`: 40 bits, valid once for 10 minutes. Every failed
//! attempt voids every outstanding code, so a guesser gets one try per code,
//! and five failures within 10 minutes refuse every code, and every new one,
//! until the oldest of them is 10 minutes old. Client ids and tokens are a
//! prefix and 256 random bits as lowercase hex. Codes and token hashes are
//! compared in constant time. A client keeps one pairing per box name it
//! connects by, so the box keeps up to [`MAX_PAIRINGS`] per client.

use std::{
    collections::VecDeque,
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use devlanes_proto::{
    RpcError,
    remote::{
        CLIENT_ID_PREFIX, CLIENTS_EVENT, ClaimParams, PAIRING_CODE_TTL_MS, RemoteClient,
        TOKEN_PREFIX, VerifyParams, is_random_id,
    },
};
use devlanes_state::{P2pClient, RemotePairing, Store};
use devlanes_transport::{BoxError, PairingClient, PairingReason, Ticket, TicketSecret};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use super::{ClientKey, feed::clean, now_ms};
use crate::{
    engine::{Inner, params, required},
    integrations::{RemotePrompt, RemotePrompter},
    model::lock,
    registry::store_error,
};

/// Characters a code is made of: Crockford's base32, which leaves out I, L, O
/// and U so a code read aloud or retyped is not mistaken.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters per code, without the dash.
const CODE_LEN: usize = 8;
/// How long a code stays valid.
pub(crate) const CODE_TTL: Duration = Duration::from_millis(PAIRING_CODE_TTL_MS);
/// Failed attempts within [`FAILURE_WINDOW`] after which every code is refused.
pub(crate) const MAX_FAILURES: usize = 5;
/// How long a failed attempt counts against [`MAX_FAILURES`].
pub(crate) const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Outstanding codes kept at most; minting another drops the oldest.
const MAX_CODES: usize = 16;
/// Pairings kept per client, one per box name it pairs under; pairing once
/// more drops the oldest. A client that lost its token and paired again
/// leaves its old pairing behind, which this bounds.
pub(crate) const MAX_PAIRINGS: usize = 8;

/// `prefix` and 256 bits from the OS random source as lowercase hex.
pub(crate) fn random_hex(prefix: &str) -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(format!("{prefix}{}", hex(&bytes)))
}

/// `bytes` as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Lowercase hex SHA-256 of `token`, the form a box stores.
pub(crate) fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Whether `a` and `b` are equal, in time that depends on their lengths only.
fn same(a: &[u8], b: &[u8]) -> bool {
    bool::from(a.ct_eq(b))
}

/// `text` as the code's 8 characters: the dash and spaces dropped, letters
/// uppercased, and the letters Crockford's alphabet reads as digits (O as 0,
/// I and L as 1) mapped. `None` when it cannot be a code.
fn normalize(text: &str) -> Option<[u8; CODE_LEN]> {
    let mut code = [0u8; CODE_LEN];
    let mut length = 0;
    for c in text.chars().filter(|c| *c != '-' && !c.is_whitespace()) {
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        let byte = u8::try_from(c)
            .ok()
            .filter(|byte| ALPHABET.contains(byte))?;
        *code.get_mut(length)? = byte;
        length += 1;
    }
    (length == CODE_LEN).then_some(code)
}

/// `XXXX-XXXX` for `code`.
fn display(code: &[u8; CODE_LEN]) -> String {
    let (head, tail) = code.split_at(CODE_LEN / 2);
    format!(
        "{}-{}",
        String::from_utf8_lossy(head),
        String::from_utf8_lossy(tail)
    )
}

/// One outstanding code.
struct Code {
    value: [u8; CODE_LEN],
    expires: Instant,
}

/// One outstanding ticket's secret, from `remote.pair` with `ticket`.
struct TicketEntry {
    secret: [u8; 16],
    expires: Instant,
    ephemeral: bool,
}

impl Drop for TicketEntry {
    /// Zeroes the secret, so a used or expired one does not stay in freed memory.
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

/// Why a code was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Wrong, expired or used; every outstanding code is void now.
    Wrong,
    /// Too many failures; every code is refused for `retry_after`.
    Locked { retry_after: Duration },
}

/// Why no code was minted.
#[derive(Debug)]
pub(crate) enum MintError {
    /// Too many failures; no code is handed out for this long.
    Paused(Duration),
    /// The OS random source failed.
    Random(io::Error),
}

/// The box's outstanding codes and ticket secrets and recent failures, in
/// memory only. The times are monotonic, so a clock change neither revives
/// nor voids a code; every method takes the time, so tests choose it. Only
/// wrong codes count towards the limit, and it refuses codes only.
#[derive(Default)]
pub(crate) struct Codes {
    codes: Vec<Code>,
    /// Secrets of the outstanding tickets, at most [`MAX_CODES`].
    tickets: Vec<TicketEntry>,
    /// Times of failed attempts within [`FAILURE_WINDOW`], oldest first.
    failures: VecDeque<Instant>,
}

impl Codes {
    /// A new code, valid for [`CODE_TTL`] from `now`. While failures refuse
    /// every code, none is handed out.
    pub(crate) fn mint(&mut self, now: Instant) -> Result<String, MintError> {
        if let Some(wait) = self.locked_for(now) {
            return Err(MintError::Paused(wait));
        }
        let mut bytes = [0u8; CODE_LEN];
        getrandom::fill(&mut bytes)
            .map_err(|error| MintError::Random(io::Error::other(error.to_string())))?;
        // 32 divides 256, so masking keeps every character equally likely.
        let value = bytes.map(|byte| ALPHABET[usize::from(byte & 31)]);
        if self.codes.len() >= MAX_CODES {
            self.codes.remove(0);
        }
        self.codes.push(Code {
            value,
            expires: now + CODE_TTL,
        });
        Ok(display(&value))
    }

    /// A new ticket secret, valid for [`CODE_TTL`] from `now`, with the
    /// ticket's `ephemeral` flag. Wrong codes do not stop it: their pause
    /// guards codes, and a ticket's 128 bits gain nothing from one.
    pub(crate) fn mint_ticket(&mut self, ephemeral: bool, now: Instant) -> io::Result<[u8; 16]> {
        let mut secret = [0u8; 16];
        getrandom::fill(&mut secret).map_err(|error| io::Error::other(error.to_string()))?;
        if self.tickets.len() >= MAX_CODES {
            self.tickets.remove(0);
        }
        self.tickets.push(TicketEntry {
            secret,
            expires: now + CODE_TTL,
            ephemeral,
        });
        Ok(secret)
    }

    /// Takes back a ticket that was minted but never handed out, because
    /// the request failed after minting it.
    pub(crate) fn drop_ticket(&mut self, secret: &[u8; 16]) {
        self.tickets.retain(|ticket| !same(&ticket.secret, secret));
    }

    /// Uses `input` up when it matches an outstanding code. A failure voids
    /// every outstanding code and counts towards the limit; while the limit
    /// holds, every code is refused without being compared.
    pub(crate) fn claim(&mut self, input: &str, now: Instant) -> Result<(), Refusal> {
        if let Some(retry_after) = self.locked_for(now) {
            return Err(Refusal::Locked { retry_after });
        }
        let normalized = normalize(input);
        let valid = normalized.is_some();
        let candidate = normalized.unwrap_or([0; CODE_LEN]);
        // Every code is compared, so the time does not tell which one matched.
        let mut found = None;
        for (index, code) in self.codes.iter().enumerate() {
            if same(&code.value, &candidate) {
                found = Some(index);
            }
        }
        match found.filter(|_| valid) {
            Some(index) => {
                self.codes.remove(index);
                Ok(())
            }
            None => {
                self.codes.clear();
                self.failures.push_back(now);
                Err(Refusal::Wrong)
            }
        }
    }

    /// How long every code stays refused, when failures reached the limit.
    pub(crate) fn locked_for(&mut self, now: Instant) -> Option<Duration> {
        self.prune(now);
        if self.failures.len() < MAX_FAILURES {
            return None;
        }
        let oldest = self.failures.front()?;
        Some((*oldest + FAILURE_WINDOW).saturating_duration_since(now))
    }

    /// Where the outstanding ticket whose secret is `secret` is. `None` is
    /// also a secret that was not 16 bytes. Every ticket is compared, so the
    /// time does not tell which one matched.
    fn find_ticket(&mut self, secret: Option<&[u8; 16]>, now: Instant) -> Option<usize> {
        self.prune(now);
        let candidate = secret.copied().unwrap_or([0; 16]);
        let mut found = None;
        for (index, ticket) in self.tickets.iter().enumerate() {
            if same(&ticket.secret, &candidate) {
                found = Some(index);
            }
        }
        found.filter(|_| secret.is_some())
    }

    /// Whether `secret` is an outstanding ticket's, without using it up, so
    /// the box answers a wrong secret before it says anything else.
    pub(crate) fn holds_ticket(&mut self, secret: Option<&[u8; 16]>, now: Instant) -> bool {
        self.find_ticket(secret, now).is_some()
    }

    /// Uses up the outstanding ticket whose secret is `secret` and says
    /// whether it was ephemeral; `None` for a wrong, malformed, expired or
    /// used secret. A miss counts nowhere and voids nothing: 128 random bits
    /// are not guessed, so a pause or a void would only let anyone who knows
    /// the box's id stop the owner's pairing.
    pub(crate) fn claim_ticket(&mut self, secret: Option<&[u8; 16]>, now: Instant) -> Option<bool> {
        let index = self.find_ticket(secret, now)?;
        Some(self.tickets.remove(index).ephemeral)
    }

    /// Whether a ticket is outstanding, which is when the box admits pairing
    /// connections from keys it does not know. Wrong codes do not close it.
    pub(crate) fn takes_tickets(&mut self, now: Instant) -> bool {
        self.prune(now);
        !self.tickets.is_empty()
    }

    /// When the last outstanding ticket expires; `None` without one.
    pub(crate) fn tickets_until(&mut self, now: Instant) -> Option<Instant> {
        self.prune(now);
        self.tickets.iter().map(|ticket| ticket.expires).max()
    }

    /// Whether a code or a ticket is still valid, which keeps an idle daemon
    /// running so it does not vanish with it.
    pub(crate) fn outstanding(&mut self, now: Instant) -> bool {
        self.prune(now);
        !self.codes.is_empty() || !self.tickets.is_empty()
    }

    /// Voids every outstanding code and ticket; recent failures still count.
    pub(crate) fn void(&mut self) {
        self.codes.clear();
        self.tickets.clear();
    }

    /// Forgets expired codes and tickets and failures older than the window.
    fn prune(&mut self, now: Instant) {
        self.codes.retain(|code| code.expires > now);
        self.tickets.retain(|ticket| ticket.expires > now);
        while self
            .failures
            .front()
            .is_some_and(|at| *at + FAILURE_WINDOW <= now)
        {
            self.failures.pop_front();
        }
    }
}

/// `retry_after` in milliseconds, for answers.
pub(super) fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

/// The text of too many wrong codes. The wait goes separately as
/// `retry_after_ms`, so a client can show it from a time that stays right.
const PAUSED: &str = "the box paused pairing after 5 wrong codes in 10 minutes";

/// `pairing_refused`, which the tunnel hands to the client as the reason.
fn refused(refusal: Refusal) -> RpcError {
    let (message, retry_after_ms) = match refusal {
        Refusal::Wrong => (
            "the pairing code is wrong, expired or already used. Get a new one with \
             `devlanes daemon pair` on the box"
                .to_owned(),
            None,
        ),
        Refusal::Locked { retry_after } => (PAUSED.to_owned(), Some(millis(retry_after))),
    };
    RpcError::new(
        "pairing_refused",
        json!({"message": message, "retry_after_ms": retry_after_ms}),
        4,
    )
}

/// `pairing_paused`: `daemon pair` hands out no code while failures refuse
/// every code.
fn paused(wait: Duration) -> RpcError {
    RpcError::new(
        "pairing_paused",
        json!({"message": PAUSED, "retry_after_ms": millis(wait)}),
        4,
    )
}

/// `io` errors of the random source, as the daemon reports them.
fn random_error(error: io::Error) -> RpcError {
    RpcError::new("random_failed", json!({"message": error.to_string()}), 1)
}

/// The client part of a tunnel's `$SSH_CONNECTION` (`<ip> <port> <ip> <port>`),
/// such as `203.0.113.4`.
fn client_address(ssh: Option<&str>) -> Option<String> {
    ssh.and_then(|ssh| ssh.split_whitespace().next())
        .map(str::to_owned)
}

/// `remote.pair` parameters. Without `ticket` only a code is minted, as
/// before tickets existed, so a client that asks for none opens no listener.
#[derive(Debug, Default, Deserialize)]
struct PairParams {
    /// Mint a pairing ticket too.
    #[serde(default)]
    ticket: bool,
    /// The client forgets the box once it stays unreachable for an hour.
    #[serde(default)]
    ephemeral: bool,
}

/// `revoke` parameters.
#[derive(Debug, Deserialize)]
struct RevokeParams {
    client: String,
}

/// Shortest start of an id `revoke` matches: the prefix and four hex digits.
const MIN_ID_PREFIX: usize = CLIENT_ID_PREFIX.len() + 4;

/// A paired client with all its pairings, as `daemon clients` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Paired {
    id: String,
    /// The name of its most recently used pairing.
    name: String,
    /// When its first pairing was made.
    paired_at: i64,
    last_seen: Option<i64>,
    pairings: usize,
    /// The short form of the most recently used of its ticket keys.
    p2p_key: Option<String>,
}

/// Token `pairings` and the clients `p2p` paired by ticket, grouped by
/// client id, in the order of each client's first appearance there.
fn by_client(pairings: &[RemotePairing], p2p: &[P2pClient]) -> Vec<Paired> {
    let mut clients: Vec<Paired> = Vec::new();
    let tokens = pairings.iter().map(|pairing| {
        (
            &pairing.client_id,
            &pairing.name,
            pairing.paired_at,
            pairing.last_seen,
        )
    });
    let tickets = p2p.iter().map(|client| {
        (
            &client.client_id,
            &client.name,
            client.paired_at,
            client.last_seen,
        )
    });
    for (id, name, paired_at, last_seen) in tokens.chain(tickets) {
        let recent = last_seen.unwrap_or(paired_at);
        match clients.iter_mut().find(|c| c.id == *id) {
            Some(client) => {
                if recent > client.last_seen.unwrap_or(client.paired_at) {
                    client.name.clone_from(name);
                }
                client.paired_at = client.paired_at.min(paired_at);
                client.last_seen = client.last_seen.max(last_seen);
                client.pairings += 1;
            }
            None => clients.push(Paired {
                id: id.clone(),
                name: name.clone(),
                paired_at,
                last_seen,
                pairings: 1,
                p2p_key: None,
            }),
        }
    }
    // Compared among the client's keys only: a newer token pairing says
    // nothing about which key it used last.
    for client in &mut clients {
        client.p2p_key = p2p
            .iter()
            .filter(|key| key.client_id == client.id)
            .max_by_key(|key| key.last_seen.unwrap_or(key.paired_at))
            .and_then(|key| key.peer_key.get(..8))
            .map(str::to_owned);
    }
    clients
}

/// `connections_off`: this machine turns every client away, so a code would
/// be of no use. `invalid` says the user config's remote settings have
/// problems and count as false, rather than the setting being false.
fn connections_off(invalid: bool) -> RpcError {
    let message = if invalid {
        "this machine does not accept connections because the remote settings in its user \
         config are invalid"
    } else {
        "this machine does not accept connections because remote.allow_connections is false"
    };
    RpcError::new(
        "connections_off",
        json!({"message": message, "invalid": invalid}),
        4,
    )
}

/// `remote.allow_connections` of the user config in `home`, read as the
/// tunnel reads it ([`devlanes_config::box_settings`]): a missing file
/// allows connections, a problem under `remote` or with the whole file does
/// not. The second value says such a problem made it false. Blocking.
pub(super) fn allow_connections(home: &std::path::Path) -> (bool, bool) {
    let path = home.join("config.yaml");
    if !path.exists() {
        return (true, false);
    }
    let (config, problems) = devlanes_config::load_user_lenient(&path);
    let (settings, remote) = devlanes_config::box_settings(&config, problems);
    (settings.allow_connections, !remote.is_empty())
}

impl Inner {
    /// `remote.pair`: a new one-time code, and with `ticket` a pairing ticket
    /// too, which starts the peer-to-peer listener until it is used or
    /// expires. `connections_off` when this machine's user config turns
    /// clients away, `pairing_paused` while wrong codes refuse every code,
    /// `p2p_unavailable` when the endpoint cannot listen or tell its address.
    /// While codes are paused a ticket still goes out, without a code and
    /// with `code_paused_ms`. Neither the code nor the ticket is logged.
    pub(crate) async fn pair_method(self: &Arc<Self>, value: Value) -> Result<Value, RpcError> {
        let args: PairParams = params(value)?;
        let home = self.opts.home.clone();
        let (allowed, invalid) = tokio::task::spawn_blocking(move || allow_connections(&home))
            .await
            .unwrap_or((false, true));
        if !allowed {
            return Err(connections_off(invalid));
        }
        let expires_at_ms = now_ms().saturating_add(PAIRING_CODE_TTL_MS);
        if !args.ticket {
            let minted = lock(&self.remote.codes).mint(Instant::now());
            let code = match minted {
                Ok(code) => code,
                Err(MintError::Paused(wait)) => return Err(paused(wait)),
                Err(MintError::Random(error)) => return Err(random_error(error)),
            };
            tracing::info!(expires_at_ms, "issued a pairing code");
            return Ok(json!({
                "code": code,
                "expires_at_ms": expires_at_ms,
                "valid_ms": PAIRING_CODE_TTL_MS,
            }));
        }
        // The ticket first: a code minted before a failed ticket would stay
        // valid with nobody to type it.
        let (ticket, secret) = self.mint_ticket(args.ephemeral).await?;
        let mut answer = json!({
            "ticket": ticket,
            "expires_at_ms": expires_at_ms,
            "valid_ms": PAIRING_CODE_TTL_MS,
        });
        let minted = lock(&self.remote.codes).mint(Instant::now());
        match minted {
            Ok(code) => answer["code"] = Value::from(code),
            // Wrong codes pause codes only: the ticket goes out without one.
            Err(MintError::Paused(wait)) => answer["code_paused_ms"] = Value::from(millis(wait)),
            Err(MintError::Random(error)) => {
                self.take_back_ticket(&secret).await;
                return Err(random_error(error));
            }
        }
        tracing::info!(
            expires_at_ms,
            code = answer.get("code").is_some(),
            "issued a pairing ticket"
        );
        Ok(answer)
    }

    /// A ticket for `remote.pair`, as text with its secret: mints the
    /// secret, listens and reads the address to put in it. A failure takes
    /// the secret back and applies the listening rule again, so a ticket
    /// nobody got leaves no listener behind.
    async fn mint_ticket(
        self: &Arc<Self>,
        ephemeral: bool,
    ) -> Result<(String, [u8; 16]), RpcError> {
        let Some(endpoint) = self
            .integ
            .peer
            .clone()
            .filter(|_| self.integ.box_serving.is_some())
        else {
            return Err(RpcError::new(
                "not_supported_yet",
                json!({"feature": "pairing by ticket"}),
                1,
            ));
        };
        let minted = lock(&self.remote.codes).mint_ticket(ephemeral, Instant::now());
        let secret = minted.map_err(random_error)?;
        let addr = match self.refresh_listening().await {
            Ok(()) => endpoint.addr().await.map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        // A ticket without its relay works on a LAN only and fails across
        // NATs with a reason that points elsewhere, so it is not handed out.
        let addr = addr.and_then(|addr| match &addr.relay {
            Some(relay) if relay.len() > Ticket::MAX_RELAY => Err(format!(
                "the relay URL {relay} is longer than {} bytes, so a ticket cannot hold it; \
                 shorten remote.relays",
                Ticket::MAX_RELAY
            )),
            _ => Ok(addr),
        });
        match addr {
            Ok(addr) => {
                let ticket = Ticket {
                    addr,
                    secret: TicketSecret(secret),
                    ephemeral,
                };
                Ok((ticket.encode(), secret))
            }
            Err(message) => {
                self.take_back_ticket(&secret).await;
                tracing::warn!(%message, "cannot hand out a pairing ticket");
                Err(RpcError::new(
                    "p2p_unavailable",
                    json!({"message": message}),
                    1,
                ))
            }
        }
    }

    /// Takes back a minted ticket nobody got, then applies the listening
    /// rule, so it leaves no listener behind.
    async fn take_back_ticket(self: &Arc<Self>, secret: &[u8; 16]) {
        lock(&self.remote.codes).drop_ticket(secret);
        if let Err(error) = self.refresh_listening().await {
            tracing::debug!(%error, "cannot stop listening after a failed ticket");
        }
    }

    /// `remote.pairing.verify`: whether a client's token is one of its
    /// pairings'; an accepted one updates that pairing's name and last seen
    /// time. A refused token is logged with the client's id and address.
    /// Every pairing's hash is compared in constant time, and a placeholder
    /// when the client has none, so the time tells nothing either way.
    pub(crate) async fn verify_pairing(&self, value: Value) -> Result<Value, RpcError> {
        let args: VerifyParams = required(value)?;
        let Some(token) = args.token else {
            return Ok(json!({"paired": false, "reason": PairingReason::Missing}));
        };
        let id = clean(&args.id);
        let stored = self
            .store
            .remote_pairings_of(id.clone())
            .await
            .map_err(store_error)?;
        let presented = token_hash(&token);
        let placeholder = "0".repeat(presented.len());
        let mut matches = stored.is_empty() && same(presented.as_bytes(), placeholder.as_bytes());
        for pairing in &stored {
            // `|`, not `||`: no comparison is skipped once one matched.
            matches |= same(presented.as_bytes(), pairing.token_sha256.as_bytes());
        }
        let matches = matches && !stored.is_empty();
        let name = clean(&args.name);
        let address = args.address.as_deref().map(clean);
        if !matches {
            tracing::warn!(client_id = %id, %name, address = ?address, "refused a client's remote token");
            return Ok(json!({"paired": false, "reason": PairingReason::Invalid}));
        }
        let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        if let Err(error) = self.store.touch_remote_pairing(presented, name, at).await {
            tracing::warn!(%error, client_id = %id, "cannot record when a client was last seen");
        }
        Ok(json!({"paired": true}))
    }

    /// `remote.pairing.claim`: trades a one-time code for a remote token and
    /// stores the pairing next to the client's others, dropping its oldest
    /// beyond [`MAX_PAIRINGS`]. A refused code is logged with the client's id
    /// and address.
    pub(crate) async fn claim_pairing(&self, value: Value) -> Result<Value, RpcError> {
        let args: ClaimParams = required(value)?;
        let id = clean(&args.id);
        let name = clean(&args.name);
        let address = args.address.as_deref().map(clean);
        if !is_random_id(&id, CLIENT_ID_PREFIX) {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": "the client sent no valid client id"}),
                2,
            ));
        }
        let outcome = lock(&self.remote.codes).claim(&args.code, Instant::now());
        if let Err(refusal) = outcome {
            match refusal {
                Refusal::Wrong => tracing::warn!(
                    client_id = %id, %name, address = ?address,
                    "refused a pairing code; every outstanding code is void now"
                ),
                Refusal::Locked { retry_after } => tracing::warn!(
                    client_id = %id, %name, address = ?address,
                    retry_after_ms = millis(retry_after),
                    "refused a pairing code after too many failures"
                ),
            }
            return Err(refused(refusal));
        }
        let token = random_hex(TOKEN_PREFIX).map_err(random_error)?;
        let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        let dropped = self
            .store
            .add_remote_pairing(
                RemotePairing {
                    client_id: id.clone(),
                    name: name.clone(),
                    token_sha256: token_hash(&token),
                    paired_at: at,
                    last_seen: Some(at),
                },
                MAX_PAIRINGS,
            )
            .await
            .map_err(store_error)?;
        for old in dropped {
            tracing::info!(
                client_id = %id, %name, old_name = %old.name, paired_at = old.paired_at,
                "replaced the oldest pairing of {}", old.name
            );
        }
        tracing::info!(client_id = %id, %name, address = ?address, "paired a client");
        self.clients_changed();
        Ok(json!({"token": token}))
    }

    /// Announces that `remote.clients` would answer differently now
    /// ([`CLIENTS_EVENT`]), so a TUI showing the clients fetches them again.
    pub(crate) fn clients_changed(&self) {
        self.bus.emit(CLIENTS_EVENT, json!({}));
    }

    /// `remote.clients`: every paired client and every client a tunnel serves
    /// now, connected ones first, then by name, with its connections
    /// (address, since when, the client's devlanes version, and what it
    /// mirrors: `{ports, bound, skipped_lanes}`) and how many pairings it has,
    /// and this daemon's clock
    /// (`now_ms`) for the times.
    pub(crate) async fn clients_method(&self) -> Result<Value, RpcError> {
        let pairings = self.store.remote_pairings().await.map_err(store_error)?;
        let p2p = self.store.p2p_clients().await.map_err(store_error)?;
        let live: Vec<(ClientKey, RemoteClient)> = lock(&self.remote.clients)
            .iter()
            .map(|(key, (_, client))| (key.clone(), client.clone()))
            .collect();
        let mut items: Vec<Value> = by_client(&pairings, &p2p)
            .into_iter()
            .map(|client| {
                let mut item = json!({
                    "id": client.id,
                    "name": client.name,
                    "paired": true,
                    "pairings": client.pairings,
                    "paired_at_ms": client.paired_at,
                    "last_seen_ms": client.last_seen,
                    "connections": [],
                });
                // Only on clients paired by ticket, so other answers stay as they were.
                if let Some(key) = client.p2p_key {
                    item["transport"] = Value::from("p2p");
                    item["key"] = Value::from(key);
                }
                item
            })
            .collect();
        for (key, client) in live {
            let connection = json!({
                "address": client_address(client.ssh.as_deref()),
                "since_ms": client.since_ms,
                "version": client.version,
                "mirror": client.mirror.as_ref().map(|report| json!({
                    "ports": report.ports.len(),
                    "bound": report.ports.iter().filter(|port| port.state == "bound").count(),
                    "skipped_lanes": report.skipped_lanes.len(),
                })),
            });
            let existing = match &key {
                ClientKey::Id(id) => items.iter_mut().find(|item| item["id"] == id.as_str()),
                ClientKey::Anonymous(_) => None,
            };
            match existing {
                Some(item) => {
                    if let Some(connections) = item["connections"].as_array_mut() {
                        connections.push(connection);
                    }
                }
                None => items.push(json!({
                    "id": client.id,
                    "name": client.name,
                    "paired": false,
                    "pairings": 0,
                    "paired_at_ms": null,
                    "last_seen_ms": null,
                    "connections": [connection],
                })),
            }
        }
        sort_clients(&mut items);
        Ok(json!({"items": items, "now_ms": now_ms()}))
    }

    /// `remote.revoke`: deletes every pairing of the client named by its id,
    /// its name or a unique start of its id, then ends its sessions, in that
    /// order: a feed that starts meanwhile finds the pairing gone
    /// ([`RegistryParams::paired`](devlanes_proto::remote::RegistryParams::paired)).
    /// A client connected without pairing cannot be revoked; the answer says
    /// so (`unpaired`). A client paired by ticket loses its pinned key too,
    /// and its sessions are closed with the refused-key reason.
    pub(crate) async fn revoke_method(self: &Arc<Self>, value: Value) -> Result<Value, RpcError> {
        let args: RevokeParams = required(value)?;
        let target = args.client.trim().to_owned();
        let pairings = self.store.remote_pairings().await.map_err(store_error)?;
        let p2p = self.store.p2p_clients().await.map_err(store_error)?;
        let clients = by_client(&pairings, &p2p);
        let found = matching(&clients, &target);
        if found.is_empty() {
            let unpaired = lock(&self.remote.clients).values().any(|(_, client)| {
                !client.id.is_empty() && (client.id == target || client.name == target)
            });
            let all: Vec<&Paired> = clients.iter().collect();
            return Err(RpcError::new(
                "client_not_found",
                json!({"client": target, "unpaired": unpaired, "candidates": candidates(&all)}),
                3,
            ));
        }
        if found.len() > 1 {
            return Err(RpcError::new(
                "client_ambiguous",
                json!({"client": target, "candidates": candidates(&found)}),
                3,
            ));
        }
        let ids: Vec<String> = found.iter().map(|client| client.id.clone()).collect();
        // Rows and pins change together under this lock, so a pairing of
        // the same client meanwhile is either deleted here with its pin or
        // stored and pinned after.
        let rule = self.pairing_rule().await;
        // One transaction: a failure deletes neither kind of pairing.
        self.store
            .delete_client_pairings(ids.clone())
            .await
            .map_err(store_error)?;
        // Unpinned before anything is closed, so the endpoint refuses the
        // client's next dial while its sessions end.
        let keys = self.unpin_peers(&rule, |_, id| ids.iter().any(|revoked| revoked == id));
        drop(rule);
        // Counted before the connections close, which ends the feeds too.
        let ended = self
            .remote
            .end_feeds(|id| ids.iter().any(|revoked| revoked == id));
        self.close_peers(&keys);
        // Last: with no client and no ticket left the endpoint stops
        // listening, after `close_peer` sent the refusal while it listened.
        if let Err(error) = self.refresh_listening().await {
            tracing::warn!(%error, "cannot apply the listening rule after a revoke");
        }
        for client in &found {
            tracing::info!(client_id = %client.id, name = %client.name, ended, "revoked a client's pairings");
        }
        self.clients_changed();
        let revoked: Vec<Value> = found
            .iter()
            .map(|client| json!({"id": client.id, "name": client.name}))
            .collect();
        Ok(json!({"revoked": revoked, "ended": ended}))
    }

    /// After `daemon reset-state` deleted every pairing while holding `rule`,
    /// the guard of [`Inner::pairing_rule`]: voids the outstanding codes and
    /// tickets, unpins every key and ends every client's session, so no
    /// client keeps a box that no longer knows it, then stops listening.
    /// Recent failed attempts still count.
    pub(crate) async fn forget_pairings(self: &Arc<Self>, rule: tokio::sync::MutexGuard<'_, ()>) {
        lock(&self.remote.codes).void();
        let keys = self.unpin_peers(&rule, |_, _| true);
        self.remote.peer.forget_failed_load();
        drop(rule);
        let ended = self.remote.end_feeds(|_| true);
        self.close_peers(&keys);
        if ended > 0 {
            tracing::info!(ended, "ended every client's session after a state reset");
        }
        if let Err(error) = self.refresh_listening().await {
            tracing::warn!(%error, "cannot stop listening after a state reset");
        }
        self.clients_changed();
    }
}

/// The clients `target` names: the one with that id, else those with that
/// name, else those whose id starts with it (at least [`MIN_ID_PREFIX`] long).
fn matching<'a>(clients: &'a [Paired], target: &str) -> Vec<&'a Paired> {
    if let Some(exact) = clients.iter().find(|c| c.id == target) {
        return vec![exact];
    }
    let named: Vec<&Paired> = clients.iter().filter(|c| c.name == target).collect();
    if !named.is_empty() || target.len() < MIN_ID_PREFIX {
        return named;
    }
    clients
        .iter()
        .filter(|c| c.id.starts_with(target))
        .collect()
}

/// `remote.clients` items with connected clients first, then by name and id,
/// so `daemon clients` and the TUI list them alike.
fn sort_clients(items: &mut [Value]) {
    items.sort_by_cached_key(|item| {
        let away = item["connections"]
            .as_array()
            .is_none_or(|connections| connections.is_empty());
        (
            away,
            item["name"].as_str().unwrap_or_default().to_owned(),
            item["id"].as_str().unwrap_or_default().to_owned(),
        )
    });
}

/// `{id, name}` of each client, for a `client_not_found` or
/// `client_ambiguous` answer.
fn candidates(clients: &[&Paired]) -> Vec<Value> {
    clients
        .iter()
        .map(|c| json!({"id": c.id, "name": c.name}))
        .collect()
}

/// A code from `remote connect --code` and when it was given: it is used
/// only within [`CODE_TTL`], after which the box has dropped it anyway and
/// sending it would only void the box's other codes.
pub(super) type GivenCode = Arc<std::sync::Mutex<Option<(String, Instant)>>>;

/// Client side: the token this machine keeps for one box, and the person who
/// types the box's code, through `remote connect --code` or a prompt to the
/// attached `remote connect`.
pub(super) struct HostPairing {
    pub(super) host: String,
    pub(super) store: Store,
    pub(super) prompter: Arc<dyn RemotePrompter>,
    /// A code from `remote connect --code`, used by the next pairing only.
    pub(super) code: GivenCode,
}

#[async_trait]
impl PairingClient for HostPairing {
    /// The box's token from `remote_tokens`; a read failure counts as none.
    async fn token(&self) -> Option<String> {
        match self.store.remote_token(self.host.clone()).await {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, host = %self.host, "cannot read the box's remote token");
                None
            }
        }
    }

    /// The code from `--code` while it can still be valid, else one asked of
    /// the attached `remote connect`.
    async fn code(&self, reason: PairingReason) -> Option<String> {
        let given = lock(&self.code).take();
        match given {
            Some((code, at)) if at.elapsed() < CODE_TTL => return Some(code),
            Some(_) => {
                tracing::debug!(host = %self.host, "dropping a --code older than a code lives")
            }
            None => {}
        }
        let prompt = match reason {
            PairingReason::Missing => "pairing code from `devlanes daemon pair` on the box:",
            PairingReason::Invalid => "new pairing code from `devlanes daemon pair` on the box:",
        };
        self.prompter
            .ask(RemotePrompt {
                host: self.host.clone(),
                prompt: prompt.to_owned(),
                kind: "code",
            })
            .await
    }

    /// Stores the token in `remote_tokens`.
    async fn store(&self, token: String) -> Result<(), BoxError> {
        let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        self.store
            .set_remote_token(self.host.clone(), token, at)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    /// A code works once, in any case and with or without its dash, and not
    /// after it expired.
    #[test]
    fn a_code_is_used_once_and_expires() -> Result<(), MintError> {
        let start = Instant::now();
        let mut codes = Codes::default();
        let code = codes.mint(start)?;
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(codes.outstanding(start));
        let typed = code.replace('-', " ").to_lowercase();
        assert_eq!(codes.claim(&typed, start + MINUTE), Ok(()));
        assert!(!codes.outstanding(start + MINUTE));
        assert_eq!(
            codes.claim(&code, start + MINUTE),
            Err(Refusal::Wrong),
            "used up"
        );

        let late = codes.mint(start)?;
        assert!(codes.outstanding(start + CODE_TTL - Duration::from_millis(1)));
        assert!(!codes.outstanding(start + CODE_TTL));
        assert_eq!(
            codes.claim(&late, start + CODE_TTL),
            Err(Refusal::Wrong),
            "expired"
        );
        Ok(())
    }

    /// One wrong code voids every outstanding code, including valid ones,
    /// and so does a state reset, which keeps the failures.
    #[test]
    fn a_failure_or_a_reset_voids_every_code() -> Result<(), MintError> {
        let start = Instant::now();
        let mut codes = Codes::default();
        let first = codes.mint(start)?;
        let second = codes.mint(start)?;
        assert_eq!(codes.claim("not-a-code", start), Err(Refusal::Wrong));
        assert!(!codes.outstanding(start));
        assert_eq!(codes.claim(&first, start), Err(Refusal::Wrong));
        assert_eq!(codes.claim(&second, start), Err(Refusal::Wrong));
        let third = codes.mint(start)?;
        codes.void();
        assert!(!codes.outstanding(start));
        assert_eq!(codes.claim(&third, start), Err(Refusal::Wrong));
        assert_eq!(codes.failures.len(), 4, "the reset forgot no failure");
        Ok(())
    }

    /// Five failures within ten minutes refuse every code with the time
    /// until the oldest failure is ten minutes old, and no new code is handed
    /// out meanwhile; then codes work again.
    #[test]
    fn failures_lock_out_codes_for_a_while() -> Result<(), MintError> {
        let start = Instant::now();
        let mut codes = Codes::default();
        let early = codes.mint(start)?;
        for minute in 0..MAX_FAILURES as u32 {
            assert_eq!(
                codes.claim("0000-0000", start + MINUTE * minute),
                Err(Refusal::Wrong)
            );
        }
        assert_eq!(codes.locked_for(start + MINUTE * 5), Some(MINUTE * 5));
        assert!(matches!(
            codes.mint(start + MINUTE * 6),
            Err(MintError::Paused(wait)) if wait == MINUTE * 4
        ));
        assert_eq!(
            codes.claim(&early, start + MINUTE * 6),
            Err(Refusal::Locked {
                retry_after: MINUTE * 4
            })
        );
        // The oldest failure leaves the window at minute 10.
        assert_eq!(codes.locked_for(start + MINUTE * 10), None);
        let code = codes.mint(start + MINUTE * 10)?;
        assert_eq!(codes.claim(&code, start + MINUTE * 10), Ok(()));
        Ok(())
    }

    /// A ticket's secret works once, with the ticket's flag, and not after
    /// it expired; until then it opens pairing to unknown keys.
    #[test]
    fn a_ticket_is_used_once_and_expires() {
        let start = Instant::now();
        let mut codes = Codes::default();
        let secret = codes.mint_ticket(true, start).expect("ticket");
        assert!(codes.takes_tickets(start));
        assert!(codes.outstanding(start), "keeps an idle daemon up");
        assert!(
            codes.holds_ticket(Some(&secret), start),
            "checked, not used"
        );
        assert_eq!(
            codes.claim_ticket(Some(&secret), start + MINUTE),
            Some(true)
        );
        assert!(!codes.takes_tickets(start + MINUTE));
        assert_eq!(
            codes.claim_ticket(Some(&secret), start + MINUTE),
            None,
            "used up"
        );

        let late = codes.mint_ticket(false, start).expect("ticket");
        assert_eq!(codes.tickets_until(start), Some(start + CODE_TTL));
        assert!(codes.takes_tickets(start + CODE_TTL - Duration::from_millis(1)));
        assert_eq!(codes.tickets_until(start + CODE_TTL), None);
        assert!(!codes.holds_ticket(Some(&late), start + CODE_TTL));
        assert_eq!(
            codes.claim_ticket(Some(&late), start + CODE_TTL),
            None,
            "expired"
        );
    }

    /// Wrong or malformed secrets, however many, count nowhere and void
    /// nothing: the ticket and a code minted before them still work, and
    /// codes are not paused.
    #[test]
    fn wrong_secrets_pause_and_void_nothing() {
        let start = Instant::now();
        let mut codes = Codes::default();
        let code = codes.mint(start).expect("code");
        let secret = codes.mint_ticket(false, start).expect("ticket");
        let mut wrong = secret;
        wrong[0] ^= 1;
        for _ in 0..MAX_FAILURES * 2 {
            assert_eq!(codes.claim_ticket(Some(&wrong), start), None);
            assert_eq!(codes.claim_ticket(None, start), None);
        }
        assert!(codes.failures.is_empty());
        assert_eq!(codes.locked_for(start), None);
        assert_eq!(codes.claim(&code, start), Ok(()), "the code still works");
        assert_eq!(codes.claim_ticket(Some(&secret), start), Some(false));
    }

    /// Wrong codes pause codes only: pairing connections are still let in,
    /// a ticket minted before or during the pause works, and a wrong code
    /// voids no ticket.
    #[test]
    fn wrong_codes_pause_codes_and_leave_tickets_usable() {
        let start = Instant::now();
        let mut codes = Codes::default();
        let before = codes.mint_ticket(false, start).expect("ticket");
        for _ in 0..MAX_FAILURES {
            assert_eq!(codes.claim("0000-0000", start), Err(Refusal::Wrong));
        }
        assert!(matches!(codes.mint(start), Err(MintError::Paused(_))));
        assert!(codes.takes_tickets(start));
        let during = codes.mint_ticket(true, start).expect("ticket while paused");
        assert_eq!(
            codes.claim_ticket(Some(&before), start + MINUTE),
            Some(false)
        );
        assert_eq!(
            codes.claim_ticket(Some(&during), start + MINUTE),
            Some(true)
        );
    }

    /// Codes use the unambiguous alphabet, and the letters read as digits
    /// are accepted for them.
    #[test]
    fn codes_read_ambiguous_letters_as_digits() {
        assert_eq!(normalize("o1l0-IIOO"), Some(*b"01101100"));
        assert_eq!(normalize("ABCD-EFG"), None, "too short");
        assert_eq!(normalize("ABCD-EFGHJ"), None, "too long");
        assert_eq!(normalize("ABCD-EFGU"), None, "U is not in the alphabet");
        assert_eq!(normalize("ABCD-ÉFGH"), None);
        assert!(!ALPHABET.iter().any(|c| b"ILOU".contains(c)));
    }

    /// Tokens and ids are their prefix and 64 lowercase hex digits; a box
    /// keeps their SHA-256, also in lowercase hex.
    #[test]
    fn tokens_are_random_and_stored_as_hashes() -> io::Result<()> {
        let token = random_hex(TOKEN_PREFIX)?;
        assert!(is_random_id(&token, TOKEN_PREFIX), "{token}");
        assert_ne!(random_hex(TOKEN_PREFIX)?, token);
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        Ok(())
    }

    /// A client's pairings, by token and by ticket, make one entry: the name
    /// of the one used last, the first pairing time, the last connection,
    /// their count and the key of its ticket pairing.
    #[test]
    fn pairings_are_grouped_by_client() {
        let pairing =
            |id: &str, name: &str, paired_at: i64, last_seen: Option<i64>| RemotePairing {
                client_id: id.into(),
                name: name.into(),
                token_sha256: format!("{id}{paired_at}"),
                paired_at,
                last_seen,
            };
        let ticket = |id: &str, name: &str, key: char, paired_at: i64| P2pClient {
            peer_key: key.to_string().repeat(64),
            client_id: id.into(),
            name: name.into(),
            paired_at,
            last_seen: Some(paired_at),
        };
        let clients = by_client(
            &[
                pairing("dl_cid_a", "laptop", 5, Some(9)),
                pairing("dl_cid_a", "old-laptop", 2, Some(3)),
                pairing("dl_cid_b", "desk", 4, None),
                pairing("dl_cid_d", "nas", 1, Some(30)),
            ],
            // The desk paired by ticket since, under a new name; another
            // client only by ticket; the NAS has two keys, the older listed
            // first, and a token pairing used after both.
            &[
                ticket("dl_cid_b", "desk-2", 'b', 6),
                ticket("dl_cid_c", "tower", 'c', 7),
                ticket("dl_cid_d", "nas", 'f', 10),
                ticket("dl_cid_d", "nas", 'e', 20),
            ],
        );
        assert_eq!(
            clients,
            [
                Paired {
                    id: "dl_cid_a".into(),
                    name: "laptop".into(),
                    paired_at: 2,
                    last_seen: Some(9),
                    pairings: 2,
                    p2p_key: None,
                },
                Paired {
                    id: "dl_cid_b".into(),
                    name: "desk-2".into(),
                    paired_at: 4,
                    last_seen: Some(6),
                    pairings: 2,
                    p2p_key: Some("bbbbbbbb".into()),
                },
                Paired {
                    id: "dl_cid_d".into(),
                    name: "nas".into(),
                    paired_at: 1,
                    last_seen: Some(30),
                    pairings: 3,
                    p2p_key: Some("eeeeeeee".into()),
                },
                Paired {
                    id: "dl_cid_c".into(),
                    name: "tower".into(),
                    paired_at: 7,
                    last_seen: Some(7),
                    pairings: 1,
                    p2p_key: Some("cccccccc".into()),
                },
            ]
        );
    }

    /// Connected clients come first, then everyone by name and id.
    #[test]
    fn clients_list_connected_ones_first() {
        let item = |id: &str, name: &str, connections: usize| {
            let connections = vec![json!({}); connections];
            json!({"id": id, "name": name, "connections": connections})
        };
        let mut items = vec![
            item("dl_cid_c", "alpha", 0),
            item("dl_cid_b", "zulu", 1),
            item("dl_cid_d", "beta", 0),
            item("dl_cid_a", "mike", 2),
            item("dl_cid_e", "alpha", 0),
            item("", "beta", 1),
        ];
        sort_clients(&mut items);
        let order: Vec<&str> = items
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            order,
            [
                "", "dl_cid_a", "dl_cid_b", "dl_cid_c", "dl_cid_e", "dl_cid_d"
            ]
        );
    }

    /// `revoke` takes an exact id first, then a name, then a long enough
    /// start of an id; two clients with the same name stay ambiguous.
    #[test]
    fn revoke_names_clients_by_id_name_or_id_start() {
        let client = |id: &str, name: &str| Paired {
            id: id.into(),
            name: name.into(),
            paired_at: 0,
            last_seen: None,
            pairings: 1,
            p2p_key: None,
        };
        let clients = [
            client("dl_cid_aaaa1111", "laptop"),
            client("dl_cid_aaaa2222", "laptop"),
            client("dl_cid_bbbb3333", "desk"),
        ];
        let ids = |target: &str| -> Vec<String> {
            matching(&clients, target)
                .iter()
                .map(|c| c.id.clone())
                .collect()
        };
        assert_eq!(ids("dl_cid_aaaa1111"), ["dl_cid_aaaa1111"]);
        assert_eq!(ids("desk"), ["dl_cid_bbbb3333"]);
        assert_eq!(ids("laptop").len(), 2);
        assert_eq!(ids("dl_cid_bbbb"), ["dl_cid_bbbb3333"]);
        assert_eq!(ids("dl_cid_aaaa").len(), 2);
        assert!(ids("dl_cid_b").is_empty(), "too short a start");
        assert!(ids("nobody").is_empty());
    }

    /// A `--code` is given once, and not after a code would have expired.
    #[tokio::test]
    async fn a_given_code_is_used_once_and_not_once_stale() -> Result<(), Box<dyn std::error::Error>>
    {
        struct Nobody;

        #[async_trait]
        impl RemotePrompter for Nobody {
            async fn ask(&self, _: RemotePrompt) -> Option<String> {
                None
            }
        }

        let code: GivenCode = Arc::default();
        let pairing = HostPairing {
            host: "box".into(),
            store: Store::open(":memory:").await?,
            prompter: Arc::new(Nobody),
            code: code.clone(),
        };
        *lock(&code) = Some(("K7QM-3XWD".into(), Instant::now()));
        let reason = PairingReason::Missing;
        assert_eq!(pairing.code(reason).await.as_deref(), Some("K7QM-3XWD"));
        assert_eq!(pairing.code(reason).await, None, "used once");
        let Some(old) = Instant::now().checked_sub(CODE_TTL + MINUTE) else {
            // A monotonic clock that started less than 11 minutes ago.
            return Ok(());
        };
        *lock(&code) = Some(("K7QM-3XWD".into(), old));
        assert_eq!(pairing.code(reason).await, None, "a stale code is not sent");
        Ok(())
    }
}
