//! Makes a box's non-hidden endpoints local (plan 14.4): every allocated port of
//! its serving services and containers is bound on this machine's loopback
//! addresses under the same number, and its hostnames join the local proxy's
//! routing table. Each accepted connection becomes one mux data stream to the box.
//!
//! Local lanes win: a box port inside one of this machine's port blocks is never
//! bound (its hostnames still route through a relay), and local allocations keep
//! clear of the box's ports (`Remotes::advertised_ports`). A port something here
//! holds on any loopback address, or on a wildcard address, is bound on none.
//! Which box lane serves hostnames that several lanes claim is the
//! [`assign`](super::assign) decision; a box lane it skips is skipped whole, and
//! a port another box's lane owns is reported with that box. A stopped local
//! lane a chosen box lane is in use instead of keeps no port either.
//!
//! The listeners are the daemon's, not this mirror's
//! ([`Listeners`](super::listeners::Listeners)). When the decision moves a
//! lane to another box, its listeners and relays lead to the new box from
//! that moment, and the routes this mirror kept ready for its skipped lanes
//! go live in the same publish. The old box's connections close once their
//! answers are through.
//!
//! Listeners stay bound while the link reconnects; connections accepted meanwhile
//! wait briefly for the new session and the proxy shows a reconnecting page.
//! They also stay while the link waits for a sign-in; connections then fail at
//! once and the proxy shows the sign-in page.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use devlanes_core::ports::PortBlock;
use devlanes_proto::{events, remote as wire};
use devlanes_state::BlockOwner;
use devlanes_transport::{
    DialTarget, HostHeader, LaneKey, LinkState, OpenError, Registry, RejectReason, UnitInfo,
    UnitKind, serving,
};
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpSocket, TcpStream},
    sync::broadcast,
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use super::{
    HostConn,
    assign::{Assignment, Cause, PortOwner, Skip},
    listeners::Target,
};
use crate::{
    engine::Inner,
    integrations::{RouteEntry, RouteStatus},
    model::lock,
};

/// How long a connection accepted during a reconnect waits for the new session.
const HOLD: Duration = Duration::from_secs(10);

/// Shortest time between two `open_failed` warnings for the same port and reason.
const OPEN_WARNING_EVERY: Duration = Duration::from_secs(60);

/// How long a connection to a box that lost its lane stays open after the
/// box last spoke, so the end of an answer gets through. Streamed SSR pages
/// and slow chunked answers pause between chunks, often for a few tens of
/// milliseconds and sometimes longer, and closing in such a pause would cut
/// the answer. An idle connection still closes at once.
const SWITCH_QUIET: Duration = Duration::from_millis(300);

/// Longest a connection to a box that lost its lane stays open, answer under
/// way or not.
const SWITCH_LIMIT: Duration = Duration::from_secs(5);

/// What a host mirrors right now, for routes and status output.
#[derive(Debug, Clone, Default)]
pub(super) struct View {
    /// Routes of the lanes this machine mirrors from the box.
    pub routes: Vec<RouteEntry>,
    /// Routes of the box's skipped lanes, ready for the moment a switch
    /// gives one of them to this box. Never published while skipped.
    pub standby: Vec<RouteEntry>,
    pub ports: Vec<Value>,
    /// Ports with at least one listener, and the box unit behind each
    /// (`project@lane:unit`), so a local spawn can tell whose port it hit.
    pub bound: BTreeMap<u16, String>,
    pub skipped: Vec<Value>,
    /// Box hostnames outside the names this machine routes (`.localhost` or the
    /// configured TLD).
    pub refused_hosts: Vec<Value>,
    /// The last failed open per port listed in `ports`, until the next
    /// successful one, shown there as `last_error`. A failed open through a
    /// relay of a port that is not listed (a routed unit whose port is not
    /// bound here) is reported only by its `open_failed` warning.
    pub open_errors: BTreeMap<u16, OpenFailure>,
    /// When the last `open_failed` warning per port and reason went out.
    pub warned: BTreeMap<(u16, &'static str), Instant>,
    /// The lanes this mirror built routes for, live or kept aside. Only
    /// they count in the assignment, so the box never wins a lane whose
    /// hostnames would then lead nowhere.
    pub built: BTreeSet<String>,
}

/// A connection to a mirrored port that did not reach the box's service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenFailure {
    /// `not_registered`, `hidden`, `not_running` or `dial_failed` when the box
    /// refused; `timeout`, `interrupted`, `session_lost`, `not_connected`,
    /// `needs_auth` or `needs_pairing` otherwise.
    pub reason: &'static str,
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
}

impl View {
    /// Messages for ports that could not be mirrored.
    pub(super) fn warnings(&self) -> Vec<String> {
        self.ports
            .iter()
            .filter_map(|port| port.get("conflicts").and_then(Value::as_array))
            .flatten()
            .filter_map(|conflict| conflict.get("message").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    /// The mirrored ports for status output, each with its `last_error`
    /// (`{reason, message, at_ms}`, or null).
    pub(super) fn ports_json(&self) -> Vec<Value> {
        self.ports
            .iter()
            .map(|port| {
                let error = port
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|number| u16::try_from(number).ok())
                    .and_then(|number| self.open_errors.get(&number))
                    .map_or(Value::Null, |failure| {
                        json!({
                            "reason": failure.reason,
                            "message": failure.message,
                            "at_ms": failure.at_ms,
                        })
                    });
                let mut port = port.clone();
                if let Some(fields) = port.as_object_mut() {
                    fields.insert("last_error".into(), error);
                }
                port
            })
            .collect()
    }

    /// Whether `port` is one of the listed mirrored ports.
    fn lists(&self, port: u16) -> bool {
        self.ports
            .iter()
            .any(|entry| entry.get("port").and_then(Value::as_u64) == Some(u64::from(port)))
    }

    /// Drops failures of ports no longer listed, and warning times of ports
    /// neither listed nor `relayed` or older than [`OPEN_WARNING_EVERY`], which
    /// no longer hold a warning back.
    fn prune(&mut self, now: Instant, relayed: impl Fn(u16) -> bool) {
        let listed: BTreeSet<u16> = self
            .ports
            .iter()
            .filter_map(|entry| entry.get("port").and_then(Value::as_u64))
            .filter_map(|port| u16::try_from(port).ok())
            .collect();
        self.open_errors.retain(|port, _| listed.contains(port));
        self.warned.retain(|(port, _), last| {
            (listed.contains(port) || relayed(*port))
                && now.duration_since(*last) < OPEN_WARNING_EVERY
        });
    }

    /// Records a failed open of `port` (as `last_error` when the port is
    /// listed) and reports whether its warning is due: the first for this port
    /// and reason, or the first after [`OPEN_WARNING_EVERY`].
    fn record_failure(&mut self, port: u16, failure: OpenFailure, now: Instant) -> bool {
        let key = (port, failure.reason);
        if self.lists(port) {
            self.open_errors.insert(port, failure);
        }
        let due = self
            .warned
            .get(&key)
            .is_none_or(|last| now.duration_since(*last) >= OPEN_WARNING_EVERY);
        if due {
            self.warned.insert(key, now);
        }
        due
    }
}

/// The reason `remote ls` shows next to a skipped lane: the lane in use.
pub(super) fn skip_reason(skip: &Skip) -> String {
    let lane = &skip.winner_lane;
    match (skip.cause, &skip.winner) {
        (Cause::LocalRunning, _) => skip.hold.as_ref().map_or_else(
            || format!("{lane} runs on this machine"),
            |hold| hold.runs(lane),
        ),
        (_, Some(winner)) => format!("{winner}'s {lane} is in use"),
        (_, None) => format!("this machine's {lane} is in use"),
    }
}

/// Adds what holds the running local lane's ports to a skipped lane's
/// `fields`, when one is the cause: `holds`, and for containers
/// `compose_project` and `containers`.
fn add_hold(fields: &mut Value, skip: &Skip) {
    if let (Some(hold), Some(fields)) = (&skip.hold, fields.as_object_mut()) {
        fields.extend(hold.fields());
    }
}

/// A skipped lane as `host.list` lists it.
fn skip_json(lane: &LaneKey, skip: &Skip) -> Value {
    let mut entry = json!({
        "lane": lane_ref(lane),
        "hosts": skip.hosts,
        "box": skip.winner,
        "reason": skip_reason(skip),
        "cause": skip.cause.as_str(),
        "winner": skip.winner,
        "winner_lane": skip.winner_lane,
        "can_use": skip.cause != Cause::LocalRunning,
    });
    add_hold(&mut entry, skip);
    entry
}

/// The link's state as the mirror sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connected,
    /// Down but retrying: listeners stay, routes show "reconnecting".
    Down,
    /// Down until a person signs in again: listeners stay, routes show the
    /// sign-in page.
    SignIn,
    /// Down until a person pairs this machine with the box again: listeners
    /// stay, routes show the pairing page.
    Pairing,
    /// Given up: nothing stays mirrored.
    Over,
}

