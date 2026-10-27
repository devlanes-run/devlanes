//! Makes a box's non-hidden endpoints local (plan 14.4): every allocated port of
//! its serving services and containers is bound on this machine's loopback
//! addresses under the same number, and its hostnames join the local proxy's
//! routing table. Each accepted connection becomes one mux data stream to the box.
//!
//! Local lanes win: a box port inside one of this machine's port blocks is never
//! bound (its hostnames still route through a relay), and local allocations keep
//! clear of the box's ports (`Remotes::advertised_ports`). A port something here
//! holds on any loopback address, or on a wildcard address, is bound on none. A
//! box lane with a hostname that overlaps a local route, by name or wildcard, is
//! skipped. Between boxes, the one connected first keeps a hostname both serve,
//! and a port another box's mirror holds is reported with that box.
//!
//! Listeners stay bound while the link reconnects; connections accepted meanwhile
//! wait briefly for the new session and the proxy shows a reconnecting page.
//! They also stay while the link waits for a sign-in; connections then fail at
//! once and the proxy shows the sign-in page.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Weak},
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

use super::HostConn;
use crate::{
    engine::Inner,
    integrations::{RouteEntry, RouteStatus},
    model::lock,
};

/// How long a connection accepted during a reconnect waits for the new session.
const HOLD: Duration = Duration::from_secs(10);

/// Shortest time between two `open_failed` warnings for the same port and reason.
const OPEN_WARNING_EVERY: Duration = Duration::from_secs(60);

/// What a host mirrors right now, for routes and status output.
#[derive(Debug, Clone, Default)]
pub(super) struct View {
    pub routes: Vec<RouteEntry>,
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

/// Why a box lane is skipped: its hostnames that are served already.
#[derive(Debug, Default)]
struct Collision {
    hosts: BTreeSet<String>,
    /// A local lane serves at least one of them.
    local: bool,
    /// Boxes connected earlier that serve the others.
    boxes: BTreeSet<String>,
}

impl Collision {
    /// The earlier box to name, unless a local lane is involved: local lanes
    /// win over every box, so they are the reason then.
    fn other_box(&self) -> Option<&String> {
        if self.local { None } else { self.boxes.first() }
    }

