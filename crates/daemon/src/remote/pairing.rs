//! Pairing. On a box: one-time codes for `devlanes remote pair`, kept in
//! memory with the failed attempts that limit them, the remote tokens they
//! are traded for (only their SHA-256 is stored, in `remote_pairings`), and
//! `remote clients` and `remote revoke`. On a client: the token each box gave
//! this machine (`remote_tokens`) and the code a person types.
//!
//! Codes are 8 characters of Crockford's base32 alphabet (no I, L, O or U),
//! shown as `XXXX-XXXX`: 40 bits, valid once for 10 minutes. Every failed
//! attempt voids every outstanding code, so a guesser gets one try per code,
//! and five failures within 10 minutes refuse every code until the oldest of
//! them is 10 minutes old. Client ids and tokens are a prefix and 256 random
//! bits as lowercase hex. Codes and token hashes are compared in constant time.

use std::{collections::VecDeque, io, sync::Arc};

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
use serde_json::{Map, Value, json};
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
/// Failed attempts within [`FAILURE_WINDOW_MS`] after which every code is refused.
pub(crate) const MAX_FAILURES: usize = 5;
/// How long a failed attempt counts against [`MAX_FAILURES`].
pub(crate) const FAILURE_WINDOW_MS: u64 = 10 * 60 * 1000;
/// Outstanding codes kept at most; minting another drops the oldest.
const MAX_CODES: usize = 16;

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
    expires_ms: u64,
}

/// Why a code was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Wrong, expired or used; every outstanding code is void now.
    Wrong,
    /// Too many failures; every code is refused for `retry_after_ms`.
    Locked { retry_after_ms: u64 },
}

/// The box's outstanding codes and recent failures, in memory only. Every
/// method takes the time, so tests choose it.
#[derive(Default)]
pub(crate) struct Codes {
    codes: Vec<Code>,
    /// Times of failed attempts within [`FAILURE_WINDOW_MS`], oldest first.
    failures: VecDeque<u64>,
}

impl Codes {
    /// A new code, valid until the returned time (Unix ms).
    pub(crate) fn mint(&mut self, now_ms: u64) -> io::Result<(String, u64)> {
        self.prune(now_ms);
        let mut bytes = [0u8; CODE_LEN];
        getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
        // 32 divides 256, so masking keeps every character equally likely.
        let value = bytes.map(|byte| ALPHABET[usize::from(byte & 31)]);
        let expires_ms = now_ms.saturating_add(PAIRING_CODE_TTL_MS);
        if self.codes.len() >= MAX_CODES {
            self.codes.remove(0);
        }
        self.codes.push(Code { value, expires_ms });
        Ok((display(&value), expires_ms))
    }

    /// Uses `input` up when it matches an outstanding code. A failure voids
    /// every outstanding code and counts towards the limit; while the limit
    /// holds, every code is refused without being compared.
    pub(crate) fn claim(&mut self, input: &str, now_ms: u64) -> Result<(), Refusal> {
        self.prune(now_ms);
        if let Some(retry_after_ms) = self.locked_for(now_ms) {
            return Err(Refusal::Locked { retry_after_ms });
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
                self.failures.push_back(now_ms);
                Err(Refusal::Wrong)
            }
        }
    }

    /// How long every code stays refused, when failures reached the limit.
    pub(crate) fn locked_for(&mut self, now_ms: u64) -> Option<u64> {
        self.prune(now_ms);
        if self.failures.len() < MAX_FAILURES {
            return None;
        }
        let oldest = self.failures.front()?;
        Some(
            oldest
                .saturating_add(FAILURE_WINDOW_MS)
                .saturating_sub(now_ms),
        )
    }

    /// Whether a code is still valid, which keeps an idle daemon running so
    /// the code does not vanish with it.
    pub(crate) fn outstanding(&mut self, now_ms: u64) -> bool {
        self.prune(now_ms);
        !self.codes.is_empty()
    }

    /// Forgets expired codes and failures older than the window.
    fn prune(&mut self, now_ms: u64) {
        self.codes.retain(|code| code.expires_ms > now_ms);
        while self
            .failures
            .front()
            .is_some_and(|at| at.saturating_add(FAILURE_WINDOW_MS) <= now_ms)
        {
            self.failures.pop_front();
        }
    }
}

