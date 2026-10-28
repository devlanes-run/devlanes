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
    assign::{Assignment, BoxLanes, Cause, LocalLane, PortUse, assign, clashing_choices},
    mirror, now_ms,
};
use crate::{
    engine::{Inner, required},
    integrations::RouteEntry,
    model::{LaneSt, lock},
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

/// Whether a local lane holds its ports, so it wins over every box lane it
/// clashes with: from the start of a start that passed its checks, while a
/// unit or an orphan runs, and while its containers are up.
pub(crate) fn holds_ports(st: &LaneSt) -> bool {
    st.claiming > 0 || st.any_running() || !st.infra_containers.is_empty()
}

/// `local_lane_running`: this machine's `running` keeps box `host`'s `lane`
/// from serving.
fn local_running(host: &str, lane: &str, running: &str) -> RpcError {
    use_error(
        "local_lane_running",
        json!({"host": host, "lane": lane, "local_lane": running}),
        format!(
            "{running} runs on this machine. Stop it with devlanes stop --lane {running}, then run devlanes remote use {host} {lane}."
        ),
        1,
    )
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

    /// Deletes every choice, after `reset-state` emptied the store.
    pub(crate) fn forget_all_choices(&self) {
        lock(&self.remote.choices).clear();
    }

    /// Reads the choices from the store again after retention deleted the
    /// rows of forgotten boxes, and applies them.
    pub(crate) async fn reload_choices(&self) {
        {
            // A `remote.use` writes the store and memory under this lock, so
            // the rows read here are not older than what memory holds.
            let _choosing = self.remote.connecting.lock().await;
            match self.store.remote_lane_choices().await {
                Ok(rows) => {
                    *lock(&self.remote.choices) =
                        rows.into_iter().map(|row| (row.lane, row.host)).collect();
                }
                Err(error) => tracing::warn!(%error, "cannot read the lane choices"),
            }
        }
        self.reassign();
    }

    /// [`Inner::reassign_with`] this machine's current routes.
    pub(crate) fn reassign(&self) -> (u64, Arc<Assignment>) {
        let computing = lock(&self.remote.assigning);
        // Read under the lock, so routes read before a newer assignment
        // never replace it.
        let local = self.local_route_entries();
        self.reassign_locked(computing, &local)
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
        self.reassign_locked(computing, local)
    }

    /// [`Inner::reassign_with`]'s work, with the assignment lock held as
    /// `computing`.
    fn reassign_locked(
        &self,
        computing: std::sync::MutexGuard<'_, ()>,
        local: &[RouteEntry],
    ) -> (u64, Arc<Assignment>) {
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

    /// The assignment's inputs: this machine's lanes from `local`, counted
    /// as running by [`holds_ports`], and every box whose link has not ended,
    /// with the hostnames this machine would route and the ports of each of
    /// its lanes. A box lane counts only once the box's mirror built its
    /// routes, so a box that connects never wins a lane it has no routes for
    /// yet.
    fn assign_inputs(&self, local: &[RouteEntry]) -> (Vec<LocalLane>, Vec<BoxLanes>) {
        let running: BTreeMap<String, bool> = self
            .all_lanes()
            .iter()
            .map(|(project, lane)| {
                let st = lane.st();
                (st.reference(&project.name()), holds_ports(&st))
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
                let built = lock(&conn.view).built.clone();
                let mut lanes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                let mut ports: BTreeMap<String, Vec<PortUse>> = BTreeMap::new();
                for unit in registry.units().filter(|unit| unit.mirrorable()) {
                    let lane = mirror::lane_ref(&unit.key.lane);
                    if !built.contains(&lane) {
                        continue;
                    }
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
            let message = format!(
                "{local} started on this machine, so {host}'s {lane} is no longer mirrored or chosen."
            );
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
                let choosing = self.remote.connecting.clone();
                let choices = self.remote.choices.clone();
                runtime.spawn(async move {
                    // `remote.use` writes under this lock, so a choice of the
                    // same box made since is in memory by now and kept.
                    let _choosing = choosing.lock().await;
                    if lock(&choices).get(&lane) == Some(&host) {
                        return;
                    }
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
        // One at a time, and against `remote disconnect`: the check, the
        // store and memory see the same choices.
        let choosing = self.remote.connecting.lock().await;
        let local = self.local_route_entries();
        let (_, before) = self.reassign_with(&local);
        let (_, boxes) = self.assign_inputs(&local);
        let mut pending = false;
        let (previous, replaced) = match &args.host {
            Some(host) => {
                pending = !self.check_box_lane(host, &lane, &local).await?;
                let hosts = boxes
                    .iter()
                    .find(|other| other.host == *host)
                    .and_then(|other| other.lanes.get(&lane))
                    .cloned()
                    .unwrap_or_default();
                let choices = lock(&self.remote.choices).clone();
                let replaced = clashing_choices(&lane, &hosts, Some(host), &boxes, &choices);
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
                (previous, replaced)
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
                // Whatever took the lane's hostnames, also a choice of another
                // project's lane.
                let hosts: BTreeSet<String> = local
                    .iter()
                    .filter(|route| route.lane == lane)
                    .map(|route| route.host.clone())
                    .collect();
                let choices = lock(&self.remote.choices).clone();
                let replaced = clashing_choices(&lane, &hosts, None, &boxes, &choices);
                (before.yielded.get(&lane).cloned(), replaced)
            }
        };
        for (other, host) in &replaced {
            self.store
                .delete_remote_lane_choice(other.clone(), Some(host.clone()))
                .await
                .map_err(crate::registry::store_error)?;
            let mut choices = lock(&self.remote.choices);
            if choices.get(other) == Some(host) {
                choices.remove(other);
            }
        }
        drop(choosing);
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
            // The lane's own choice, which `--local` deletes, is `previous`.
            "replaced": replaced
                .iter()
                .filter(|(other, _)| *other != lane)
                .map(|(other, host)| json!({"lane": other, "host": host}))
                .collect::<Vec<_>>(),
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
    /// choice applies once it connects. Its lanes are unknown until then, so
    /// only a local lane of the same reference refuses it.
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
                .map_err(crate::registry::store_error)?
                .iter()
                .any(|stored| stored.name == host);
            if known {
                return match self.running_here(lane) {
                    true => Err(local_running(host, lane, lane)),
                    false => Ok(false),
                };
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
        // A running local lane of the same reference holds its ports even
        // when no hostname clashes, and the start that made it run cleared
        // any choice for it.
        if self.running_here(lane) {
            return Err(local_running(host, lane, lane));
        }
        // A running local lane of another reference only wins where its
        // hostnames clash, which only the assignment with this choice tells.
        let (lanes, boxes) = self.assign_inputs(local);
        let mut choices = lock(&self.remote.choices).clone();
        choices.insert(lane.to_owned(), host.to_owned());
        let trial = assign(&lanes, &boxes, &choices);
        if let Some(skip) = trial
            .skip(host, lane)
            .filter(|skip| skip.cause == Cause::LocalRunning)
        {
            return Err(local_running(host, lane, &skip.winner_lane));
        }
        Ok(true)
    }

    /// Whether this machine's lane `lane` holds its ports ([`holds_ports`]).
    fn running_here(&self, lane: &str) -> bool {
        self.all_lanes().iter().any(|(project, local)| {
            let st = local.st();
            st.reference(&project.name()) == lane && holds_ports(&st)
        })
    }

    /// Waits, at most [`APPLY_WAIT`], until every mirror whose link has not
    /// ended reconciled with assignment `version` or a later one.
    pub(crate) async fn await_applied(&self, version: u64) {
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
