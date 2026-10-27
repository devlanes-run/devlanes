//! Pairing records: on a box, the clients it serves, and on a client, the
//! remote token each box gave it.

use crate::{Error, Store, models::*, schema::*};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

/// One pairing of a client with a box (`devlanes remote pair` on the box,
/// then the client's connection with the code). A client paired under several
/// box names has one per name. The token itself is never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePairing {
    /// The client's `dl_cid_` id.
    pub client_id: String,
    /// The client's host name when it last connected with this pairing.
    pub name: String,
    /// Lowercase hex SHA-256 of the pairing's remote token.
    pub token_sha256: String,
    /// Unix milliseconds of the pairing.
    pub paired_at: i64,
    /// Unix milliseconds of the last connection accepted with this pairing.
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
    /// Adds `pairing` and keeps at most `keep` pairings of its client, newest
    /// first; returns the ones it dropped to stay within that.
    pub async fn add_remote_pairing(
        &self,
        pairing: RemotePairing,
        keep: usize,
    ) -> Result<Vec<RemotePairing>, Error> {
        if pairing.client_id.is_empty() {
            return Err(Error::Invalid("empty client id".into()));
        }
        self.call(move |c| {
            c.immediate_transaction(|c| {
                let client_id = pairing.client_id.clone();
                let row = PairingRow {
                    token_sha256: pairing.token_sha256,
                    client_id: pairing.client_id,
                    name: pairing.name,
                    paired_at: pairing.paired_at,
                    last_seen: pairing.last_seen,
                };
                diesel::insert_into(remote_pairings::table)
                    .values(&row)
                    .execute(c)?;
                let dropped: Vec<PairingRow> = remote_pairings::table
                    .filter(remote_pairings::client_id.eq(&client_id))
                    .order((
                        remote_pairings::paired_at.desc(),
                        remote_pairings::token_sha256,
                    ))
                    .offset(i64::try_from(keep).unwrap_or(i64::MAX))
                    .select(PairingRow::as_select())
                    .load(c)?;
                let hashes: Vec<String> =
                    dropped.iter().map(|row| row.token_sha256.clone()).collect();
                diesel::delete(
                    remote_pairings::table.filter(remote_pairings::token_sha256.eq_any(hashes)),
                )
                .execute(c)?;
                Ok(dropped.into_iter().map(RemotePairing::from).collect())
            })
        })
        .await
    }

    /// Every pairing of `client_id`.
    pub async fn remote_pairings_of(&self, client_id: String) -> Result<Vec<RemotePairing>, Error> {
        self.call(move |c| {
            Ok(remote_pairings::table
                .filter(remote_pairings::client_id.eq(client_id))
                .order(remote_pairings::token_sha256)
                .select(PairingRow::as_select())
                .load::<PairingRow>(c)?
                .into_iter()
                .map(RemotePairing::from)
                .collect())
        })
        .await
    }

    /// Every pairing, by client name, then client id, then when it was made.
    pub async fn remote_pairings(&self) -> Result<Vec<RemotePairing>, Error> {
        self.call(|c| {
            Ok(remote_pairings::table
                .order((
                    remote_pairings::name,
                    remote_pairings::client_id,
                    remote_pairings::paired_at,
                ))
                .select(PairingRow::as_select())
                .load::<PairingRow>(c)?
                .into_iter()
                .map(RemotePairing::from)
                .collect())
        })
        .await
    }

    /// Records a connection accepted at `at` with the pairing whose token
    /// hash is `token_sha256`, from a client now called `name`. Returns false
    /// when no pairing has that hash.
    pub async fn touch_remote_pairing(
        &self,
        token_sha256: String,
        name: String,
        at: i64,
    ) -> Result<bool, Error> {
        self.call(move |c| {
            let changed = diesel::update(remote_pairings::table.find(token_sha256))
                .set((
                    remote_pairings::name.eq(name),
                    remote_pairings::last_seen.eq(Some(at)),
                ))
                .execute(c)?;
            Ok(changed > 0)
        })
        .await
    }

    /// Deletes every pairing of `client_ids`; returns how many there were.
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