/// `pairing_refused`, which the tunnel hands to the client as the reason.
fn refused(refusal: Refusal) -> RpcError {
    let (message, retry_after_ms) = match refusal {
        Refusal::Wrong => (
            "the pairing code is wrong, expired or already used, and every other code from \
             `devlanes remote pair` is void now; run `devlanes remote pair` on the box for a new one"
                .to_owned(),
            None,
        ),
        Refusal::Locked { retry_after_ms } => (
            format!(
                "too many wrong pairing codes; the box takes codes again in {}, then run \
                 `devlanes remote pair` there for a new one",
                minutes(retry_after_ms)
            ),
            Some(retry_after_ms),
        ),
    };
    RpcError::new(
        "pairing_refused",
        json!({"message": message, "retry_after_ms": retry_after_ms}),
        4,
    )
}

/// `retry_after_ms` as whole minutes, rounded up: `1 minute`, `7 minutes`.
fn minutes(ms: u64) -> String {
    match ms.div_ceil(60_000).max(1) {
        1 => "1 minute".to_owned(),
        count => format!("{count} minutes"),
    }
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

impl Inner {
    /// `remote.pair`: a new one-time code. The code is never logged.
    pub(crate) fn pair_method(&self) -> Result<Value, RpcError> {
        let now = now_ms();
        let mut codes = lock(&self.remote.codes);
        let (code, expires_at_ms) = codes.mint(now).map_err(random_error)?;
        let locked = codes.locked_for(now);
        drop(codes);
        tracing::info!(expires_at_ms, "issued a pairing code");
        let mut reply = Map::new();
        reply.insert("code".into(), json!(code));
        reply.insert("expires_at_ms".into(), json!(expires_at_ms));
        if let Some(retry_after_ms) = locked {
            reply.insert("retry_after_ms".into(), json!(retry_after_ms));
        }
        Ok(Value::Object(reply))
    }

    /// `remote.pairing.verify`: whether a client's token is the one its
    /// pairing was made with; an accepted one updates the pairing's name and
    /// last seen time. A refused token is logged with the client's id and
    /// address. The hash is compared in constant time, also against a
    /// placeholder when the client has no pairing.
    pub(crate) async fn verify_pairing(&self, value: Value) -> Result<Value, RpcError> {
        let args: VerifyParams = required(value)?;
        let Some(token) = args.token else {
            return Ok(json!({"paired": false, "reason": PairingReason::Missing}));
        };
        let id = clean(&args.id);
        let stored = self
            .store
            .remote_pairing(id.clone())
            .await
            .map_err(store_error)?;
        let presented = token_hash(&token);
        let expected = stored
            .as_ref()
            .map_or_else(|| "0".repeat(presented.len()), |p| p.token_sha256.clone());
        let matches = same(presented.as_bytes(), expected.as_bytes()) && stored.is_some();
        let name = clean(&args.name);
        let address = args.address.as_deref().map(clean);
        if !matches {
            tracing::warn!(client_id = %id, %name, address = ?address, "refused a client's remote token");
            return Ok(json!({"paired": false, "reason": PairingReason::Invalid}));
        }
        let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        if let Err(error) = self.store.touch_remote_pairing(id.clone(), name, at).await {
            tracing::warn!(%error, client_id = %id, "cannot record when a client was last seen");
        }
        Ok(json!({"paired": true}))
    }

    /// `remote.pairing.claim`: trades a one-time code for a remote token and
    /// stores the pairing, replacing an earlier one of the same client. A
    /// refused code is logged with the client's id and address.
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
        let now = now_ms();
        let outcome = lock(&self.remote.codes).claim(&args.code, now);
        if let Err(refusal) = outcome {
            match refusal {
                Refusal::Wrong => tracing::warn!(
                    client_id = %id, %name, address = ?address,
                    "refused a pairing code; every outstanding code is void now"
                ),
                Refusal::Locked { retry_after_ms } => tracing::warn!(
                    client_id = %id, %name, address = ?address, retry_after_ms,
                    "refused a pairing code after too many failures"
                ),
            }
            return Err(refused(refusal));
        }
        let token = random_hex(TOKEN_PREFIX).map_err(random_error)?;
        let at = i64::try_from(now).unwrap_or(i64::MAX);
        self.store
            .upsert_remote_pairing(RemotePairing {
                client_id: id.clone(),
                name: name.clone(),
                token_sha256: token_hash(&token),
                paired_at: at,
                last_seen: Some(at),
            })
            .await
            .map_err(store_error)?;
        tracing::info!(client_id = %id, %name, address = ?address, "paired a client");
        Ok(json!({"token": token}))
    }

    /// `remote.clients`: every paired client and every client a tunnel serves
    /// now, by name, with its connections (address, since when, and what it
    /// mirrors: `{ports, bound, skipped_lanes}`).
    pub(crate) async fn clients_method(&self) -> Result<Value, RpcError> {
        let pairings = self.store.remote_pairings().await.map_err(store_error)?;
        let live: Vec<(ClientKey, RemoteClient)> = lock(&self.remote.clients)
            .iter()
            .map(|(key, (_, client))| (key.clone(), client.clone()))
            .collect();
        let mut items: Vec<Value> = pairings
            .iter()
            .map(|pairing| {
                json!({
                    "id": pairing.client_id,
                    "name": pairing.name,
                    "paired": true,
                    "paired_at_ms": pairing.paired_at,
                    "last_seen_ms": pairing.last_seen,
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

    /// `remote.revoke`: deletes the pairing of the client named by its id,
    /// its name or a unique start of its id, then ends its sessions, in that
    /// order: a feed that starts meanwhile finds the pairing gone
    /// ([`RegistryParams::paired`](devlanes_proto::remote::RegistryParams::paired)).
    pub(crate) async fn revoke_method(&self, value: Value) -> Result<Value, RpcError> {
        let args: RevokeParams = required(value)?;
        let target = args.client.trim().to_owned();
        let pairings = self.store.remote_pairings().await.map_err(store_error)?;
        let found = matching(&pairings, &target);
        if found.is_empty() {
            let all: Vec<&RemotePairing> = pairings.iter().collect();
            return Err(RpcError::new(
                "client_not_found",
                json!({"client": target, "candidates": candidates(&all)}),
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
        let ids: Vec<String> = found.iter().map(|p| p.client_id.clone()).collect();
        self.store
            .delete_remote_pairings(ids.clone())
            .await
            .map_err(store_error)?;
        let mut ended = 0;
        for (id, revoked) in lock(&self.remote.feeds).values() {
            if ids.contains(id) {
                revoked.cancel();
                ended += 1;
            }
        }
        for pairing in &found {
            tracing::info!(client_id = %pairing.client_id, name = %pairing.name, ended, "revoked a client's pairing");
        }
        let revoked: Vec<Value> = found
            .iter()
            .map(|p| json!({"id": p.client_id, "name": p.name}))
            .collect();
        Ok(json!({"revoked": revoked, "ended": ended}))
    }
}

/// The pairings `target` names: the one with that id, else those with that
/// name, else those whose id starts with it (at least [`MIN_ID_PREFIX`] long).
fn matching<'a>(pairings: &'a [RemotePairing], target: &str) -> Vec<&'a RemotePairing> {
    if let Some(exact) = pairings.iter().find(|p| p.client_id == target) {
        return vec![exact];
    }
    let named: Vec<&RemotePairing> = pairings.iter().filter(|p| p.name == target).collect();
    if !named.is_empty() || target.len() < MIN_ID_PREFIX {
        return named;
    }
    pairings
        .iter()
        .filter(|p| p.client_id.starts_with(target))
        .collect()
}

/// `{id, name}` of each pairing, for a `client_not_found` or
/// `client_ambiguous` answer.
fn candidates(pairings: &[&RemotePairing]) -> Vec<Value> {
    pairings
        .iter()
        .map(|p| json!({"id": p.client_id, "name": p.name}))
        .collect()
}

/// Client side: the token this machine keeps for one box, and the person who
/// types the box's code, through `remote connect --code` or a prompt to the
/// attached `remote connect`.
pub(super) struct HostPairing {
    pub(super) host: String,
    pub(super) store: Store,
    pub(super) prompter: Arc<dyn RemotePrompter>,
    /// A code from `remote connect --code`, used by the next pairing only.
    pub(super) code: Arc<std::sync::Mutex<Option<String>>>,
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

    /// The code from `--code`, else one asked of the attached `remote connect`.
    async fn code(&self, reason: PairingReason) -> Option<String> {
        if let Some(code) = lock(&self.code).take() {
            return Some(code);
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

    const MINUTE: u64 = 60_000;

    /// A code works once, in any case and with or without its dash, and not
    /// after it expired.
    #[test]
    fn a_code_is_used_once_and_expires() -> io::Result<()> {
        let mut codes = Codes::default();
        let (code, expires) = codes.mint(0)?;
        assert_eq!(expires, PAIRING_CODE_TTL_MS);
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(codes.outstanding(0));
        let typed = code.replace('-', " ").to_lowercase();
        assert_eq!(codes.claim(&typed, MINUTE), Ok(()));
        assert!(!codes.outstanding(MINUTE));
        assert_eq!(codes.claim(&code, MINUTE), Err(Refusal::Wrong), "used up");

        let (late, _) = codes.mint(0)?;
        assert!(codes.outstanding(PAIRING_CODE_TTL_MS - 1));
        assert!(!codes.outstanding(PAIRING_CODE_TTL_MS));
        assert_eq!(
            codes.claim(&late, PAIRING_CODE_TTL_MS),
            Err(Refusal::Wrong),
            "expired"
        );
        Ok(())
    }

    /// One wrong code voids every outstanding code, including valid ones.
    #[test]
    fn a_failure_voids_every_code() -> io::Result<()> {
        let mut codes = Codes::default();
        let (first, _) = codes.mint(0)?;
        let (second, _) = codes.mint(0)?;
        assert_eq!(codes.claim("not-a-code", 1), Err(Refusal::Wrong));
        assert!(!codes.outstanding(1));
        assert_eq!(codes.claim(&first, 2), Err(Refusal::Wrong));
        assert_eq!(codes.claim(&second, 3), Err(Refusal::Wrong));
        Ok(())
    }

    /// Five failures within ten minutes refuse every code, a valid one too,
    /// with the time until the oldest failure is ten minutes old; then codes
    /// work again.
    #[test]
    fn failures_lock_out_codes_for_a_while() -> io::Result<()> {
        let mut codes = Codes::default();
        for minute in 0..MAX_FAILURES as u64 {
            assert_eq!(
                codes.claim("0000-0000", minute * MINUTE),
                Err(Refusal::Wrong)
            );
        }
        assert_eq!(codes.locked_for(5 * MINUTE), Some(5 * MINUTE));
        let (code, _) = codes.mint(5 * MINUTE)?;
        assert_eq!(
            codes.claim(&code, 6 * MINUTE),
            Err(Refusal::Locked {
                retry_after_ms: 4 * MINUTE
            })
        );
        assert!(
            codes.outstanding(6 * MINUTE),
            "a locked-out claim voids nothing"
        );
        // The oldest failure leaves the window at minute 10.
        assert_eq!(codes.locked_for(10 * MINUTE), None);
        assert_eq!(codes.claim(&code, 10 * MINUTE), Ok(()));
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

    /// `revoke` takes an exact id first, then a name, then a long enough
    /// start of an id; two clients with the same name stay ambiguous.
    #[test]
    fn revoke_names_clients_by_id_name_or_id_start() {
        let pairing = |id: &str, name: &str| RemotePairing {
            client_id: id.into(),
            name: name.into(),
            token_sha256: String::new(),
            paired_at: 0,
            last_seen: None,
        };
        let pairings = [
            pairing("dl_cid_aaaa1111", "laptop"),
            pairing("dl_cid_aaaa2222", "laptop"),
            pairing("dl_cid_bbbb3333", "desk"),
        ];
        let ids = |target: &str| -> Vec<String> {
            matching(&pairings, target)
                .iter()
                .map(|p| p.client_id.clone())
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

    #[test]
    fn waits_are_rounded_up_to_minutes() {
        assert_eq!(minutes(1), "1 minute");
        assert_eq!(minutes(60_001), "2 minutes");
        assert_eq!(minutes(10 * 60_000), "10 minutes");
    }
}
