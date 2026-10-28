//! Lane choices and the assignment built from them. `remote.use` chooses the
//! box lane that serves hostnames several lanes clash on, or gives them back
//! to this machine's lane. [`Inner::reassign`] turns this machine's routes,
//! the connected boxes' registries and the choices into the
//! [`Assignment`] every mirror and the route publisher follow.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use devlanes_proto::{RpcError, remote as wire};
use devlanes_state::RemoteLaneChoice;
use serde_json::{Value, json};

use super::{
    Remotes,
    assign::{Assignment, BoxLanes, Cause, LocalLane, PortUse, assign},
    mirror, now_ms,
};
use crate::{
    engine::{Inner, required},
    integrations::RouteEntry,
    model::lock,
};

/// How long `remote.use` waits for the mirrors to show a choice in
/// `host.list`. Nothing in a reconcile waits for a box, so this is a bound
/// for a busy machine only.
const APPLY_WAIT: Duration = Duration::from_secs(2);

impl Remotes {
    /// The latest assignment.
    pub(crate) fn assignment(&self) -> Arc<Assignment> {
        lock(&self.assignment).1.clone()
    }
}

/// An error of `remote.use` with its plain `message`.
fn use_error(code: &str, mut data: Value, message: String, exit: u8) -> RpcError {
    if let Some(fields) = data.as_object_mut() {
        fields.insert("message".into(), json!(message));
    }
    RpcError::new(code, data, exit)
}

impl Inner {
    /// Reads the lane choices from the store, once.
    pub(crate) async fn load_choices(&self) {
        self.remote
            .choices_loaded
            .get_or_init(|| async {
                match self.store.remote_lane_choices().await {
                    Ok(rows) => {
                        let mut choices = lock(&self.remote.choices);
                        for row in rows {
                            choices.entry(row.lane).or_insert(row.host);
                        }
                    }
                    Err(error) => tracing::warn!(%error, "cannot read the lane choices"),
                }
            })
            .await;
    }

    /// Deletes every choice of box `host`, as `remote disconnect` does.
    pub(super) async fn forget_choices_of(&self, host: &str) {
        lock(&self.remote.choices).retain(|_, chosen| chosen != host);
        if let Err(error) = self
            .store
            .delete_remote_lane_choices_of(host.to_owned())
            .await
        {
            tracing::warn!(%error, %host, "a restarted daemon may still use this box's lane choices");
        }
    }

    /// [`Inner::reassign_with`] this machine's current routes.
    pub(crate) fn reassign(&self) -> (u64, Arc<Assignment>) {
        let local = self.local_route_entries();
        self.reassign_with(&local)
    }

    /// Recomputes the assignment from `local` (this machine's routes), the
    /// boxes whose link has not ended and the choices, and returns it with
    /// its version. A choice whose lane loses to a running local lane is
    /// deleted with a `choice_cleared` warning. When the result differs from
    /// the last one, every mirror is woken and the routes are published: the
    /// published routes follow the new assignment at once, before any mirror
    /// has reconciled.
    pub(crate) fn reassign_with(&self, local: &[RouteEntry]) -> (u64, Arc<Assignment>) {
        let computing = lock(&self.remote.assigning);
        let (lanes, boxes) = self.assign_inputs(local);
        let choices = lock(&self.remote.choices).clone();
        let mut next = assign(&lanes, &boxes, &choices);
        let mut cleared = Vec::new();
        if !next.cleared.is_empty() {
            let choices = {
                let mut stored = lock(&self.remote.choices);
                for (lane, host) in &next.cleared {
                    if stored.get(lane) == Some(host) {
                        stored.remove(lane);
                        let local = next
                            .skip(host, lane)
                            .map_or_else(|| lane.clone(), |skip| skip.winner_lane.clone());
                        cleared.push((lane.clone(), host.clone(), local));
                    }
                }
                stored.clone()
            };
            next = assign(&lanes, &boxes, &choices);
        }
        let next = Arc::new(next);
        let changed = {
            let mut current = lock(&self.remote.assignment);
            if *current.1 == *next {
                None
            } else {
                current.0 += 1;
                current.1 = next.clone();
                Some(current.0)
            }
        };
        let version = changed.unwrap_or_else(|| lock(&self.remote.assignment).0);
        if changed.is_some() {
            // Still under the lock: a listener a mirror binds right now is
            // pointed after this, at the owner this assignment names.
            self.remote
                .listeners
                .apply(&next, &|owner| self.remote.target(owner));
        }
        // Before waking anything: a woken mirror and the route publisher
        // reassign in turn, and this lock is not reentrant.
        drop(computing);
        self.announce_cleared(cleared);
        if changed.is_some() {
            for conn in self.remote.all() {
                conn.wake.notify_one();
                conn.mark_changed();
            }
            self.publish_routes();
        }
        (version, next)
    }

