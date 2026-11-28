//! Peer-to-peer pairings: on a box, the clients paired by ticket, pinned by
//! the key they proved in the handshake.

use crate::{Error, Store, models::*, schema::*};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

/// One key a box paired with by ticket, and the client it was paired for.
/// One row per key: pairing the key again replaces its row, also under
/// another client id. A client keeps several keys, up to the number the
/// caller of [`Store::add_p2p_client`] keeps; a pairing never replaces
/// another key's row, so claiming another client's id evicts nobody.
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
    /// Pins `client`'s key, replacing the row of the same key, and keeps at
    /// most `keep` keys of its client id, the most recently paired, in one
    /// transaction; the key just paired is always kept.
    /// Returns the keys it dropped to stay within that, which the caller
    /// stops admitting.
    pub async fn add_p2p_client(
        &self,
        client: P2pClient,
        keep: usize,
    ) -> Result<Vec<String>, Error> {
        if client.client_id.is_empty() || client.peer_key.is_empty() {
            return Err(Error::Invalid("empty client id or peer key".into()));
        }
        self.call(move |c| {
            c.immediate_transaction(|c| {
                let client_id = client.client_id.clone();
                let row = P2pClientRow {
                    peer_key: client.peer_key,
                    client_id: client.client_id,
                    name: client.name,
                    paired_at: client.paired_at,
                    last_seen: client.last_seen,
                };
                // Deleted and inserted rather than updated, so the key's
                // new row is the newest by rowid.
                diesel::delete(p2p_clients::table.find(&row.peer_key)).execute(c)?;
                diesel::insert_into(p2p_clients::table)
                    .values(&row)
                    .execute(c)?;
                // Insertion order is pairing order. `paired_at` is the wall
                // clock, which may step back, and ordering by it could drop
                // the key just paired, which the caller pins anyway. This
                // relies on SQLite giving a new row the highest rowid in a
                // table without AUTOINCREMENT; the daemon only runs
                // `incremental_vacuum`, which keeps rowids.
                let dropped: Vec<String> = p2p_clients::table
                    .filter(p2p_clients::client_id.eq(&client_id))
                    .order(diesel::dsl::sql::<diesel::sql_types::BigInt>("rowid DESC"))
                    .offset(i64::try_from(keep).unwrap_or(i64::MAX))
                    .select(p2p_clients::peer_key)
                    .load(c)?;
                diesel::delete(p2p_clients::table.filter(p2p_clients::peer_key.eq_any(&dropped)))
                    .execute(c)?;
                Ok(dropped)
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

    /// Deletes every pairing of `client_ids`, by token and by ticket, in one
    /// transaction, so a revoke never removes one kind and keeps the other;
    /// returns how many of each there were.
    pub async fn delete_client_pairings(
        &self,
        client_ids: Vec<String>,
    ) -> Result<(usize, usize), Error> {
        self.call(move |c| {
            c.immediate_transaction(|c| {
                let tokens = diesel::delete(
                    remote_pairings::table.filter(remote_pairings::client_id.eq_any(&client_ids)),
                )
                .execute(c)?;
                let keys = diesel::delete(
                    p2p_clients::table.filter(p2p_clients::client_id.eq_any(&client_ids)),
                )
                .execute(c)?;
                Ok((tokens, keys))
            })
        })
        .await
    }
}
