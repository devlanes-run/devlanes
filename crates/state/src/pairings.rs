//! Pairing records: on a box, the clients it serves, and on a client, the
//! remote token each box gave it.

use crate::{Error, Store, models::*, schema::*};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

/// A client a box paired with (`devlanes remote pair` on the box, then the
/// client's first connection with the code). The token itself is never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePairing {
    /// The client's `dl_cid_` id.
    pub client_id: String,
    /// The client's host name when it last connected.
    pub name: String,
    /// Lowercase hex SHA-256 of the client's remote token.
    pub token_sha256: String,
    /// Unix milliseconds of the pairing.
    pub paired_at: i64,
    /// Unix milliseconds of the client's last accepted connection.
    pub last_seen: Option<i64>,
}

impl From<PairingRow> for RemotePairing {
    /// Copies the row.
    fn from(row: PairingRow) -> Self {
        Self {
            client_id: row.client_id,
            name: row.name,
            token_sha256: row.token_sha256,
            paired_at: row.paired_at,
            last_seen: row.last_seen,
        }
    }
}

impl Store {
    /// Stores `pairing`, replacing an earlier pairing of the same client id,
    /// whose token stops working.
    pub async fn upsert_remote_pairing(&self, pairing: RemotePairing) -> Result<(), Error> {
        if pairing.client_id.is_empty() {
            return Err(Error::Invalid("empty client id".into()));
        }
        self.call(move |c| {
            let row = PairingRow {
                client_id: pairing.client_id,
                name: pairing.name,
                token_sha256: pairing.token_sha256,
                paired_at: pairing.paired_at,
                last_seen: pairing.last_seen,
            };
            diesel::insert_into(remote_pairings::table)
                .values(&row)
                .on_conflict(remote_pairings::client_id)
                .do_update()
                .set(&row)
                .execute(c)?;
            Ok(())
        })
        .await
    }

    /// The pairing of `client_id`, if the box has one.
    pub async fn remote_pairing(&self, client_id: String) -> Result<Option<RemotePairing>, Error> {
        self.call(move |c| {
            Ok(remote_pairings::table
                .find(client_id)
                .select(PairingRow::as_select())
                .first::<PairingRow>(c)
                .optional()?
                .map(RemotePairing::from))
        })
        .await
    }

    /// Every pairing, by client name and then id.
    pub async fn remote_pairings(&self) -> Result<Vec<RemotePairing>, Error> {
        self.call(|c| {
            Ok(remote_pairings::table
                .order((remote_pairings::name, remote_pairings::client_id))
                .select(PairingRow::as_select())
                .load::<PairingRow>(c)?
                .into_iter()
                .map(RemotePairing::from)
                .collect())
        })
        .await
    }

    /// Records an accepted connection of `client_id` at `at` under `name`.
    /// Returns false when the client has no pairing.
    pub async fn touch_remote_pairing(
        &self,
        client_id: String,
        name: String,
        at: i64,
    ) -> Result<bool, Error> {
        self.call(move |c| {
            let changed = diesel::update(remote_pairings::table.find(client_id))
                .set((
                    remote_pairings::name.eq(name),
                    remote_pairings::last_seen.eq(Some(at)),
                ))
                .execute(c)?;
            Ok(changed > 0)
        })
        .await
    }

    /// Deletes the pairings of `client_ids`; returns how many existed.
    pub async fn delete_remote_pairings(&self, client_ids: Vec<String>) -> Result<usize, Error> {
        self.call(move |c| {
            Ok(diesel::delete(
                remote_pairings::table.filter(remote_pairings::client_id.eq_any(client_ids)),
            )
            .execute(c)?)
        })
        .await
    }

    /// The remote token box `host` gave this machine, if any.
    pub async fn remote_token(&self, host: String) -> Result<Option<String>, Error> {
        self.call(move |c| {
            Ok(remote_tokens::table
                .find(host)
                .select(remote_tokens::token)
                .first::<String>(c)
                .optional()?)
        })
        .await
    }

    /// Keeps `token` for box `host`, replacing an earlier one; `at` is when the
    /// box issued it, in Unix milliseconds.
    pub async fn set_remote_token(
        &self,
        host: String,
        token: String,
        at: i64,
    ) -> Result<(), Error> {
        if host.is_empty() {
            return Err(Error::Invalid("empty remote host name".into()));
        }
        self.call(move |c| {
            let row = TokenRow {
                host,
                token,
                paired_at: at,
            };
            diesel::insert_into(remote_tokens::table)
                .values(&row)
                .on_conflict(remote_tokens::host)
                .do_update()
                .set(&row)
                .execute(c)?;
            Ok(())
        })
        .await
    }
}