impl Phase {
    /// Classifies a link state.
    fn of(state: &LinkState) -> Self {
        match state {
            LinkState::Connected(_) => Self::Connected,
            LinkState::NeedsAuth { .. } => Self::SignIn,
            LinkState::NeedsPairing { .. } => Self::Pairing,
            LinkState::Failed { .. } | LinkState::Stopped => Self::Over,
            LinkState::Connecting { .. } | LinkState::Retrying { .. } => Self::Down,
        }
    }
}

/// One accept loop; aborted when the listener is no longer wanted.
pub(super) struct Bound {
    pub address: IpAddr,
    pub task: JoinHandle<()>,
}

impl Drop for Bound {
    /// Stops accepting; the listener closes with the task.
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Stops the accept loops of `bounds` and waits until their listeners are closed.
/// Dropping a [`Bound`] only schedules that, and a local service started right
/// after would then find the port still taken by this daemon.
pub(super) async fn unbind(bounds: Vec<Bound>) {
    for bound in &bounds {
        bound.task.abort();
    }
    for mut bound in bounds {
        let _ = (&mut bound.task).await;
    }
}

/// A box port this box wants mirrored and what keeps it from being bound
/// here. The listeners themselves are the daemon's ([`Listeners`]).
///
/// [`Listeners`]: super::listeners::Listeners
struct PortMirror {
    unit: String,
    lane: String,
    kind: UnitKind,
    /// Addresses whose bind failed, with what the port inspector found, or
    /// every address while another box's lane owns the number.
    conflicts: BTreeMap<IpAddr, Value>,
    /// Set while the port lies in a local lane's block: the message and the owner.
    local: Option<(String, String)>,
}

impl PortMirror {
    /// A port of `unit` without listeners yet.
    fn new(unit: &UnitInfo) -> Self {
        Self {
            unit: unit.key.name.clone(),
            lane: lane_ref(&unit.key.lane),
            kind: unit.key.kind,
            conflicts: BTreeMap::new(),
            local: None,
        }
    }
}

/// Mirror state owned by one host's mirror task.
struct Mirror {
    /// Handed to accept loops, which report failed opens on the event bus.
    engine: Weak<Inner>,
    conn: Arc<HostConn>,
    addresses: Vec<IpAddr>,
    ports: BTreeMap<u16, PortMirror>,
    /// The relays this box's routes use, by lane and box port.
    relays: BTreeSet<(String, u16)>,
    /// Lanes the assignment skips, as last reported.
    skipped: BTreeMap<LaneKey, Skip>,
    /// Lane references mirrored because a choice names this box, as of the
    /// last reconcile.
    chosen: BTreeSet<String>,
}

/// Keeps the host's listeners and routes in step with its registry, the link
/// state and local lanes until the host is disconnected.
pub(super) async fn run(engine: Weak<Inner>, conn: Arc<HostConn>) {
    let Some(inner) = engine.upgrade() else {
        return;
    };
    let mut mirror = Mirror {
        engine: engine.clone(),
        conn: conn.clone(),
        addresses: inner.integ.remote.mirror_addresses(),
        ports: BTreeMap::new(),
        relays: BTreeSet::new(),
        skipped: BTreeMap::new(),
        chosen: BTreeSet::new(),
    };
    let mut bus = inner.bus.subscribe();
    drop(inner);
    let mut registry = conn.registry.subscribe();
    let mut state = conn.state.clone();
    let mut announced = Registry::default();
    loop {
        let Some(inner) = engine.upgrade() else { break };
        let current = registry.borrow_and_update().clone();
        // `host.list` carries the registry, so a client showing it hears of
        // any change, also one that binds or routes nothing.
        if current != announced {
            conn.mark_changed();
            announced = current.clone();
        }
        let phase = Phase::of(&state.borrow_and_update());
        let version = if phase == Phase::Over {
            mirror.clear(&inner).await
        } else {
            mirror.reconcile(&inner, &current, phase).await
        };
        // `remote.use` replies once every mirror got this far.
        conn.applied.send_replace(version);
        drop(inner);
        // Local lanes matter because their hostnames win over the box's.
        let wake = async {
            loop {
                match bus.recv().await {
                    Ok(event)
                        if event.event == events::LANE_CHANGED
                            || event.event == events::LANE_REMOVED =>
                    {
                        return true;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return true,
                    Err(broadcast::error::RecvError::Closed) => return false,
                }
            }
        };
        let go_on = tokio::select! {
            () = conn.cancel.cancelled() => false,
            changed = registry.changed() => changed.is_ok(),
            changed = state.changed() => changed.is_ok(),
            more = wake => more,
            // A local spawn found one of its ports mirrored (`release_for_local`).
            () = conn.wake.notified() => true,
        };
        if !go_on {
            break;
        }
    }
    // Before `HostConn::close` returns: the disconnect removed this box, so
    // the reassignment hands its lanes' listeners to other boxes, and what
    // nobody takes is closed.
    if let Some(inner) = engine.upgrade() {
        inner.reassign();
        mirror.release_all(&inner).await;
    }
}

impl Mirror {
    /// Gives up every port and relay of this box. Listeners another box's
    /// lane owns now stay bound for it.
    async fn release_all(&mut self, engine: &Arc<Inner>) {
        let listeners = &engine.remote.listeners;
        for port in std::mem::take(&mut self.ports).into_keys() {
            listeners.release(port, self.conn.serial).await;
        }
        listeners.sweep().await;
        self.relays.clear();
        listeners.keep_relays(self.conn.serial, &self.relays);
    }

    /// Unbinds everything and drops the host's routes, once the link gave up.
    /// Returns the assignment version it saw, which no longer has this box.
    async fn clear(&mut self, engine: &Arc<Inner>) -> u64 {
        let (version, _) = engine.reassign();
        self.release_all(engine).await;
        self.skipped.clear();
        self.chosen.clear();
        let (had_routes, had_ports) = {
            let mut view = lock(&self.conn.view);
            let had = (
                !view.routes.is_empty() || !view.standby.is_empty(),
                !view.bound.is_empty(),
            );
            if had.0
                || !view.ports.is_empty()
                || !view.skipped.is_empty()
                || !view.refused_hosts.is_empty()
                || !view.open_errors.is_empty()
            {
                self.conn.mark_changed();
            }
            *view = View::default();
            had
        };
        if had_routes {
            engine.publish_routes();
        }
        if had_routes || had_ports {
            engine.remote.wake_others(self.conn.serial);
        }
        version
    }

    /// Binds new ports, drops stale ones, retries conflicts and republishes
    /// routes. Returns the assignment version it followed.
    async fn reconcile(&mut self, engine: &Arc<Inner>, registry: &Registry, phase: Phase) -> u64 {
        // Read after the registry, so it has every lane `registry` lists.
        let (version, assignment) = engine.reassign();
        let suffixes = route_suffixes(engine);
        // Tasks are listed by the box but never mirrored. A box sends no hidden
        // units; the filter keeps any it would send off this machine all the same.
        let visible: Vec<&UnitInfo> = registry.units().filter(|unit| unit.mirrorable()).collect();

        // A box lane the assignment skips is skipped whole: renaming it would
        // break the CORS origins and cookies the box chose.
        let mut skipped: BTreeMap<LaneKey, Skip> = BTreeMap::new();
        for unit in &visible {
            if let Some(skip) = assignment.skip(&self.conn.name, &lane_ref(&unit.key.lane)) {
                skipped
                    .entry(unit.key.lane.clone())
                    .or_insert_with(|| skip.clone());
            }
        }
        for (lane, skip) in &skipped {
            // Said once, when the lane stops being mirrored. A lane that lost
            // to a choice, or that a choice no longer holds, follows a change
            // `lane_switched` or `choice_cleared` already announced.
            let switched = skip.cause == Cause::BoxChosen || self.chosen.contains(&lane_ref(lane));
            if !self.skipped.contains_key(lane) && !switched {
                tracing::warn!(host = %self.conn.name, lane = %lane_ref(lane), hosts = ?skip.hosts, winner = ?skip.winner, cause = skip.cause.as_str(), "lane skipped: another lane serves its hostnames");
                let mut warning = json!({
                    "host": self.conn.name,
                    "code": "lane_skipped",
                    "lane": lane_ref(lane),
                    "hosts": skip.hosts,
                    "box": skip.winner,
                    "local_lanes": skip.local_lanes,
                    "cause": skip.cause.as_str(),
                    "winner_lane": skip.winner_lane,
                });
                add_hold(&mut warning, skip);
                engine.bus.emit(wire::WARNING_EVENT, warning);
            }
        }
        self.skipped = skipped;
        self.chosen = assignment
            .chosen
            .iter()
            .filter(|(host, _)| *host == self.conn.name)
            .map(|(_, lane)| lane.clone())
            .collect();
        // A lane the assignment has not seen yet is neither: it binds nothing
        // until it counts.
        let mirrored: Vec<&UnitInfo> = visible
            .iter()
            .copied()
            .filter(|unit| assignment.mirrors(&self.conn.name, &lane_ref(&unit.key.lane)))
            .collect();

        // Idle units are not bound, so their numbers stay free until they start.
        let mut wanted: BTreeMap<u16, &UnitInfo> = BTreeMap::new();
        for unit in mirrored.iter().filter(|unit| serving(&unit.state)) {
            for port in &unit.ports {
                wanted.entry(port.port).or_insert(unit);
            }
        }
        let local = local_blocks(engine, &assignment).await;
        let listeners = &engine.remote.listeners;
        let stale: Vec<u16> = self
            .ports
            .keys()
            .copied()
            .filter(|port| !wanted.contains_key(port))
            .collect();
        // A port this box's lane lost to another box's stays bound for that
        // box; `release` only closes what nobody else owns.
        for port in &stale {
            self.ports.remove(port);
            listeners.release(*port, self.conn.serial).await;
        }
        // Every wanted port, bound or not: a program that started listening
        // on a wildcard address since the last reconcile takes its port back.
        let shadowing = shadowing(wanted.keys().copied().collect()).await;
        for (port, unit) in &wanted {
            let mirror = self
                .ports
                .entry(*port)
                .or_insert_with(|| PortMirror::new(unit));
            let single = PortBlock {
                start: *port,
                size: 1,
            };
            let owner = local
                .iter()
                .find(|(block, _)| block.overlaps(single))
                .map(|(_, owner)| owner);
            if let Some(owner) = owner {
                // Local lanes win: dropping the listeners frees the port for them.
                listeners.release(*port, self.conn.serial).await;
                mirror.conflicts.clear();
                if mirror
                    .local
                    .as_ref()
                    .is_none_or(|(_, known)| known != owner)
                {
                    let message =
                        format!("port {port} is not mirrored because it belongs to {owner} here");
                    tracing::warn!(host = %self.conn.name, %message, "cannot mirror port");
                    engine.bus.emit(
                        wire::WARNING_EVENT,
                        json!({
                            "host": self.conn.name,
                            "code": "port_in_use",
                            "message": message,
                            "port": port,
                            "local": owner,
                            "unit": mirror.unit,
                            "lane": mirror.lane,
                        }),
                    );
                    mirror.local = Some((message, owner.clone()));
                }
                continue;
            }
            mirror.local = None;
            let owner = assignment.listeners.get(port);
            if let Some(other) = owner.filter(|owner| owner.host != self.conn.name) {
                // A lane of another box wants the same number and ranks
                // first. A switch never gets here: the lane's ports move with
                // it, so only two different lanes compete for a number.
                box_conflict(engine, &self.conn, &self.addresses, mirror, *port, other);
                continue;
            }
            let shadowed = shadowing.get(port).copied();
            if shadowed.is_some() {
                // Every address is tried again below and fails as shadowed.
                listeners.remove(*port).await;
            }
            let bound = listeners.addresses(*port);
            for address in &bound {
                mirror.conflicts.remove(address);
            }
            let missing: Vec<IpAddr> = self
                .addresses
                .iter()
                .copied()
                .filter(|address| !bound.contains(address))
                .collect();
            let attempt = bind_port(&missing, *port, shadowed);
            if attempt.held() {
                // Held on one loopback address, the port stays off the others
                // too. Closed now, so the holder lookup below cannot find this
                // daemon's own listener.
                listeners.remove(*port).await;
            }
            for address in &attempt.closed {
                mirror.conflicts.remove(address);
            }
            for (address, listener) in attempt.listeners {
                mirror.conflicts.remove(&address);
                engine
                    .remote
                    .listen(*port, address, listener, self.engine.clone());
            }
            // Looked up once per port, whichever addresses failed: a process
            // scan each.
            let mut pid_holder: Option<Option<u32>> = None;
            let mut names: BTreeMap<u32, String> = BTreeMap::new();
            for (address, failure) in attempt.failures {
                let in_use = failure.in_use();
                let pid = match &failure {
                    BindFailure::Shadowed(pid) => Some(*pid),
                    BindFailure::Bind(_) if in_use => match pid_holder {
                        Some(pid) => pid,
                        None => *pid_holder.insert(engine.integ.ports.holder(*port).await),
                    },
                    BindFailure::Bind(_) => None,
                };
                // This daemon itself: usually a listener that is still
                // closing. The next reconcile tries again.
                let own = pid == Some(std::process::id());
                // What keeps the port when something does: the
                // message's subject. A bind that failed for another
                // reason has none.
                let holder = if own {
                    Some("this devlanes daemon".to_owned())
                } else if in_use {
                    Some(match pid {
                        Some(pid) => {
                            let name = match names.entry(pid) {
                                std::collections::btree_map::Entry::Occupied(known) => {
                                    known.get().clone()
                                }
                                std::collections::btree_map::Entry::Vacant(slot) => {
                                    slot.insert(process_name(pid).await).clone()
                                }
                            };
                            format!("{name} (pid {pid})")
                        }
                        None => "another process".to_owned(),
                    })
                } else {
                    None
                };
                let message = match &holder {
                    Some(holder) => {
                        format!("port {port} is not mirrored because {holder} uses it here")
                    }
                    None => format!(
                        "port {port} is not mirrored because binding it on {address} failed: {}",
                        failure.describe()
                    ),
                };
                // Reported once per holder: again when something else holds
                // it now. A port this daemon holds is only listed until a
                // wake tells why.
                let known = mirror.conflicts.get(&address);
                let report = !own
                    && known.is_none_or(|known| {
                        !known["box"].is_null()
                            || known["pid"].as_u64() == Some(u64::from(std::process::id()))
                    });
                if report {
                    tracing::warn!(host = %self.conn.name, %message, "cannot mirror port");
                    engine.bus.emit(
                        wire::WARNING_EVENT,
                        json!({
                            "host": self.conn.name,
                            "code": "port_in_use",
                            "message": message,
                            "port": port,
                            "address": address,
                            "pid": pid,
                            "box": null,
                            "held_by": null,
                            "unit": mirror.unit,
                            "lane": mirror.lane,
                        }),
                    );
                }
                mirror.conflicts.insert(
                    address,
                    json!({
                        "address": address,
                        "pid": pid,
                        "box": null,
                        "holder": holder,
                        "message": message,
                    }),
                );
            }
        }
        listeners.sweep().await;

        // Hostnames: the proxy dials 127.0.0.1 first, so a routed port this
        // machine does not hold there for the lane gets a relay on an
        // ephemeral port instead. Skipped lanes and lanes the assignment has
        // not seen yet get their routes too, kept aside: when the assignment
        // gives this box the lane, the route publisher takes them at once,
        // before this mirror reconciles again.
        let mut routes = Vec::new();
        let mut standby = Vec::new();
        let mut relays = BTreeSet::new();
        let mut refused_hosts = Vec::new();
        let box_https_port = lock(&self.conn.info).https_port;
        for unit in &visible {
            let reference = lane_ref(&unit.key.lane);
            let is_mirrored = assignment.mirrors(&self.conn.name, &reference);
            let lane = registry
                .lanes()
                .find(|lane| lane.key == unit.key.lane)
                .cloned();
            for host in &unit.hosts {
                // The local CA signs whatever the routes name, so a box may only
                // claim valid names this machine already routes to itself.
                let Some(name) = accepted(&host.host, &suffixes) else {
                    if is_mirrored {
                        refused_hosts.push(json!({
                            "host": host.host,
                            "lane": reference,
                            "unit": unit.key.name,
                        }));
                    }
                    continue;
                };
                let local_port = if listeners.serves_v4(host.port, &reference) {
                    host.port
                } else {
                    let relay = engine.remote.relay(
                        &reference,
                        host.port,
                        self.conn.serial,
                        self.engine.clone(),
                    );
                    match relay {
                        Ok(port) => {
                            relays.insert((reference.clone(), host.port));
                            port
                        }
                        Err(error) => {
                            tracing::warn!(host = %self.conn.name, port = host.port, %error, "cannot bind a route relay");
                            continue;
                        }
                    }
                };
                let box_name = &self.conn.name;
                // The proxy shows the hint on the page of the route's state.
                let start_hint = match phase {
                    Phase::SignIn => {
                        format!("Sign in to {box_name} again: devlanes remote connect {box_name}")
                    }
                    Phase::Pairing => format!(
                        "Pair with {box_name} again: devlanes daemon pair on {box_name}, then devlanes remote connect {box_name} --code <code>"
                    ),
                    _ => format!("Start it on {box_name}: devlanes start --lane {reference}"),
                };
                let route = RouteEntry {
                    host: name,
                    port: local_port,
                    fallback: host.fallback,
                    host_header_upstream: host.host_header == HostHeader::Upstream,
                    status: route_status(&unit.state, phase),
                    start_hint,
                    project: unit.key.lane.project.clone(),
                    lane: reference.clone(),
                    unit: unit.key.name.clone(),
                    certificate_names: lane
                        .as_ref()
                        .and_then(|lane| lane.domain.as_deref())
                        .and_then(|domain| accepted(domain, &suffixes))
                        .into_iter()
                        .collect(),
                    remote: true,
                    box_https_port,
                    // The local end is the mirror's listener, on either loopback.
                    address: None,
                    upstream_box: Some(self.conn.name.clone()),
                };
                if is_mirrored {
                    routes.push(route);
                } else {
                    standby.push(route);
                }
            }
        }
        if !refused_hosts.is_empty() && lock(&self.conn.view).refused_hosts != refused_hosts {
            tracing::warn!(host = %self.conn.name, ?refused_hosts, "box hostnames that are invalid or outside the local suffixes skipped");
            engine.bus.emit(
                wire::WARNING_EVENT,
                json!({"host": self.conn.name, "code": "hosts_skipped", "hosts": refused_hosts}),
            );
        }
        listeners.keep_relays(self.conn.serial, &relays);
        self.relays = relays;

        let bound = listeners.bound_by(self.conn.serial);
        let mut view = View {
            routes,
            standby,
            ports: self
                .ports
                .iter()
                .map(|(port, mirror)| {
                    let mut conflicts: Vec<Value> = mirror.conflicts.values().cloned().collect();
                    if let Some((message, owner)) = &mirror.local {
                        conflicts.push(json!({"message": message, "local": owner}));
                    }
                    let addresses = if bound.contains_key(port) {
                        listeners.addresses(*port)
                    } else {
                        Vec::new()
                    };
                    let state = port_state(
                        mirror.local.is_some(),
                        !addresses.is_empty(),
                        &mirror.conflicts,
                    );
                    json!({
                        "port": port,
                        "lane": mirror.lane,
                        "unit": mirror.unit,
                        "kind": mirror.kind,
                        "state": state,
                        "addresses": addresses,
                        "conflicts": conflicts,
                        "relay": listeners.relay_port(&mirror.lane, *port),
                    })
                })
                .collect(),
            bound,
            skipped: self
                .skipped
                .iter()
                .map(|(lane, skip)| skip_json(lane, skip))
                .collect(),
            refused_hosts,
            open_errors: BTreeMap::new(),
            warned: BTreeMap::new(),
            built: visible
                .iter()
                .map(|unit| lane_ref(&unit.key.lane))
                .collect(),
        };
        let routes_changed = {
            let mut current = lock(&self.conn.view);
            // New lanes count in the assignment from this publish on, which
            // reassigns with them.
            let changed = current.routes != view.routes
                || current.standby != view.standby
                || current.built != view.built;
            if changed
                || current.ports != view.ports
                || current.skipped != view.skipped
                || current.refused_hosts != view.refused_hosts
            {
                self.conn.mark_changed();
            }
            // Failed opens outlive a reconcile, but not the port they are about.
            view.open_errors = std::mem::take(&mut current.open_errors);
            view.warned = std::mem::take(&mut current.warned);
            let relayed: BTreeSet<u16> = self.relays.iter().map(|(_, port)| *port).collect();
            view.prune(Instant::now(), |port| relayed.contains(&port));
            *current = view;
            changed
        };
        if routes_changed {
            engine.publish_routes();
        }
        version
    }
}

/// Records that another box's lane `other` owns box port `port`, which this
/// box also wants for another lane, on every mirror address, and says so once
/// per owner.
fn box_conflict(
    engine: &Inner,
    conn: &HostConn,
    addresses: &[IpAddr],
    mirror: &mut PortMirror,
    port: u16,
    other: &PortOwner,
) {
    let held = format!("{}:{}", other.lane, other.unit);
    let holder = format!("box {} ({held})", other.host);
    let message = format!("port {port} is not mirrored because {holder} uses it here");
    let known = addresses.iter().all(|address| {
        mirror
            .conflicts
            .get(address)
            .is_some_and(|c| c["box"] == other.host.as_str())
    });
    if !known {
        tracing::warn!(host = %conn.name, %message, "cannot mirror port");
        engine.bus.emit(
            wire::WARNING_EVENT,
            json!({
                "host": conn.name,
                "code": "port_in_use",
                "message": message,
                "port": port,
                "pid": null,
                "box": other.host,
                "held_by": held,
                "unit": mirror.unit,
                "lane": mirror.lane,
            }),
        );
    }
    for address in addresses {
        mirror.conflicts.insert(
            *address,
            json!({
                "address": address,
                "pid": null,
                "box": other.host,
                "holder": holder,
                "message": message,
            }),
        );
    }
}

/// A mirrored port's `state` for `host.list`: `local_lane` inside a local
/// lane's block, `bound` with a listener on at least one address (one without
/// a loopback here, such as IPv6, has none), `in_use` when something holds
/// it on any address, `failed` when the binds failed with nothing holding it
/// (a port Windows reserves, a low port), else `unbound`.
fn port_state(local: bool, bound: bool, conflicts: &BTreeMap<IpAddr, Value>) -> &'static str {
    if local {
        "local_lane"
    } else if bound {
        "bound"
    } else if conflicts.values().any(|c| !c["holder"].is_null()) {
        "in_use"
    } else if !conflicts.is_empty() {
        "failed"
    } else {
        "unbound"
    }
}

/// `project@lane` for a box lane key.
pub(super) fn lane_ref(lane: &LaneKey) -> String {
    format!("{}@{}", lane.project, lane.lane)
}

/// Proxy state for a box unit: reconnecting while the link is down, and
/// waiting for a sign-in or pairing while it needs a person.
fn route_status(state: &str, phase: Phase) -> RouteStatus {
    match phase {
        Phase::Connected => {}
        Phase::SignIn => return RouteStatus::SignInNeeded,
        Phase::Pairing => return RouteStatus::PairingNeeded,
        Phase::Down | Phase::Over => return RouteStatus::Reconnecting,
    }
    match state {
        "starting" => RouteStatus::Starting,
        "failed" => RouteStatus::Failed {
            log_tail: String::new(),
        },
        state if serving(state) => RouteStatus::Ready,
        _ => RouteStatus::Idle,
    }
}

/// This machine's port blocks, each with a description of its owner for
/// conflict messages. Box ports inside them are never mirrored. The blocks of
/// stopped lanes a chosen box lane is in use instead of are left out: the chosen
/// lane gets its ports too, until the local lane starts again.
async fn local_blocks(engine: &Arc<Inner>, assignment: &Assignment) -> Vec<(PortBlock, String)> {
    let records = match engine.store.port_blocks().await {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "cannot read local port blocks for the mirror");
            return Vec::new();
        }
    };
    let lanes: BTreeMap<_, String> = engine
        .all_lanes()
        .iter()
        .filter_map(|(project, lane)| {
            let st = lane.st();
            Some((st.record.as_ref()?.id, st.reference(&project.name())))
        })
        .collect();
    records
        .into_iter()
        .filter(|record| match record.owner {
            BlockOwner::Lane(id) => lanes
                .get(&id)
                .is_none_or(|lane| !assignment.yielded.contains_key(lane)),
            BlockOwner::Project(_) => true,
        })
        .map(|record| {
            let owner = match record.owner {
                BlockOwner::Lane(id) => lanes.get(&id).map_or_else(
                    || "a local lane".to_owned(),
                    |lane| format!("local lane {lane}"),
                ),
                BlockOwner::Project(id) => engine.project(id).map_or_else(
                    || "a local project's shared infra".to_owned(),
                    |project| format!("the shared infra of local project {}", project.name()),
                ),
            };
            (record.block, owner)
        })
        .collect()
}

