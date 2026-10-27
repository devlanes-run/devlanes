//! Pairing. On a box: one-time codes for `devlanes remote pair`, kept in
//! memory with the failed attempts that limit them, the remote tokens they
//! are traded for (only their SHA-256 is stored, in `remote_pairings`), and
//! `remote clients` and `remote revoke`. On a client: the token each box gave
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
        CLIENT_ID_PREFIX, ClaimParams, PAIRING_CODE_TTL_MS, RemoteClient, TOKEN_PREFIX,
        VerifyParams, is_random_id,
    },
};
use devlanes_state::{RemotePairing, Store};
use devlanes_transport::{BoxError, PairingClient, PairingReason};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{ClientKey, feed::clean, now_ms};
use crate::{
    engine::{Inner, required},
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

/// The box's outstanding codes and recent failures, in memory only. The
/// times are monotonic, so a clock change neither revives nor voids a code;
/// every method takes the time, so tests choose it.
#[derive(Default)]
pub(crate) struct Codes {
    codes: Vec<Code>,
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

    /// Whether a code is still valid, which keeps an idle daemon running so
    /// the code does not vanish with it.
    pub(crate) fn outstanding(&mut self, now: Instant) -> bool {
        self.prune(now);
        !self.codes.is_empty()
    }

    /// Voids every outstanding code; recent failures still count.
    pub(crate) fn void(&mut self) {
        self.codes.clear();
    }

    /// Forgets expired codes and failures older than the window.
    fn prune(&mut self, now: Instant) {
        self.codes.retain(|code| code.expires > now);
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
fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

/// The text of too many wrong codes. The wait goes separately as
/// `retry_after_ms`, so a client can show it from a time that stays right.
const PAUSED: &str = "5 wrong pairing codes in the last 10 minutes";

/// `pairing_refused`, which the tunnel hands to the client as the reason.
fn refused(refusal: Refusal) -> RpcError {
    let (message, retry_after_ms) = match refusal {
        Refusal::Wrong => (
            "the pairing code is wrong, expired or already used, and every other code from \
             `devlanes remote pair` is void now; run `devlanes remote pair` on the box for a new one"
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

/// `pairing_paused`: `remote pair` hands out no code while failures refuse
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

/// `revoke` parameters.
#[derive(Debug, Deserialize)]
struct RevokeParams {
    client: String,
}

/// Shortest start of an id `revoke` matches: the prefix and four hex digits.
const MIN_ID_PREFIX: usize = CLIENT_ID_PREFIX.len() + 4;

/// A paired client with all its pairings, as `remote clients` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Paired {
    id: String,
    /// The name of its most recently used pairing.
    name: String,
    /// When its first pairing was made.
    paired_at: i64,
    last_seen: Option<i64>,
    pairings: usize,
}

/// `pairings` grouped by client, in the order of each client's first pairing.
fn by_client(pairings: &[RemotePairing]) -> Vec<Paired> {
    let mut clients: Vec<Paired> = Vec::new();
    for pairing in pairings {
        let recent = pairing.last_seen.unwrap_or(pairing.paired_at);
        match clients.iter_mut().find(|c| c.id == pairing.client_id) {
            Some(client) => {
                if recent > client.last_seen.unwrap_or(client.paired_at) {
                    client.name.clone_from(&pairing.name);
                }
                client.paired_at = client.paired_at.min(pairing.paired_at);
                client.last_seen = client.last_seen.max(pairing.last_seen);
                client.pairings += 1;
            }
            None => clients.push(Paired {
                id: pairing.client_id.clone(),
                name: pairing.name.clone(),
                paired_at: pairing.paired_at,
                last_seen: pairing.last_seen,
                pairings: 1,
            }),
        }
    }
    clients
}

impl Inner {
    /// `remote.pair`: a new one-time code, or `pairing_paused` while wrong
    /// codes refuse every code. The code is never logged.
    pub(crate) fn pair_method(&self) -> Result<Value, RpcError> {
        let minted = lock(&self.remote.codes).mint(Instant::now());
        let code = match minted {
            Ok(code) => code,
            Err(MintError::Paused(wait)) => return Err(paused(wait)),
            Err(MintError::Random(error)) => return Err(random_error(error)),
        };
        let expires_at_ms = now_ms().saturating_add(PAIRING_CODE_TTL_MS);
        tracing::info!(expires_at_ms, "issued a pairing code");
        Ok(json!({"code": code, "expires_at_ms": expires_at_ms}))
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
        Ok(json!({"token": token}))
    }

    /// `remote.clients`: every paired client and every client a tunnel serves
    /// now, by name, with its connections (address, since when, and what it
    /// mirrors: `{ports, bound, skipped_lanes}`) and how many pairings it has.
    pub(crate) async fn clients_method(&self) -> Result<Value, RpcError> {
        let pairings = self.store.remote_pairings().await.map_err(store_error)?;
        let live: Vec<(ClientKey, RemoteClient)> = lock(&self.remote.clients)
            .iter()
            .map(|(key, (_, client))| (key.clone(), client.clone()))
            .collect();
        let mut items: Vec<Value> = by_client(&pairings)
            .into_iter()
            .map(|client| {
                json!({
                    "id": client.id,
                    "name": client.name,
                    "paired": true,
                    "pairings": client.pairings,
                    "paired_at_ms": client.paired_at,
                    "last_seen_ms": client.last_seen,
                    "connections": [],
                })
            })
            .collect();
        for (key, client) in live {
            let connection = json!({
                "address": client_address(client.ssh.as_deref()),
                "since_ms": client.since_ms,
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
        items.sort_by(|a, b| {
            let key = |item: &Value| {
                (
                    item["name"].as_str().unwrap_or_default().to_owned(),
                    item["id"].as_str().unwrap_or_default().to_owned(),
                )
            };
            key(a).cmp(&key(b))
        });
        Ok(json!({"items": items}))
    }

    /// `remote.revoke`: deletes every pairing of the client named by its id,
    /// its name or a unique start of its id, then ends its sessions, in that
    /// order: a feed that starts meanwhile finds the pairing gone
    /// ([`RegistryParams::paired`](devlanes_proto::remote::RegistryParams::paired)).
    /// A client connected without pairing cannot be revoked; the answer says
    /// so (`unpaired`).
    pub(crate) async fn revoke_method(&self, value: Value) -> Result<Value, RpcError> {
        let args: RevokeParams = required(value)?;
        let target = args.client.trim().to_owned();
        let pairings = self.store.remote_pairings().await.map_err(store_error)?;
        let clients = by_client(&pairings);
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
        self.store
            .delete_remote_pairings(ids.clone())
            .await
            .map_err(store_error)?;
        let ended = self
            .remote
            .end_feeds(|id| ids.iter().any(|revoked| revoked == id));
        for client in &found {
            tracing::info!(client_id = %client.id, name = %client.name, ended, "revoked a client's pairings");
        }
        let revoked: Vec<Value> = found
            .iter()
            .map(|client| json!({"id": client.id, "name": client.name}))
            .collect();
        Ok(json!({"revoked": revoked, "ended": ended}))
    }

    /// After `daemon reset-state` deleted every pairing: voids the
    /// outstanding codes and ends every client's session, so no client keeps
    /// a box that no longer knows it. Recent failed attempts still count.
    pub(crate) fn forget_pairings(&self) {
        lock(&self.remote.codes).void();
        let ended = self.remote.end_feeds(|_| true);
        if ended > 0 {
            tracing::info!(ended, "ended every client's session after a state reset");
        }
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
            PairingReason::Missing => {
                "the box serves only paired clients; pairing code from `devlanes remote pair` on the box:"
            }
            PairingReason::Invalid => {
                "the box no longer accepts this machine's pairing; new pairing code from `devlanes remote pair` on the box:"
            }
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

    /// A client's pairings make one entry: the name of the one used last,
    /// the first pairing time, the last connection and their count.
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
        let clients = by_client(&[
            pairing("dl_cid_a", "laptop", 5, Some(9)),
            pairing("dl_cid_a", "old-laptop", 2, Some(3)),
            pairing("dl_cid_b", "desk", 4, None),
        ]);
        assert_eq!(
            clients,
            [
                Paired {
                    id: "dl_cid_a".into(),
                    name: "laptop".into(),
                    paired_at: 2,
                    last_seen: Some(9),
                    pairings: 2,
                },
                Paired {
                    id: "dl_cid_b".into(),
                    name: "desk".into(),
                    paired_at: 4,
                    last_seen: None,
                    pairings: 1,
                },
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