    /// The reason `remote ls` shows next to the skipped lane.
    fn reason(&self) -> String {
        match self.other_box() {
            Some(other) => format!("box {other}, connected earlier, serves its hostnames"),
            None => "this machine serves its hostnames".to_owned(),
        }
    }
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
struct Bound {
    address: IpAddr,
    task: JoinHandle<()>,
}

impl Drop for Bound {
    /// Stops accepting; the listener closes with the task.
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Stops the accept loops of `bounds` and waits until their listeners are closed.
/// Dropping a [`Bound`] only schedules that, and another box's mirror woken right
/// after would then find the port still taken by this daemon.
async fn unbind(bounds: Vec<Bound>) {
    for bound in &bounds {
        bound.task.abort();
    }
    for mut bound in bounds {
        let _ = (&mut bound.task).await;
    }
}

/// A mirrored box port and the local addresses it holds.
struct PortMirror {
    unit: String,
    lane: String,
    kind: UnitKind,
    bound: Vec<Bound>,
    /// Addresses whose bind failed, with what the port inspector found.
    conflicts: BTreeMap<IpAddr, Value>,
    /// Set while the port lies in a local lane's block: the message and the owner.
    local: Option<(String, String)>,
}

impl PortMirror {
    /// Whether `127.0.0.1:<port>` reaches the box, which the proxy dials first.
    fn owns_v4(&self) -> bool {
        self.bound
            .iter()
            .any(|bound| bound.address == IpAddr::V4(Ipv4Addr::LOCALHOST))
    }
}

/// Mirror state owned by one host's mirror task.
struct Mirror {
    /// Handed to accept loops, which report failed opens on the event bus.
    engine: Weak<Inner>,
    conn: Arc<HostConn>,
    addresses: Vec<IpAddr>,
    ports: BTreeMap<u16, PortMirror>,
    /// Ephemeral `127.0.0.1` listeners for routed box ports this machine could not
    /// bind under their own number: box port -> listener.
    relays: BTreeMap<u16, (u16, Bound)>,
    skipped: BTreeSet<LaneKey>,
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
        relays: BTreeMap::new(),
        skipped: BTreeSet::new(),
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
        if phase == Phase::Over {
            mirror.clear(&inner).await;
        } else {
            mirror.reconcile(&inner, &current, phase).await;
        }
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
    // Closed before `HostConn::close` returns, so a box woken by the disconnect
    // can bind these numbers; relays close as `mirror` drops.
    let bounds = std::mem::take(&mut mirror.ports)
        .into_values()
        .flat_map(|port| port.bound)
        .collect();
    unbind(bounds).await;
}

impl Mirror {
    /// Unbinds everything and drops the host's routes, once the link gave up.
    async fn clear(&mut self, engine: &Arc<Inner>) {
        let bounds = std::mem::take(&mut self.ports)
            .into_values()
            .flat_map(|port| port.bound)
            .collect();
        unbind(bounds).await;
        self.relays.clear();
        self.skipped.clear();
        let (had_routes, had_ports) = {
            let mut view = lock(&self.conn.view);
            let had = (!view.routes.is_empty(), !view.bound.is_empty());
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
    }

    /// Binds new ports, drops stale ones, retries conflicts and republishes routes.
    async fn reconcile(&mut self, engine: &Arc<Inner>, registry: &Registry, phase: Phase) {
        let local_hosts: BTreeSet<String> = engine
            .local_route_entries()
            .into_iter()
            .map(|route| route.host)
            .collect();
        let suffixes = route_suffixes(engine);
        // Hidden units and tasks are listed by the box but never mirrored.
        let visible: Vec<&UnitInfo> = registry.units().filter(|unit| unit.mirrorable()).collect();

        // Hostnames of boxes connected earlier, which keep them over this one.
        let earlier: Vec<(String, BTreeSet<String>)> = engine
            .remote
            .all()
            .into_iter()
            .filter(|other| other.serial < self.conn.serial)
            .map(|other| {
                let hosts = lock(&other.view)
                    .routes
                    .iter()
                    .map(|route| route.host.clone())
                    .collect();
                (other.name.clone(), hosts)
            })
            .collect();

        // A box lane whose hostname this machine or an earlier box already serves
        // is skipped whole: renaming it would break the CORS origins and cookies
        // the box chose. Patterns count, not only equal names: a box name under
        // a local wildcard, or a box wildcard over a local name, would take
        // requests the local lane serves today.
        let mut collisions: BTreeMap<LaneKey, Collision> = BTreeMap::new();
        for unit in &visible {
            for host in &unit.hosts {
                let name = host.host.to_ascii_lowercase();
                let owner = if local_hosts.iter().any(|local| overlap(local, &name)) {
                    None
                } else if let Some((other, _)) = earlier
                    .iter()
                    .find(|(_, hosts)| hosts.iter().any(|held| overlap(held, &name)))
                {
                    Some(other.clone())
                } else {
                    continue;
                };
                let collision = collisions.entry(unit.key.lane.clone()).or_default();
                collision.hosts.insert(name);
                match owner {
                    Some(other) => {
                        collision.boxes.insert(other);
                    }
                    None => collision.local = true,
                }
            }
        }
        for (lane, collision) in &collisions {
            if !self.skipped.contains(lane) {
                let other = collision.other_box();
                tracing::warn!(host = %self.conn.name, lane = %lane_ref(lane), hosts = ?collision.hosts, ?other, "lane skipped: its hostnames are served already");
                engine.bus.emit(
                    wire::WARNING_EVENT,
                    json!({
                        "host": self.conn.name,
                        "code": "lane_skipped",
                        "lane": lane_ref(lane),
                        "hosts": collision.hosts,
                        "box": other,
                    }),
                );
            }
        }
        self.skipped = collisions.keys().cloned().collect();
        let mirrored: Vec<&UnitInfo> = visible
            .into_iter()
            .filter(|unit| !self.skipped.contains(&unit.key.lane))
            .collect();

        // Idle units are not bound, so their numbers stay free until they start.
        let mut wanted: BTreeMap<u16, &UnitInfo> = BTreeMap::new();
        for unit in mirrored.iter().filter(|unit| serving(&unit.state)) {
            for port in &unit.ports {
                wanted.entry(port.port).or_insert(unit);
            }
        }
        let local = local_blocks(engine).await;
        let mut released = Vec::new();
        let stale: Vec<u16> = self
            .ports
            .keys()
            .copied()
            .filter(|port| !wanted.contains_key(port))
            .collect();
        for port in stale {
            released.extend(self.ports.remove(&port).into_iter().flat_map(|m| m.bound));
        }
        let unbound: BTreeSet<u16> = wanted
            .keys()
            .copied()
            .filter(|port| {
                self.ports
                    .get(port)
                    .is_none_or(|mirror| mirror.bound.len() < self.addresses.len())
            })
            .collect();
        let shadowing = shadowing(unbound).await;
        for (port, unit) in &wanted {
            let mirror = self.ports.entry(*port).or_insert_with(|| PortMirror {
                unit: unit.key.name.clone(),
                lane: lane_ref(&unit.key.lane),
                kind: unit.key.kind,
                bound: Vec::new(),
                conflicts: BTreeMap::new(),
                local: None,
            });
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
                released.append(&mut mirror.bound);
                mirror.conflicts.clear();
                if mirror
                    .local
                    .as_ref()
                    .is_none_or(|(_, known)| known != owner)
                {
                    let message = format!("port {port} is allocated to {owner}; not mirrored");
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
            let missing: Vec<IpAddr> = self
                .addresses
                .iter()
                .copied()
                .filter(|address| !mirror.bound.iter().any(|b| b.address == *address))
                .collect();
            let attempt = bind_port(&missing, *port, shadowing.get(port).copied());
            if attempt.held() {
                // Held on one loopback address, the port stays off the others
                // too. Closed now, so the holder lookup below cannot find this
                // daemon's own listener.
                unbind(std::mem::take(&mut mirror.bound)).await;
            }
            for address in &attempt.closed {
                mirror.conflicts.remove(address);
            }
            for (address, listener) in attempt.listeners {
                mirror.conflicts.remove(&address);
                mirror.bound.push(Bound {
                    address,
                    task: tokio::spawn(accept_loop(
                        listener,
                        *port,
                        self.conn.clone(),
                        self.engine.clone(),
                    )),
                });
            }
            for (address, failure) in attempt.failures {
                let in_use = failure.in_use();
                // Another box's mirror runs in this daemon, so its pid
                // would only name devlanes itself.
                let other_box = match &failure {
                    BindFailure::Bind(_) if in_use => other_box_holding(engine, &self.conn, *port),
                    _ => None,
                };
                let pid = match &failure {
                    BindFailure::Shadowed(pid) => Some(*pid),
                    BindFailure::Bind(_) if in_use && other_box.is_none() => {
                        engine.integ.ports.holder(*port).await
                    }
                    BindFailure::Bind(_) => None,
                };
                // This daemon itself: usually another box's mirror that bound
                // the port and has not listed it yet. Its reconcile wakes
                // this mirror once it has, and the check above names it.
                let own = pid == Some(std::process::id());
                // What keeps the port when something does: the
                // message's subject. A bind that failed for another
                // reason has none.
                let holder = if let Some((other, unit)) = &other_box {
                    Some(format!("box {other} ({unit})"))
                } else if own {
                    Some("this devlanes daemon".to_owned())
                } else if in_use {
                    Some(match pid {
                        Some(pid) => format!("{} (pid {pid})", process_name(pid).await),
                        None => "another process".to_owned(),
                    })
                } else {
                    None
                };
                let message = match &holder {
                    Some(holder) => {
                        format!("port {port} is in use on this machine by {holder}; not mirrored")
                    }
                    None => format!(
                        "port {port} cannot be bound on {address}: {}; not mirrored",
                        failure.describe()
                    ),
                };
                // Reported once per holder: again when another box, or
                // something other than a box, holds it now. A port this
                // daemon holds is only listed until a wake tells why.
                let holder_box = other_box.as_ref().map(|(other, _)| other.as_str());
                let known = mirror.conflicts.get(&address);
                let report = !own
                    && known.is_none_or(|known| {
                        known["box"].as_str() != holder_box
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
                            "box": other_box.as_ref().map(|(other, _)| other),
                            "held_by": other_box.as_ref().map(|(_, unit)| unit),
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
                        "box": other_box.as_ref().map(|(other, _)| other),
                        "holder": holder,
                        "message": message,
                    }),
                );
            }
        }
        unbind(released).await;

        // Hostnames: the proxy dials 127.0.0.1 first, so a routed port this machine
        // does not hold there gets a relay on an ephemeral port instead.
        let mut routes = Vec::new();
        let mut relayed = BTreeSet::new();
        let mut refused_hosts = Vec::new();
        let box_https_port = lock(&self.conn.info).https_port;
        for unit in &mirrored {
            let lane = registry
                .lanes()
                .find(|lane| lane.key == unit.key.lane)
                .cloned();
            for host in &unit.hosts {
                // The local CA signs whatever the routes name, so a box may only
                // claim names this machine already routes to itself.
                if !routable(&host.host, &suffixes) {
                    refused_hosts.push(json!({
                        "host": host.host,
                        "lane": lane_ref(&unit.key.lane),
                        "unit": unit.key.name,
                    }));
                    continue;
                }
                let owned = self.ports.get(&host.port).is_some_and(PortMirror::owns_v4);
                let local_port = if owned {
                    host.port
                } else {
                    relayed.insert(host.port);
                    match self.relay(host.port) {
                        Some(port) => port,
                        None => continue,
                    }
                };
                let reference = lane_ref(&unit.key.lane);
                let box_name = &self.conn.name;
                // The proxy shows the hint on the page of the route's state.
                let start_hint = match phase {
                    Phase::SignIn => {
                        format!("Sign in to {box_name} again: devlanes remote connect {box_name}")
                    }
                    Phase::Pairing => format!(
                        "Pair with {box_name} again: run devlanes daemon pair on {box_name}, then devlanes remote connect {box_name} --code <code>"
                    ),
                    _ => format!("Start it on {box_name}: devlanes start --lane {reference}"),
                };
                routes.push(RouteEntry {
                    host: host.host.to_ascii_lowercase(),
                    port: local_port,
                    fallback: host.fallback,
                    host_header_upstream: host.host_header == HostHeader::Upstream,
                    status: route_status(&unit.state, phase),
                    start_hint,
                    project: unit.key.lane.project.clone(),
                    lane: reference,
                    unit: unit.key.name.clone(),
                    certificate_names: lane
                        .as_ref()
                        .and_then(|lane| lane.domain.clone())
                        .filter(|domain| routable(domain, &suffixes))
                        .into_iter()
                        .collect(),
                    remote: true,
                    box_https_port,
                    // The local end is the mirror's listener, on either loopback.
                    address: None,
                });
            }
        }
        if !refused_hosts.is_empty() && lock(&self.conn.view).refused_hosts != refused_hosts {
            tracing::warn!(host = %self.conn.name, ?refused_hosts, "box hostnames outside the local suffixes skipped");
            engine.bus.emit(
                wire::WARNING_EVENT,
                json!({"host": self.conn.name, "code": "hosts_skipped", "hosts": refused_hosts}),
            );
        }
        self.relays.retain(|port, _| relayed.contains(port));

        let mut view = View {
            routes,
            ports: self
                .ports
                .iter()
                .map(|(port, mirror)| {
                    let mut conflicts: Vec<Value> = mirror.conflicts.values().cloned().collect();
                    if let Some((message, owner)) = &mirror.local {
                        conflicts.push(json!({"message": message, "local": owner}));
                    }
                    let state = port_state(
                        mirror.local.is_some(),
                        !mirror.bound.is_empty(),
                        &mirror.conflicts,
                    );
                    json!({
                        "port": port,
                        "lane": mirror.lane,
                        "unit": mirror.unit,
                        "kind": mirror.kind,
                        "state": state,
                        "addresses": mirror.bound.iter().map(|b| b.address).collect::<Vec<_>>(),
                        "conflicts": conflicts,
                        "relay": self.relays.get(port).map(|(local, _)| local),
                    })
                })
                .collect(),
            bound: self
                .ports
                .iter()
                .filter(|(_, mirror)| !mirror.bound.is_empty())
                .map(|(port, mirror)| (*port, format!("{}:{}", mirror.lane, mirror.unit)))
                .collect(),
            skipped: collisions
                .iter()
                .map(|(lane, collision)| {
                    json!({
                        "lane": lane_ref(lane),
                        "hosts": collision.hosts,
                        "box": collision.other_box(),
                        "reason": collision.reason(),
                    })
                })
                .collect(),
            refused_hosts,
            open_errors: BTreeMap::new(),
            warned: BTreeMap::new(),
        };
        let (routes_changed, others_affected) = {
            let mut current = lock(&self.conn.view);
            // Other boxes care which names and ports this one holds, not their states.
            let hosts = |routes: &[RouteEntry]| -> BTreeSet<String> {
                routes.iter().map(|route| route.host.clone()).collect()
            };
            let changed = (
                current.routes != view.routes,
                hosts(&current.routes) != hosts(&view.routes)
                    || current.bound.keys().ne(view.bound.keys()),
            );
            if changed.0
                || current.ports != view.ports
                || current.skipped != view.skipped
                || current.refused_hosts != view.refused_hosts
            {
                self.conn.mark_changed();
            }
            // Failed opens outlive a reconcile, but not the port they are about.
            view.open_errors = std::mem::take(&mut current.open_errors);
            view.warned = std::mem::take(&mut current.warned);
            view.prune(Instant::now(), |port| self.relays.contains_key(&port));
            *current = view;
            changed
        };
        if routes_changed {
            engine.publish_routes();
        }
        if others_affected {
            engine.remote.wake_others(self.conn.serial);
        }
    }

    /// The local relay port for box port `port`, binding one on first use.
    fn relay(&mut self, port: u16) -> Option<u16> {
        if let Some((local, _)) = self.relays.get(&port) {
            return Some(*local);
        }
        let address = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let listener = match bind(address, 0) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(host = %self.conn.name, port, %error, "cannot bind a route relay");
                return None;
            }
        };
        let local = listener.local_addr().ok()?.port();
        let task = tokio::spawn(accept_loop(
            listener,
            port,
            self.conn.clone(),
            self.engine.clone(),
        ));
        self.relays.insert(port, (local, Bound { address, task }));
        Some(local)
    }
}

/// A mirrored port's `state` for `host.list`: `local_lane` inside a local
/// lane's block, `bound` with a listener on at least one address (a port held
/// on one address and bound on the other still works), `in_use` when
/// something holds it, `failed` when the binds failed with nothing holding it
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

/// The box other than `conn` whose mirror holds `port` here, with the unit
/// behind it (`project@lane:unit`).
fn other_box_holding(engine: &Inner, conn: &HostConn, port: u16) -> Option<(String, String)> {
    engine
        .remote
        .all()
        .into_iter()
        .filter(|other| other.serial != conn.serial)
        .find_map(|other| {
            let unit = lock(&other.view).bound.get(&port).cloned()?;
            Some((other.name.clone(), unit))
        })
}

/// `project@lane` for a box lane key.
fn lane_ref(lane: &LaneKey) -> String {
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
/// conflict messages. Box ports inside them are never mirrored.
async fn local_blocks(engine: &Arc<Inner>) -> Vec<(PortBlock, String)> {
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

/// Whether `name` has at least one label in front of `parent`.
fn below(name: &str, parent: &str) -> bool {
    name.strip_suffix(parent)
        .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
}

/// Whether some name matches both route patterns, each an exact name or a
/// `*.` wildcard covering any depth below its parent (but not the parent),
/// as the proxy matches them. Case and a trailing dot are ignored.
fn overlap(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('.').to_ascii_lowercase();
    let b = b.trim_end_matches('.').to_ascii_lowercase();
    match (a.strip_prefix("*."), b.strip_prefix("*.")) {
        (None, None) => a == b,
        (Some(parent), None) => below(&b, parent),
        (None, Some(parent)) => below(&a, parent),
        // Two wildcards share every name below the deeper parent.
        (Some(first), Some(second)) => {
            first == second || below(first, second) || below(second, first)
        }
    }
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
    "permission denied (low ports may need privileges)".to_owned()
}

/// Windows needs no privileges for low ports but refuses binds inside its
/// reserved port ranges with access denied, which reads like a permissions
/// problem; name the ranges' command.
#[cfg(windows)]
fn denied_hint() -> String {
    format!(
        "access denied (the port may lie in a Windows reserved port range, which `{}` lists)",
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
/// `localhost` would then reach the box instead of that program. Unix refuses
/// the bind instead, so there is nothing to look up.
#[cfg(windows)]
async fn shadowing(ports: BTreeSet<u16>) -> BTreeMap<u16, u32> {
    if ports.is_empty() {
        return BTreeMap::new();
    }
    match tokio::task::spawn_blocking(devlanes_sys_windows::proc::tcp_listeners).await {
        Ok(Ok(listeners)) => wildcard_holders(&listeners, std::process::id(), &ports),
        Ok(Err(error)) => {
            tracing::debug!(%error, "cannot read the TCP listener table");
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

/// Binds a mirror listener. `SO_REUSEADDR` only on Linux, where it merely skips
/// `TIME_WAIT` on rebinds: on macOS it would let this listener shadow another
/// process bound to the wildcard address instead of reporting the conflict.
fn bind(address: IpAddr, port: u16) -> std::io::Result<TcpListener> {
    let socket = match address {
        IpAddr::V4(_) => TcpSocket::new_v4()?,
        IpAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(cfg!(target_os = "linux"))?;
    socket.bind(SocketAddr::new(address, port))?;
    socket.listen(1024)
}

/// Accepts local connections and splices each into a data stream to the box.
async fn accept_loop(listener: TcpListener, port: u16, conn: Arc<HostConn>, engine: Weak<Inner>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(splice(stream, port, conn.clone(), engine.clone()));
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
async fn splice(stream: TcpStream, port: u16, conn: Arc<HostConn>, engine: Weak<Inner>) {
    let session = match conn.session() {
        Some(session) => session,
        None => match conn.session_within(HOLD).await {
            Some(session) => session,
            // A disconnect in progress closes this listener anyway.
            None if conn.cancel.is_cancelled() => return,
            None if conn.needs_auth() => {
                let message = format!(
                    "box {} needs you to sign in again: run `devlanes remote connect {}`",
                    conn.name, conn.name
                );
                open_failed(&engine, &conn, port, "needs_auth", message);
                return;
            }
            None if conn.needs_pairing() => {
                let message = format!(
                    "box {} needs this machine to pair again: run `devlanes daemon pair` there, then `devlanes remote connect {} --code <code>`",
                    conn.name, conn.name
                );
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
            let spliced =
                devlanes_transport::splice(stream, remote, devlanes_transport::HALF_CLOSE_IDLE)
                    .await;
            if let Err(error) = spliced {
                tracing::debug!(%error, port, "mirrored connection ended");
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
                    format!("nothing on the box accepted it: {detail}"),
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
                "nothing on the box accepted it: connection refused",
            ),
            (
                rejected(RejectReason::DialFailed, "dial port 3000 timed out"),
                "dial_failed",
                "nothing on the box accepted it: timed out",
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

    /// Patterns overlap when one name matches both: equal names, a name below
    /// a wildcard's parent (not the parent itself), and wildcards one of which
    /// covers the other, in either order.
    #[test]
    fn route_patterns_overlap_like_the_proxy_matches_them() {
        for (a, b) in [
            ("web.acme.localhost", "WEB.acme.localhost."),
            ("*.acme.localhost", "web.acme.localhost"),
            ("*.acme.localhost", "a.b.acme.localhost"),
            ("*.acme.localhost", "*.acme.localhost"),
            ("*.acme.localhost", "*.pages.acme.localhost"),
        ] {
            assert!(overlap(a, b), "{a} and {b}");
            assert!(overlap(b, a), "{b} and {a}");
        }
        for (a, b) in [
            ("web.acme.localhost", "api.acme.localhost"),
            ("*.acme.localhost", "acme.localhost"),
            ("*.acme.localhost", "web.acme-2.localhost"),
            ("*.acme.localhost", "*.acme-2.localhost"),
            ("*.a.localhost", "*.ba.localhost"),
        ] {
            assert!(!overlap(a, b), "{a} and {b}");
            assert!(!overlap(b, a), "{b} and {a}");
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