/// Name suffixes the box may route here: `localhost` and the configured TLD.
///
/// The TLD only counts once it passes the special-use check, so a box can never
/// get public names routed (and certificates minted) on this machine.
pub(super) fn route_suffixes(engine: &Inner) -> Vec<String> {
    let mut suffixes = vec!["localhost".to_owned()];
    suffixes.extend(engine.resolver_zone().map(|zone| zone.name().to_owned()));
    suffixes
}

/// Whether `host` lies strictly below one of the suffixes: at least one label
/// below it, two for a `*.` wildcard. The suffix itself (`localhost`, the TLD)
/// and a wildcard directly under it would let a box take every name this
/// machine does not route itself, and browsers refuse such wildcards anyway.
fn routable(host: &str, suffixes: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let base = host.strip_prefix("*.").unwrap_or(&host);
    suffixes.iter().any(|suffix| below(base, suffix))
}

/// The box hostname `host`, normalised as local hostnames are (lowercase, no
/// trailing dot), when this machine may route it: a valid DNS name, as the
/// `host_rejected` check requires of local ones, that is [`routable`].
pub(super) fn accepted(host: &str, suffixes: &[String]) -> Option<String> {
    let name = devlanes_config::normalize_host(host).ok()?;
    routable(&name, suffixes).then_some(name)
}

