//! Machines connected to this one as a box, as `remote.clients` lists them:
//! the clients paired with it and the ones a tunnel serves now. The boxes
//! view shows them and revokes a pairing.

use serde::Deserialize;
use serde_json::Value;

/// What `remote.clients` answered, with `remote.require_pairing`,
/// `remote.allow_connections` and whether the remote settings are invalid,
/// from this machine's user config when the TUI could read it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Clients {
    pub items: Vec<Client>,
    /// The daemon's clock when it answered, which the times count from.
    pub now_ms: Option<u64>,
    /// `false` when tunnels here serve clients without pairing.
    pub require_pairing: Option<bool>,
    /// `false` when tunnels here turn every client away.
    pub allow_connections: Option<bool>,
    /// True when invalid remote settings in config.yaml turned connections
    /// off.
    pub remote_invalid: bool,
}

/// One client by id: its pairings and its live connections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Client {
    /// `dl_cid_…`; empty for a client of a release without ids.
    pub id: String,
    pub name: String,
    pub paired: bool,
    /// How many pairings it has: one per box name it paired under.
    pub pairings: u64,
    pub paired_at_ms: Option<u64>,
    pub last_seen_ms: Option<u64>,
    pub connections: Vec<Connection>,
}

/// A tunnel serving the client now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Connection {
    /// The client's address from `$SSH_CONNECTION`; `None` over a command.
    pub address: Option<String>,
    pub since_ms: Option<u64>,
    /// What the client last reported mirroring; `None` before its first
    /// report and from clients that send none.
    pub mirror: Option<Mirror>,
}

/// The counts of a client's mirror report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Mirror {
    pub ports: u64,
    pub bound: u64,
    pub skipped_lanes: u64,
}

/// Characters of a client id the TUI shows and `daemon revoke` takes: the
/// prefix and 8 hex digits, as `devlanes daemon clients` prints it.
pub fn short_id(id: &str) -> String {
    let keep = devlanes_proto::remote::CLIENT_ID_PREFIX.len() + 8;
    id.chars().take(keep).collect()
}

/// Parses `remote.clients` leniently: an item this client cannot read is
/// left out instead of hiding every client.
pub fn parse(value: &Value) -> Clients {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<Client>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    Clients {
        items,
        now_ms: value.get("now_ms").and_then(Value::as_u64),
        require_pairing: value.get("require_pairing").and_then(Value::as_bool),
        allow_connections: value.get("allow_connections").and_then(Value::as_bool),
        remote_invalid: value.get("remote_invalid").and_then(Value::as_bool) == Some(true),
    }
}

impl Clients {
    /// How many clients have a tunnel open now.
    pub fn connected(&self) -> usize {
        self.items.iter().filter(|c| c.connected()).count()
    }

    /// How many clients are paired.
    pub fn paired(&self) -> usize {
        self.items.iter().filter(|c| c.paired).count()
    }
}

impl Client {
    /// Whether a tunnel serves it now.
    pub fn connected(&self) -> bool {
        !self.connections.is_empty()
    }

    /// Whether `remote.revoke` can take it: only a pairing can be revoked.
    pub fn revocable(&self) -> bool {
        self.paired && !self.id.is_empty()
    }

    /// When its oldest live connection began.
    pub fn connected_since(&self) -> Option<u64> {
        self.connections.iter().filter_map(|c| c.since_ms).min()
    }

    /// The first live connection's address.
    pub fn address(&self) -> Option<&str> {
        self.connections.iter().find_map(|c| c.address.as_deref())
    }

    /// The first mirror report among its connections.
    pub fn mirror(&self) -> Option<&Mirror> {
        self.connections.iter().find_map(|c| c.mirror.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Unreadable items are left out; the rest keep their fields, and a
    /// reply without the TUI's addition has no `require_pairing`.
    #[test]
    fn remote_clients_parse_leniently() {
        let parsed = parse(&json!({
            "now_ms": 5_000,
            "items": [
                {"id": "dl_cid_0123456789abcdef", "name": "laptop", "paired": true, "pairings": 2,
                 "paired_at_ms": 1_000, "last_seen_ms": 4_000,
                 "connections": [
                     {"address": "203.0.113.4", "since_ms": 3_000,
                      "mirror": {"ports": 6, "bound": 5, "skipped_lanes": 1}},
                     {"address": null, "since_ms": 2_000, "mirror": null}
                 ]},
                {"id": "", "name": "old", "paired": false, "pairings": 0,
                 "connections": [{"address": "198.51.100.7", "since_ms": 4_500}]},
                {"name": 7}
            ]
        }));
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.now_ms, Some(5_000));
        assert_eq!(parsed.require_pairing, None);
        assert_eq!(parsed.allow_connections, None);
        assert_eq!((parsed.connected(), parsed.paired()), (2, 1));
        let laptop = &parsed.items[0];
        assert!(laptop.revocable());
        assert_eq!(laptop.connected_since(), Some(2_000));
        assert_eq!(laptop.address(), Some("203.0.113.4"));
        assert_eq!(laptop.mirror().map(|m| m.bound), Some(5));
        assert_eq!(short_id(&laptop.id), "dl_cid_01234567");
        assert!(!parsed.items[1].revocable());
        assert_eq!(parse(&json!({})), Clients::default());
    }
}