    /// The assignment's inputs: this machine's lanes from `local`, a lane in
    /// a start counted as running whatever it starts, and every box whose
    /// link has not ended, with the hostnames this machine would route and
    /// the ports of each of its lanes.
    fn assign_inputs(&self, local: &[RouteEntry]) -> (Vec<LocalLane>, Vec<BoxLanes>) {
        let running: BTreeMap<String, bool> = self
            .all_lanes()
            .iter()
            .map(|(project, lane)| {
                let st = lane.st();
                (
                    st.reference(&project.name()),
                    st.starting > 0 || st.any_running(),
                )
            })
            .collect();
        let mut hosts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for route in local {
            hosts
                .entry(route.lane.clone())
                .or_default()
                .insert(route.host.clone());
        }
        let lanes = hosts
            .into_iter()
            .map(|(lane, hosts)| LocalLane {
                running: running.get(&lane).copied().unwrap_or(false),
                lane,
                hosts,
            })
            .collect();
        let suffixes = mirror::route_suffixes(self);
        let boxes = self
            .remote
            .all()
            .into_iter()
            .filter(|conn| !conn.ended())
            .map(|conn| {
                let registry = conn.registry.current();
                let mut lanes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                let mut ports: BTreeMap<String, Vec<PortUse>> = BTreeMap::new();
                for unit in registry.units().filter(|unit| unit.mirrorable()) {
                    let lane = mirror::lane_ref(&unit.key.lane);
                    let routed: BTreeSet<u16> = unit
                        .hosts
                        .iter()
                        .filter(|host| mirror::accepted(&host.host, &suffixes).is_some())
                        .map(|host| host.port)
                        .collect();
                    lanes.entry(lane.clone()).or_default().extend(
                        unit.hosts
                            .iter()
                            .filter_map(|host| mirror::accepted(&host.host, &suffixes)),
                    );
                    let serving = devlanes_transport::serving(&unit.state);
                    let bound: BTreeSet<u16> = unit.ports.iter().map(|port| port.port).collect();
                    let uses = ports.entry(lane).or_default();
                    for port in bound.union(&routed) {
                        uses.push(PortUse {
                            port: *port,
                            unit: unit.key.name.clone(),
                            serving: serving && bound.contains(port),
                            routed: routed.contains(port),
                        });
                    }
                }
                BoxLanes {
                    host: conn.name.clone(),
                    serial: conn.serial,
                    lanes,
                    ports,
                }
            })
            .collect();
        (lanes, boxes)
    }