/// Whether `name` has at least one label in front of `parent`.
pub(super) fn below(name: &str, parent: &str) -> bool {
    name.strip_suffix(parent)
        .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
}

/// The process name for `pid`, for port conflict messages.
pub(super) async fn process_name(pid: u32) -> String {
    tokio::task::spawn_blocking(move || {
        devlanes_sys::processes()
            .ok()?
            .into_iter()
            .find(|process| process.pid == pid)
            .map(|process| process.name)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "an unknown process".to_owned())
}

/// A failed bind for warnings: the OS error, or for a denied one what usually
/// denies it ([`denied_hint`]).
pub(super) fn bind_failure(error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        denied_hint()
    } else {
        error.to_string()
    }
}

/// On Unix, ports below 1024 (or below `ip_unprivileged_port_start` on Linux)
/// need privileges, and macOS lets users bind them on the wildcard address only.
#[cfg(unix)]
fn denied_hint() -> String {
    "permission denied, since low ports may need privileges".to_owned()
}

/// Windows needs no privileges for low ports but refuses binds inside its
/// reserved port ranges with access denied, which reads like a permissions
/// problem; name the ranges' command.
#[cfg(windows)]
fn denied_hint() -> String {
    format!(
        "access denied, maybe by a Windows reserved port range that `{}` lists",
        crate::reserved_ports::LIST_COMMAND
    )
}

