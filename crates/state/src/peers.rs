//! Peer-to-peer pairings: on a box, the clients paired by ticket, pinned by
//! the key they proved in the handshake.

use crate::{Error, Store, models::*, schema::*};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

/// A client a box paired with by ticket. One per client id: pairing again,
/// also with a new key, replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P2pClient {
    /// The client's peer key, 64 lowercase hex digits.
    pub peer_key: String,
    /// The client's `dl_cid_` id.
    pub client_id: String,
    /// The client's host name when it last paired or connected.
    pub name: String,
    /// Unix milliseconds of the pairing.
    pub paired_at: i64,
    /// Unix milliseconds of the last session the box served it.
    pub last_seen: Option<i64>,
}

impl From<P2pClientRow> for P2pClient {
    /// Copies the row.
    fn from(row: P2pClientRow) -> Self {
        Self {
            peer_key: row.peer_key,
            client_id: row.client_id,
            name: row.name,
            paired_at: row.paired_at,
            last_seen: row.last_seen,
        }
    }
}

impl Store {
    /// Pins `client`, replacing in one transaction any row with its client id
    /// or its key, so a client keeps one key and a key names one client.
    /// Returns the keys of the rows it replaced.
    pub async fn add_p2p_client(&self, client: P2pClient) -> Result<Vec<String>, Error> {
        if client.client_id.is_empty() || client.peer_key.is_empty() {
            return Err(Error::Invalid("empty client id or peer key".into()));
        }
        self.call(move |c| {
            c.immediate_transaction(|c| {
                let same = p2p_clients::client_id
                    .eq(&client.client_id)
                    .or(p2p_clients::peer_key.eq(&client.peer_key));
                let replaced: Vec<String> = p2p_clients::table
                    .filter(same)
                    .select(p2p_clients::peer_key)
                    .load(c)?;
                diesel::delete(p2p_clients::table.filter(same)).execute(c)?;
                diesel::insert_into(p2p_clients::table)
                    .values(P2pClientRow {
                        peer_key: client.peer_key,
                        client_id: client.client_id,
                        name: client.name,
                        paired_at: client.paired_at,
                        last_seen: client.last_seen,
                    })
                    .execute(c)?;
                Ok(replaced)
            })
        })
        .await
    }

    /// Every client paired by ticket, by name, then client id.
    pub async fn p2p_clients(&self) -> Result<Vec<P2pClient>, Error> {
        self.call(|c| {
            Ok(p2p_clients::table
                .order((p2p_clients::name, p2p_clients::client_id))
                .select(P2pClientRow::as_select())
                .load::<P2pClientRow>(c)?
                .into_iter()
                .map(P2pClient::from)
                .collect())
        })
        .await
    }

    /// Records a session served at `at` to the client with key `peer_key`,
    /// now called `name`. Returns false when no client has that key.
    pub async fn touch_p2p_client(
        &self,
        peer_key: String,
        name: String,
        at: i64,
    ) -> Result<bool, Error> {
        self.call(move |c| {
            let changed = diesel::update(p2p_clients::table.find(peer_key))
                .set((
                    p2p_clients::name.eq(name),
                    p2p_clients::last_seen.eq(Some(at)),
                ))
                .execute(c)?;
            Ok(changed > 0)
        })
        .await
    }

    /// Deletes the clients with `client_ids`; returns how many there were.
    pub async fn delete_p2p_clients(&self, client_ids: Vec<String>) -> Result<usize, Error> {
        self.call(move |c| {
            Ok(
                diesel::delete(
                    p2p_clients::table.filter(p2p_clients::client_id.eq_any(client_ids)),
                )
                .execute(c)?,
            )
        })
        .await
    }
}