    /// Reports each choice a starting local lane cleared and deletes it from
    /// the store in the background, unless another choice replaced it.
    fn announce_cleared(&self, cleared: Vec<(String, String, String)>) {
        for (lane, host, local) in cleared {
            let message = if local == lane {
                format!("{lane} started on this machine and takes its hostnames back from {host}")
            } else {
                format!(
                    "{local} started on this machine and takes the hostnames of {host}'s {lane}"
                )
            };
            tracing::info!(%host, %lane, %local, "a starting local lane cleared a lane choice");
            self.bus.emit(
                wire::WARNING_EVENT,
                json!({
                    "host": host,
                    "code": "choice_cleared",
                    "lane": lane,
                    "local_lane": local,
                    "message": message,
                }),
            );
            // Called from synchronous route publishing, so the write cannot be
            // awaited here.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let store = self.store.clone();
                runtime.spawn(async move {
                    if let Err(error) = store.delete_remote_lane_choice(lane, Some(host)).await {
                        tracing::warn!(%error, "a restarted daemon may still use a cleared lane choice");
                    }
                });
            }
        }
    }

    /// `remote.use`: makes box `host`'s lane `lane` serve the hostnames it
    /// clashes on, or with no host this machine's lane, and replies once
    /// every mirror applied it.
    pub(crate) async fn use_method(self: &Arc<Self>, value: Value) -> Result<Value, RpcError> {
        let args: wire::UseParams = required(value)?;
        let lane = args.lane.trim().to_owned();
        if lane.is_empty() {
            return Err(RpcError::new(
                "bad_request",
                json!({"message": "name the lane, such as acme@0"}),
                2,
            ));
        }
        self.load_choices().await;
        let local = self.local_route_entries();
        let (_, before) = self.reassign_with(&local);
        let mut pending = false;
        let previous = match &args.host {
            Some(host) => {
                pending = !self.check_box_lane(host, &lane, &local).await?;
                let previous = if before.mirrors(host, &lane) {
                    Some(host.clone())
                } else if pending {
                    // Whichever box serves the lane until the chosen one connects.
                    before
                        .mirrored
                        .iter()
                        .find(|(_, mirrored)| *mirrored == lane)
                        .map(|(other, _)| other.clone())
                } else {
                    before
                        .skip(host, &lane)
                        .and_then(|skip| skip.winner.clone())
                };
                self.store
                    .set_remote_lane_choice(RemoteLaneChoice {
                        lane: lane.clone(),
                        host: host.clone(),
                        chosen_at: i64::try_from(now_ms()).unwrap_or(i64::MAX),
                    })
                    .await
                    .map_err(crate::registry::store_error)?;
                lock(&self.remote.choices).insert(lane.clone(), host.clone());
                previous
            }
            None => {
                let known = self
                    .all_lanes()
                    .iter()
                    .any(|(project, local)| local.st().reference(&project.name()) == lane);
                if !known {
                    return Err(use_error(
                        "local_lane_unknown",
                        json!({"lane": lane}),
                        format!(
                            "this machine has no lane {lane}. Run devlanes remote use <box> {lane} to choose a box"
                        ),
                        1,
                    ));
                }
                self.store
                    .delete_remote_lane_choice(lane.clone(), None)
                    .await
                    .map_err(crate::registry::store_error)?;
                lock(&self.remote.choices).remove(&lane);
                before.yielded.get(&lane).cloned()
            }
        };
        // The listeners and routes follow the new assignment by the time this
        // returns. The wait only lets the mirrors bring `host.list` up to date.
        let (version, after) = self.reassign();
        self.await_applied(version).await;
        let mirrored = match &args.host {
            Some(host) => after.mirrors(host, &lane),
            None => !after.yielded.contains_key(&lane),
        };
        if mirrored && previous != args.host {
            self.announce_switch(&lane, args.host.as_deref(), previous.as_deref());
        }
        Ok(json!({
            "lane": lane,
            "host": args.host,
            "previous": previous,
            "mirrored": mirrored,
            "pending": pending,
        }))
    }

    /// Says once that `lane` now comes from box `to`, or from this machine,
    /// instead of `from`: the one notice of a switch.
    fn announce_switch(&self, lane: &str, to: Option<&str>, from: Option<&str>) {
        let message = match to {
            Some(to) => format!("{lane} now comes from {to}"),
            None => format!("{lane} now comes from this machine"),
        };
        tracing::info!(%lane, ?to, ?from, "a lane switched");
        self.bus.emit(
            wire::WARNING_EVENT,
            json!({
                "host": to.or(from),
                "code": "lane_switched",
                "lane": lane,
                "to": to,
                "from": from,
                "message": message,
            }),
        );
    }

    /// Refuses a choice of box `host`'s lane `lane` when the box is unknown,
    /// has no such lane, or a running local lane would keep the hostnames
    /// anyway. Returns false for a known box that is not connected, whose
    /// choice applies once it connects, unchecked.
    async fn check_box_lane(
        &self,
        host: &str,
        lane: &str,
        local: &[RouteEntry],
    ) -> Result<bool, RpcError> {
        let conn = lock(&self.remote.hosts).get(host).cloned();
        let Some(conn) = conn.filter(|conn| !conn.ended()) else {
            let known = self
                .store
                .remote_hosts()
                .await
                .unwrap_or_default()
                .iter()
                .any(|stored| stored.name == host);
            if known {
                return Ok(false);
            }
            return Err(use_error(
                "box_unknown",
                json!({"host": host}),
                format!(
                    "this machine knows no box called {host}. Run devlanes remote ls to see the boxes"
                ),
                4,
            ));
        };
        let on_box = conn
            .registry
            .current()
            .lanes()
            .any(|known| mirror::lane_ref(&known.key) == lane);
        if !on_box {
            return Err(use_error(
                "lane_not_on_box",
                json!({"host": host, "lane": lane}),
                format!("box {host} has no lane {lane}. Run devlanes remote ls to see its lanes"),
                1,
            ));
        }
        // What the assignment would be with this choice.
        let (lanes, boxes) = self.assign_inputs(local);
        let mut choices = lock(&self.remote.choices).clone();
        choices.insert(lane.to_owned(), host.to_owned());
        let trial = assign(&lanes, &boxes, &choices);
        if let Some(skip) = trial
            .skip(host, lane)
            .filter(|skip| skip.cause == Cause::LocalRunning)
        {
            let running = &skip.winner_lane;
            return Err(use_error(
                "local_lane_running",
                json!({"host": host, "lane": lane, "local_lane": running}),
                format!(
                    "{running} runs on this machine. Stop it with devlanes stop --lane {running}, then run devlanes remote use {host} {lane}."
                ),
                1,
            ));
        }
        Ok(true)
    }

    /// Waits, at most [`APPLY_WAIT`], until every mirror whose link has not
    /// ended reconciled with assignment `version` or a later one.
    async fn await_applied(&self, version: u64) {
        let waits = self
            .remote
            .all()
            .into_iter()
            .filter(|conn| !conn.ended())
            .map(|conn| {
                let mut applied = conn.applied.subscribe();
                async move {
                    let _ = applied.wait_for(|seen| *seen >= version).await;
                }
            });
        let all = futures_util::future::join_all(waits);
        if tokio::time::timeout(APPLY_WAIT, all).await.is_err() {
            tracing::debug!(version, "a mirror did not apply a lane choice in time");
        }
    }
}