/// Why one loopback address of a box port is not mirrored.
#[derive(Debug)]
enum BindFailure {
    /// The bind failed.
    Bind(std::io::Error),
    /// The bind worked, but the process with this pid listens on the port's
    /// wildcard address, which Windows lets a loopback bind shadow.
    Shadowed(u32),
}

impl BindFailure {
    /// Whether something on this machine holds the port.
    fn in_use(&self) -> bool {
        match self {
            Self::Bind(error) => error.kind() == std::io::ErrorKind::AddrInUse,
            Self::Shadowed(_) => true,
        }
    }

    /// The failure for a message, when nothing is known to hold the port.
    fn describe(&self) -> String {
        match self {
            Self::Bind(error) => bind_failure(error),
            Self::Shadowed(pid) => format!("pid {pid} listens on its wildcard address"),
        }
    }
}

/// What one attempt to bind a box port on some loopback addresses gave.
#[derive(Debug, Default)]
struct Attempt {
    /// New listeners to serve.
    listeners: Vec<(IpAddr, TcpListener)>,
    /// Addresses that bound, closed again because the port is held on another.
    closed: Vec<IpAddr>,
    /// Addresses that are not mirrored, and why.
    failures: Vec<(IpAddr, BindFailure)>,
}

impl Attempt {
    /// Whether something on this machine holds the port on some address, in
    /// which case no address of it may be mirrored.
    fn held(&self) -> bool {
        self.failures.iter().any(|(_, failure)| failure.in_use())
    }
}

