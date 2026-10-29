//! Lane choices on a client: which connected box serves a lane reference that
//! several boxes run (`devlanes remote use`).

use crate::{Error, Store, models::*, schema::*};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

/// The box chosen to serve one lane reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLaneChoice {
    /// The lane reference, `project@index`.
    pub lane: String,
    /// The box's name, as `remote connect` got it.
    pub host: String,
    /// Unix milliseconds of the choice.
    pub chosen_at: i64,
}

impl From<LaneChoiceRow> for RemoteLaneChoice {
    /// Copies the row.
    fn from(row: LaneChoiceRow) -> Self {
        Self {
            lane: row.lane,
            host: row.host,
            chosen_at: row.chosen_at,
        }
    }
}

impl Store {
    /// Every lane choice, by lane.
    pub async fn remote_lane_choices(&self) -> Result<Vec<RemoteLaneChoice>, Error> {
        self.call(|c| {
            Ok(remote_lane_choices::table
                .order(remote_lane_choices::lane)
                .select(LaneChoiceRow::as_select())
                .load::<LaneChoiceRow>(c)?
                .into_iter()
                .map(RemoteLaneChoice::from)
                .collect())
        })
        .await
    }

    /// Keeps `choice`, replacing the lane's earlier one.
    pub async fn set_remote_lane_choice(&self, choice: RemoteLaneChoice) -> Result<(), Error> {
        if choice.lane.is_empty() || choice.host.is_empty() {
            return Err(Error::Invalid("empty lane or host of a lane choice".into()));
        }
        self.call(move |c| {
            let row = LaneChoiceRow {
                lane: choice.lane,
                host: choice.host,
                chosen_at: choice.chosen_at,
            };
            diesel::insert_into(remote_lane_choices::table)
                .values(&row)
                .on_conflict(remote_lane_choices::lane)
                .do_update()
                .set(&row)
                .execute(c)?;
            Ok(())
        })
        .await
    }

    /// Deletes the choice for `lane`, only when it names `host` if one is
    /// given. Returns false when there was no such choice.
    pub async fn delete_remote_lane_choice(
        &self,
        lane: String,
        host: Option<String>,
    ) -> Result<bool, Error> {
        self.call(move |c| {
            let mut query = diesel::delete(remote_lane_choices::table)
                .filter(remote_lane_choices::lane.eq(lane))
                .into_boxed();
            if let Some(host) = host {
                query = query.filter(remote_lane_choices::host.eq(host));
            }
            Ok(query.execute(c)? > 0)
        })
        .await
    }

    /// Deletes every choice of box `host` and returns how many there were.
    pub async fn delete_remote_lane_choices_of(&self, host: String) -> Result<usize, Error> {
        self.call(move |c| {
            Ok(diesel::delete(
                remote_lane_choices::table.filter(remote_lane_choices::host.eq(host)),
            )
            .execute(c)?)
        })
        .await
    }
}