/// Binds `port` on each of `addresses`. An address without a loopback here
/// (no IPv6) is neither bound nor failed. With `shadowed`, the pid of another
/// program listening on the port's wildcard address, every bind that worked
/// fails as [`BindFailure::Shadowed`]. When anything holds the port, the
/// listeners that did bind are closed again: a program that reaches
/// `localhost` on the other family would otherwise get the box instead of
/// what holds the port here.
fn bind_port(addresses: &[IpAddr], port: u16, shadowed: Option<u32>) -> Attempt {
    let mut attempt = Attempt::default();
    for address in addresses {
        match bind(*address, port) {
            Ok(listener) => attempt.listeners.push((*address, listener)),
            Err(error) if error.kind() == std::io::ErrorKind::AddrNotAvailable => {}
            Err(error) => attempt.failures.push((*address, BindFailure::Bind(error))),
        }
    }
    if let Some(pid) = shadowed {
        for (address, _) in std::mem::take(&mut attempt.listeners) {
            attempt.failures.push((address, BindFailure::Shadowed(pid)));
        }
    }
    if attempt.held() {
        for (address, _) in std::mem::take(&mut attempt.listeners) {
            attempt.closed.push(address);
        }
    }
    attempt
}

/// Box ports among `ports` that another program listens on at a wildcard
/// address, with its pid. Windows lets a loopback bind succeed next to such a
/// listener (one without `SO_EXCLUSIVEADDRUSE`), and connections to
/// `localhost` would then reach the box instead of that program. Read once
/// per reconcile for every wanted port, also ones already bound, since such
/// a program may start after the mirror bound its port. Unix refuses a
/// loopback bind next to a wildcard listener, so there is nothing to look up.
#[cfg(windows)]
async fn shadowing(ports: BTreeSet<u16>) -> BTreeMap<u16, u32> {
    if ports.is_empty() {
        return BTreeMap::new();
    }
    match tokio::task::spawn_blocking(devlanes_sys_windows::proc::tcp_listeners).await {
        Ok(Ok(listeners)) => wildcard_holders(&listeners, std::process::id(), &ports),
        Ok(Err(error)) => {
            tracing::warn!(%error, "cannot read the TCP listener table; wildcard listeners are not checked");
            BTreeMap::new()
        }
        Err(_) => BTreeMap::new(),
    }
}

/// Nothing: a loopback bind next to a wildcard listener fails on Unix.
#[cfg(not(windows))]
async fn shadowing(_ports: BTreeSet<u16>) -> BTreeMap<u16, u32> {
    BTreeMap::new()
}

/// The first other process (not `own`) listening on the wildcard address of
/// either family at each of `ports`, from a listener table of `(pid, address)`.
#[cfg(any(windows, test))]
fn wildcard_holders(
    listeners: &[(u32, SocketAddr)],
    own: u32,
    ports: &BTreeSet<u16>,
) -> BTreeMap<u16, u32> {
    let mut holders = BTreeMap::new();
    for (pid, address) in listeners {
        if *pid != own && address.ip().is_unspecified() && ports.contains(&address.port()) {
            holders.entry(address.port()).or_insert(*pid);
        }
    }
    holders
}

/// A local connection's stream that notes who spoke last and when: `true`
/// when bytes from the box went out to the local client.
struct Watched<'a> {
    stream: &'a mut TcpStream,
    last: &'a Mutex<Option<(Instant, bool)>>,
}

impl tokio::io::AsyncRead for Watched<'_> {
    /// Reads from the local client and notes it spoke.
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = std::pin::Pin::new(&mut *self.stream).poll_read(cx, buf);
        if matches!(polled, std::task::Poll::Ready(Ok(()))) && buf.filled().len() > before {
            *lock(self.last) = Some((Instant::now(), false));
        }
        polled
    }
}

impl tokio::io::AsyncWrite for Watched<'_> {
    /// Writes the box's bytes to the local client and notes the box spoke.
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut *self.stream).poll_write(cx, data);
        if matches!(polled, std::task::Poll::Ready(Ok(written)) if written > 0) {
            *lock(self.last) = Some((Instant::now(), true));
        }
        polled
    }

    /// Flushes the local side.
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.stream).poll_flush(cx)
    }

    /// Half-closes the local side.
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.stream).poll_shutdown(cx)
    }
}

/// Resolves once a connection to a box that lost its lane may close: the box
/// spoke last and has been quiet for [`SWITCH_QUIET`], or [`SWITCH_LIMIT`]
/// passed. A local client that spoke last may still wait for an answer.
async fn drained(last: &Mutex<Option<(Instant, bool)>>) {
    let deadline = Instant::now() + SWITCH_LIMIT;
    loop {
        let now = Instant::now();
        let answered = match *lock(last) {
            Some((at, true)) => Some(at + SWITCH_QUIET),
            _ => None,
        };
        let wake = match answered {
            Some(quiet) if quiet <= now => return,
            Some(quiet) => quiet.min(deadline),
            None => (now + SWITCH_QUIET).min(deadline),
        };
        if now >= deadline {
            return;
        }
        tokio::time::sleep_until(wake).await;
    }
}

/// Binds a mirror listener. `SO_REUSEADDR` only on Linux, where it merely skips
/// `TIME_WAIT` on rebinds: on macOS it would let this listener shadow another
/// process bound to the wildcard address instead of reporting the conflict.
pub(super) fn bind(address: IpAddr, port: u16) -> std::io::Result<TcpListener> {
    let socket = match address {
        IpAddr::V4(_) => TcpSocket::new_v4()?,
        IpAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(cfg!(target_os = "linux"))?;
    socket.bind(SocketAddr::new(address, port))?;
    socket.listen(1024)
}

/// Accepts local connections and splices each into a data stream to box port
/// `port` of the box `target` leads to at that moment. With no box behind it,
/// which happens only until the next reconcile unbinds it, a connection is
/// closed at once.
pub(super) async fn accept_loop(
    listener: TcpListener,
    port: u16,
    target: Arc<Target>,
    engine: Weak<Inner>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                if let Some(owner) = target.current()
                    && let Some(conn) = owner.conn.upgrade()
                {
                    tokio::spawn(splice(stream, port, conn, engine.clone(), owner.conns));
                }
            }
            Err(error) => {
                // Out of descriptors and similar: back off instead of spinning.
                tracing::debug!(%error, port, "mirror accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// One local connection to box port `port`. A connection that cannot reach the
/// box's service is recorded on the port and reported with `open_failed`.
///
/// When `conns` is cancelled, because the box lost the lane or the port, the
/// connection is reset once the box spoke last and then stayed quiet for
/// [`SWITCH_QUIET`], or after [`SWITCH_LIMIT`]. An idle keep-alive or
/// WebSocket connection closes at once, and an answer under way still gets
/// through, so a switch fails no request that reached the old box.
async fn splice(
    mut stream: TcpStream,
    port: u16,
    conn: Arc<HostConn>,
    engine: Weak<Inner>,
    conns: CancellationToken,
) {
    let session = match conn.session() {
        Some(session) => session,
        None => match conn.session_within(HOLD).await {
            Some(session) => session,
            // A disconnect in progress closes this listener anyway, and a
            // switch away from this box is no failure to report.
            None if conn.cancel.is_cancelled() || conns.is_cancelled() => return,
            None if conn.needs_auth() => {
                let message = format!("box {} is waiting for you to sign in again", conn.name);
                open_failed(&engine, &conn, port, "needs_auth", message);
                return;
            }
            None if conn.needs_pairing() => {
                let message = format!("box {} is waiting for a new pairing code", conn.name);
                open_failed(&engine, &conn, port, "needs_pairing", message);
                return;
            }
            None => {
                let message = match conn.link_error() {
                    Some(error) => format!("box {} is not connected: {error}", conn.name),
                    None => format!("box {} is not connected", conn.name),
                };
                open_failed(&engine, &conn, port, "not_connected", message);
                return;
            }
        },
    };
    match session.open(DialTarget::Port { port }).await {
        Ok(remote) => {
            if lock(&conn.view).open_errors.remove(&port).is_some() {
                conn.mark_changed();
            }
            let last = Mutex::new(None);
            let spliced = {
                let watched = Watched {
                    stream: &mut stream,
                    last: &last,
                };
                let spliced = devlanes_transport::splice(
                    watched,
                    remote,
                    devlanes_transport::HALF_CLOSE_IDLE,
                );
                tokio::pin!(spliced);
                let first = tokio::select! {
                    spliced = &mut spliced => Some(spliced),
                    () = conns.cancelled() => None,
                };
                match first {
                    Some(spliced) => Some(spliced),
                    None => tokio::select! {
                        spliced = &mut spliced => Some(spliced),
                        () = drained(&last) => None,
                    },
                }
            };
            match spliced {
                Some(Err(error)) => tracing::debug!(%error, port, "mirrored connection ended"),
                Some(Ok(_)) => {}
                // A reset leaves no TIME_WAIT behind, and the client sees
                // the connection end at once instead of after a timeout.
                None => {
                    let _ = stream.set_zero_linger();
                }
            }
        }
        Err(error) => {
            let (reason, message) = open_error(&error, port);
            open_failed(&engine, &conn, port, reason, message);
        }
    }
}

/// The reason code and message of a failed open of box port `port`, as
/// `last_error` and the `open_failed` warning show them. The messages leave
/// the port out, since both already name it.
fn open_error(error: &OpenError, port: u16) -> (&'static str, String) {
    match error {
        OpenError::Rejected { reason, message } => match reason {
            RejectReason::NotRunning => {
                ("not_running", "the unit is not running on the box".into())
            }
            RejectReason::Hidden => ("hidden", "the unit is hidden on the box".into()),
            RejectReason::NotRegistered => (
                "not_registered",
                "no running unit on the box has this port".into(),
            ),
            RejectReason::DialFailed => {
                // The box words it `dial port <port>: <error>` or `dial port <port> timed out`.
                let detail = message
                    .strip_prefix(&format!("dial port {port}"))
                    .map_or(message.as_str(), |rest| rest.trim_start_matches(':').trim());
                (
                    "dial_failed",
                    format!("nothing on the box accepted it ({detail})"),
                )
            }
        },
        OpenError::Timeout => ("timeout", "the box did not answer in time".into()),
        OpenError::Interrupted => ("interrupted", "the link dropped while it opened".into()),
        OpenError::Session(_) | OpenError::Wire(_) => (
            "session_lost",
            format!("the link to the box was lost: {error}"),
        ),
    }
}

/// Records a failed open of `port` in the host's view and emits a
/// `remote.warning` with code `open_failed`, at most once a minute per port
/// and reason.
fn open_failed(
    engine: &Weak<Inner>,
    conn: &HostConn,
    port: u16,
    reason: &'static str,
    message: String,
) {
    let failure = OpenFailure {
        reason,
        message: message.clone(),
        at_ms: super::now_ms(),
    };
    let due = lock(&conn.view).record_failure(port, failure, Instant::now());
    if !due {
        tracing::debug!(host = %conn.name, port, reason, %message, "mirrored connection failed");
        return;
    }
    // A new `last_error`; repeats within the minute only move its time.
    conn.mark_changed();
    let Some(engine) = engine.upgrade() else {
        return;
    };
    let unit = conn
        .registry
        .current()
        .units()
        .find(|unit| unit.ports.iter().any(|known| known.port == port))
        .map(|unit| (lane_ref(&unit.key.lane), unit.key.name.clone()));
    let (lane, unit) = unit.unzip();
    tracing::warn!(host = %conn.name, port, reason, %message, "mirrored connection failed");
    engine.bus.emit(
        wire::WARNING_EVENT,
        json!({
            "host": conn.name,
            "code": "open_failed",
            "port": port,
            "lane": lane,
            "unit": unit,
            "reason": reason,
            "message": message,
        }),
    );
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    /// A failure with `reason` and a message naming it.
    fn failure(reason: &'static str) -> OpenFailure {
        OpenFailure {
            reason,
            message: format!("{reason} happened"),
            at_ms: 1,
        }
    }

    /// One warning per port and reason a minute; every failure still becomes
    /// the port's `last_error`.
    #[test]
    fn failed_opens_warn_once_a_minute_per_port_and_reason() {
        let mut view = View {
            ports: vec![json!({"port": 3000}), json!({"port": 3001})],
            ..View::default()
        };
        let start = Instant::now();
        assert!(view.record_failure(3000, failure("not_running"), start));
        assert!(!view.record_failure(3000, failure("not_running"), start + Duration::from_secs(5)));
        assert!(view.record_failure(3000, failure("timeout"), start + Duration::from_secs(6)));
        assert!(view.record_failure(3001, failure("not_running"), start + Duration::from_secs(7)));
        assert!(view.record_failure(3000, failure("not_running"), start + OPEN_WARNING_EVERY));
        let ports = view.ports_json();
        assert_eq!(ports[0]["last_error"]["reason"], "not_running");
        assert_eq!(ports[0]["last_error"]["message"], "not_running happened");
        view.open_errors.remove(&3001);
        assert_eq!(view.ports_json()[1]["last_error"], Value::Null);
    }

    /// A failure on a port that is not listed (a relay only) still warns but
    /// keeps no `last_error`, which `remote ls` could not show.
    #[test]
    fn unlisted_ports_warn_without_a_last_error() {
        let mut view = View {
            ports: vec![json!({"port": 3000})],
            ..View::default()
        };
        assert!(view.record_failure(4000, failure("not_running"), Instant::now()));
        assert!(view.open_errors.is_empty());
        assert!(view.warned.contains_key(&(4000, "not_running")));
    }

    /// Pruning keeps failures of listed ports and recent warning times of
    /// listed or relayed ports; the rest goes.
    #[test]
    fn pruning_drops_gone_ports_and_old_warnings() {
        let mut view = View {
            ports: vec![json!({"port": 3000}), json!({"port": 3001})],
            ..View::default()
        };
        let start = Instant::now();
        view.record_failure(3000, failure("not_running"), start);
        view.record_failure(3001, failure("timeout"), start + OPEN_WARNING_EVERY);
        view.record_failure(4000, failure("not_running"), start + OPEN_WARNING_EVERY);
        view.record_failure(5000, failure("not_running"), start + OPEN_WARNING_EVERY);
        view.ports.pop();
        view.prune(
            start + OPEN_WARNING_EVERY + Duration::from_secs(1),
            |port| port == 4000,
        );
        assert_eq!(view.open_errors.keys().copied().collect::<Vec<_>>(), [3000]);
        assert_eq!(
            view.warned.keys().copied().collect::<Vec<_>>(),
            [(4000, "not_running")]
        );
    }

    /// A bind that failed with nothing holding the port is `failed`, one held
    /// by something `in_use`; a bound address or a local block wins.
    #[test]
    fn port_states_tell_a_held_port_from_a_failed_bind() {
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let v6 = IpAddr::V6(std::net::Ipv6Addr::LOCALHOST);
        let held = BTreeMap::from([(v4, json!({"holder": "nginx (pid 812)"}))]);
        let failed = BTreeMap::from([(v4, json!({"holder": null, "message": "access denied"}))]);
        let both = BTreeMap::from([
            (v4, json!({"holder": null})),
            (v6, json!({"holder": "box alpha (app@0:web)"})),
        ]);
        assert_eq!(port_state(false, false, &held), "in_use");
        assert_eq!(port_state(false, false, &failed), "failed");
        assert_eq!(port_state(false, false, &both), "in_use");
        assert_eq!(port_state(false, true, &failed), "bound");
        assert_eq!(port_state(true, true, &BTreeMap::new()), "local_lane");
        assert_eq!(port_state(false, false, &BTreeMap::new()), "unbound");
    }

    /// Each refusal reason gets its own wording, without the port; a dial
    /// failure keeps the box's detail.
    #[test]
    fn open_errors_are_worded_per_reason() {
        let rejected = |reason, message: &str| OpenError::Rejected {
            reason,
            message: message.to_owned(),
        };
        let cases = [
            (
                rejected(RejectReason::NotRunning, "target's unit is not running"),
                "not_running",
                "the unit is not running on the box",
            ),
            (
                rejected(RejectReason::Hidden, "target belongs to a hidden unit"),
                "hidden",
                "the unit is hidden on the box",
            ),
            (
                rejected(RejectReason::NotRegistered, "target is not registered"),
                "not_registered",
                "no running unit on the box has this port",
            ),
            (
                rejected(
                    RejectReason::DialFailed,
                    "dial port 3000: connection refused",
                ),
                "dial_failed",
                "nothing on the box accepted it (connection refused)",
            ),
            (
                rejected(RejectReason::DialFailed, "dial port 3000 timed out"),
                "dial_failed",
                "nothing on the box accepted it (timed out)",
            ),
            (
                OpenError::Interrupted,
                "interrupted",
                "the link dropped while it opened",
            ),
        ];
        for (error, reason, message) in cases {
            assert_eq!(open_error(&error, 3000), (reason, message.to_owned()));
        }
    }

    /// A box name must lie below `localhost` or the TLD: the suffix itself and
    /// a wildcard directly under it are refused, deeper names and wildcards
    /// pass, case and a trailing dot aside.
    #[test]
    fn box_names_need_a_label_below_the_suffix() {
        let suffixes = ["localhost".to_owned(), "dev.internal".to_owned()];
        for name in [
            "web.acme.localhost",
            "acme.localhost",
            "*.acme.localhost",
            "Web.Acme.Localhost.",
            "api.dev.internal",
            "*.app.dev.internal",
        ] {
            assert!(routable(name, &suffixes), "{name} is refused");
        }
        for name in [
            "localhost",
            "localhost.",
            "*.localhost",
            "dev.internal",
            "*.dev.internal",
            "internal",
            "xlocalhost",
            "web.example.com",
            "*.com",
        ] {
            assert!(!routable(name, &suffixes), "{name} is routable");
        }
    }

    /// A box name must also be a valid DNS name, as a local one must, and
    /// comes out lowercase without its trailing dot.
    #[test]
    fn box_names_must_be_valid_dns_names() {
        let suffixes = ["localhost".to_owned()];
        assert_eq!(
            accepted("Web.Acme.Localhost.", &suffixes).as_deref(),
            Some("web.acme.localhost")
        );
        assert_eq!(
            accepted("*.acme.localhost", &suffixes).as_deref(),
            Some("*.acme.localhost")
        );
        for name in [
            "*.*.localhost",
            "a..localhost",
            "x/y.localhost",
            "a*.acme.localhost",
            "-web.acme.localhost",
            "web_1.acme.localhost",
            "*.localhost",
        ] {
            assert_eq!(accepted(name, &suffixes), None, "{name}");
        }
    }

    /// A port held on one loopback address is not bound on the other: every
    /// listener of the attempt is closed and the held address fails as in use.
    /// A free port binds on both, and a wildcard holder fails every bind.
    #[tokio::test]
    async fn a_port_held_on_one_family_is_not_bound_on_the_other() {
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let v6 = IpAddr::V6(std::net::Ipv6Addr::LOCALHOST);
        // Without an IPv6 loopback only the IPv4 half can be checked.
        let has_v6 = std::net::TcpListener::bind((v6, 0)).is_ok();
        let both = if has_v6 { vec![v4, v6] } else { vec![v4] };
        for held in both.clone() {
            let holder = std::net::TcpListener::bind((held, 0)).expect("holder");
            let port = holder.local_addr().expect("holder address").port();
            let attempt = bind_port(&both, port, None);
            assert!(attempt.held(), "held on {held}: {attempt:?}");
            assert!(attempt.listeners.is_empty(), "{attempt:?}");
            assert!(
                attempt
                    .failures
                    .iter()
                    .any(|(address, failure)| *address == held && failure.in_use()),
                "{attempt:?}"
            );
            // The other family bound and was closed again, unless something
            // else on this machine happens to hold the number there too.
            for address in &both {
                let failed = attempt.failures.iter().any(|(a, _)| a == address);
                assert!(failed || attempt.closed.contains(address), "{attempt:?}");
            }
            drop(holder);
        }

        // Port 0 picks a free port, so nothing holds it.
        let attempt = bind_port(&[v4], 0, None);
        assert!(!attempt.held(), "{attempt:?}");
        assert_eq!(attempt.listeners.len(), 1, "{attempt:?}");
        drop(attempt);

        let attempt = bind_port(&[v4], 0, Some(812));
        assert!(
            attempt.held() && attempt.listeners.is_empty(),
            "{attempt:?}"
        );
        assert!(
            matches!(attempt.failures.as_slice(), [(address, BindFailure::Shadowed(812))] if *address == v4),
            "{attempt:?}"
        );
    }

    /// Only other processes' wildcard listeners on the asked ports count, the
    /// first one per port; loopback listeners and this daemon's own do not.
    #[test]
    fn wildcard_listeners_of_others_shadow_a_port() {
        let any4 = |port| SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
        let any6 = |port| SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), port);
        let lo = |port| SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let listeners = [
            (7, any4(3000)),
            (8, any6(3000)),
            (9, lo(3001)),
            (1, any4(3002)),
            (10, any6(3003)),
            (11, any4(4000)),
        ];
        let ports = BTreeSet::from([3000, 3001, 3002, 3003]);
        assert_eq!(
            wildcard_holders(&listeners, 1, &ports),
            BTreeMap::from([(3000, 7), (3003, 10)])
        );
    }
}
