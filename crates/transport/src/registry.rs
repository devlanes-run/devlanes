//! Registry types mirrored from the box, and the authorization built on them.
//!
//! These are the transport's own serde types so the protocol does not depend on
//! the daemon's internal model; the daemon maps its state into them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::server::ClientInfo;
use crate::transport::BoxError;
use crate::wire::{DialTarget, RejectReason};

/// Identifies a lane on the box.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LaneKey {
    /// Project name as the box reports it.
    pub project: String,
    /// Lane name within the project, such as `main` or `feature-x`.
    pub lane: String,
}

/// A lane and the attributes clients need to render and route it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneInfo {
    /// Lane identity.
    pub key: LaneKey,
    /// Persisted lane index; 0 is the primary worktree.
    pub index: u32,
    /// Hostname suffix chosen by the box, such as `app2.localhost`.
    pub domain: Option<String>,
}

/// Whether a unit is a managed process, a container, a task run or an
/// external port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    /// A process the box daemon supervises.
    Service,
    /// A container owned by the box's infra.
    Container,
    /// A task run, listed for its state only: never mirrored or dialable.
    Task,
    /// A port the box's config declares in `infra.external`, served by
    /// something the box daemon does not run; sent only to clients with
    /// [`crate::EXTERNAL_UNITS`].
    External,
}

impl UnitKind {
    /// Whether units of this kind serve ports a client may mirror and dial.
    pub fn has_endpoints(self) -> bool {
        !matches!(self, Self::Task)
    }
}

/// Identifies a unit within a lane.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UnitKey {
    /// Owning lane.
    pub lane: LaneKey,
    /// Service or container.
    pub kind: UnitKind,
    /// Unit name from config.
    pub name: String,
}

/// An allocated port of a unit, mirrored under the same number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    /// Port name, `default` for `self.port`.
    pub name: String,
    /// Loopback port on the box.
    pub port: u16,
    /// The one loopback address the box dials for this port. Only units of
    /// kind [`UnitKind::External`] set it, so a client without
    /// [`crate::EXTERNAL_UNITS`] never sees it; absent, the box tries
    /// `127.0.0.1`, then `::1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<std::net::IpAddr>,
}

/// How the client proxy sets `Host` when forwarding to a unit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostHeader {
    /// Keep the browser's `Host` header.
    #[default]
    Keep,
    /// Send `Host: 127.0.0.1:<port>`.
    Upstream,
}

/// An HTTP hostname routed to one of a unit's ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// Exact name or `*.` wildcard pattern.
    pub host: String,
    /// Box port the hostname routes to.
    pub port: u16,
    /// Only used when no other host matches.
    #[serde(default)]
    pub fallback: bool,
    /// Host header policy for the upstream request.
    #[serde(default)]
    pub host_header: HostHeader,
}

/// A service, container or task and its network endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitInfo {
    /// Unit identity.
    pub key: UnitKey,
    /// Hidden units are never mirrored or dialable, and clients never receive
    /// them: only the box's own registry lists them, for [`Registry::authorize`].
    pub hidden: bool,
    /// Opaque lifecycle state for display, such as `running`.
    pub state: String,
    /// Allocated ports.
    pub ports: Vec<PortInfo>,
    /// Routed hostnames.
    pub hosts: Vec<HostInfo>,
}

impl UnitInfo {
    /// Whether a client may mirror and dial this unit's ports: it is not hidden
    /// and its kind has endpoints. Its state decides the rest ([`serving`]).
    pub fn mirrorable(&self) -> bool {
        !self.hidden && self.key.kind.has_endpoints()
    }

    /// Whether `port` is one of this unit's ports or a port its hostnames route to.
    pub fn advertises(&self, port: u16) -> bool {
        self.ports.iter().any(|p| p.port == port) || self.hosts.iter().any(|h| h.port == port)
    }
}

/// The full registry at one point in time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// All lanes.
    pub lanes: Vec<LaneInfo>,
    /// All units: hidden ones included on the box, none in what a client
    /// receives ([`RegistrySnapshot::without_hidden`]).
    pub units: Vec<UnitInfo>,
}

impl RegistrySnapshot {
    /// The snapshot a client gets: hidden units left out, so their names,
    /// ports and hostnames stay on the box. The box keeps them for
    /// [`Registry::authorize`], where a hidden unit's port stays refused.
    pub fn without_hidden(mut self) -> Self {
        self.units.retain(|unit| !unit.hidden);
        self
    }
}

/// An incremental registry change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RegistryEvent {
    /// A lane appeared or changed.
    LaneUpserted { lane: LaneInfo },
    /// A lane and all of its units went away.
    LaneRemoved { lane: LaneKey },
    /// A unit appeared or changed.
    UnitUpserted { unit: UnitInfo },
    /// A unit went away.
    UnitRemoved { unit: UnitKey },
}

impl RegistryEvent {
    /// The change a client gets, matching [`RegistrySnapshot::without_hidden`],
    /// decided from `before`, the box's own registry before this change. A
    /// unit the client was shown that becomes hidden or goes away is removed
    /// there; any other change to a hidden unit, or to one the client never
    /// had, sends nothing, so no frame names a unit the client must not see.
    pub fn without_hidden(self, before: &Registry) -> Option<Self> {
        let shown = |key: &UnitKey| before.unit(key).is_some_and(|unit| !unit.hidden);
        match self {
            Self::UnitUpserted { unit } if unit.hidden => {
                shown(&unit.key).then_some(Self::UnitRemoved { unit: unit.key })
            }
            Self::UnitRemoved { unit } => shown(&unit).then_some(Self::UnitRemoved { unit }),
            event => Some(event),
        }
    }
}

/// Registry state built from a snapshot plus applied events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    lanes: BTreeMap<LaneKey, LaneInfo>,
    units: BTreeMap<UnitKey, UnitInfo>,
}

impl Registry {
    /// Builds state from a snapshot; later duplicates of a key win.
    pub fn from_snapshot(snapshot: RegistrySnapshot) -> Self {
        let mut registry = Self::default();
        registry.replace(snapshot);
        registry
    }

    /// Discards current state in favor of `snapshot`.
    pub fn replace(&mut self, snapshot: RegistrySnapshot) {
        self.lanes = snapshot
            .lanes
            .into_iter()
            .map(|lane| (lane.key.clone(), lane))
            .collect();
        self.units = snapshot
            .units
            .into_iter()
            .map(|unit| (unit.key.clone(), unit))
            .collect();
    }

    /// Returns the state as a snapshot.
    pub fn snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            lanes: self.lanes.values().cloned().collect(),
            units: self.units.values().cloned().collect(),
        }
    }

    /// Applies one change. Events are idempotent, so replays are harmless.
    pub fn apply(&mut self, event: RegistryEvent) {
        match event {
            RegistryEvent::LaneUpserted { lane } => {
                self.lanes.insert(lane.key.clone(), lane);
            }
            RegistryEvent::LaneRemoved { lane } => {
                self.lanes.remove(&lane);
                self.units.retain(|key, _| key.lane != lane);
            }
            RegistryEvent::UnitUpserted { unit } => {
                self.units.insert(unit.key.clone(), unit);
            }
            RegistryEvent::UnitRemoved { unit } => {
                self.units.remove(&unit);
            }
        }
    }

    /// Events that turn `self` into `next` when applied in order.
    ///
    /// Box daemons that only see full snapshots use this to publish incremental
    /// changes; an empty result means nothing changed.
    pub fn diff(&self, next: &Registry) -> Vec<RegistryEvent> {
        let mut events = Vec::new();
        let removed_lanes: Vec<&LaneKey> = self
            .lanes
            .keys()
            .filter(|key| !next.lanes.contains_key(*key))
            .collect();
        for lane in &removed_lanes {
            events.push(RegistryEvent::LaneRemoved {
                lane: (*lane).clone(),
            });
        }
        for key in self.units.keys() {
            // Units of a removed lane already went with `LaneRemoved`.
            if !next.units.contains_key(key) && !removed_lanes.contains(&&key.lane) {
                events.push(RegistryEvent::UnitRemoved { unit: key.clone() });
            }
        }
        for (key, lane) in &next.lanes {
            if self.lanes.get(key) != Some(lane) {
                events.push(RegistryEvent::LaneUpserted { lane: lane.clone() });
            }
        }
        for (key, unit) in &next.units {
            if self.units.get(key) != Some(unit) || removed_lanes.contains(&&key.lane) {
                events.push(RegistryEvent::UnitUpserted { unit: unit.clone() });
            }
        }
        events
    }

    /// Iterates lanes in key order.
    pub fn lanes(&self) -> impl Iterator<Item = &LaneInfo> {
        self.lanes.values()
    }

    /// Iterates units in key order, hidden included.
    pub fn units(&self) -> impl Iterator<Item = &UnitInfo> {
        self.units.values()
    }

    /// Looks up one unit.
    pub fn unit(&self, key: &UnitKey) -> Option<&UnitInfo> {
        self.units.get(key)
    }

    /// Ports a client should mirror: every port of every [`UnitInfo::mirrorable`]
    /// unit that is [`serving`]. An idle unit's port stays free on the client
    /// until it starts.
    pub fn mirrored_ports(&self) -> BTreeMap<u16, &UnitKey> {
        self.units
            .values()
            .filter(|unit| unit.mirrorable() && serving(&unit.state))
            .flat_map(|unit| unit.ports.iter().map(move |port| (port.port, &unit.key)))
            .collect()
    }

    /// Every port of every [`UnitInfo::mirrorable`] unit, idle ones included: the
    /// ports a client keeps out of its own allocations so the box's units can be
    /// mirrored once they start.
    pub fn advertised_ports(&self) -> BTreeSet<u16> {
        self.units
            .values()
            .filter(|unit| unit.mirrorable())
            .flat_map(|unit| unit.ports.iter().map(|port| port.port))
            .collect()
    }

    /// Decides whether the box may dial `target`.
    ///
    /// Only ports advertised by a non-hidden service, container or external
    /// port that is serving pass, so a connected client cannot use the box as
    /// a pivot to arbitrary local services, nor reach whatever took over the
    /// port of a stopped unit. Tasks never advertise ports. A port any hidden
    /// unit advertises stays refused whatever else lists it, so no other unit
    /// on the same port can open a hidden unit's.
    pub fn authorize(&self, target: &DialTarget) -> Result<AuthorizedTarget, RejectReason> {
        let DialTarget::Port { port } = *target;
        let advertising: Vec<&UnitInfo> = self
            .units
            .values()
            .filter(|unit| unit.key.kind.has_endpoints() && unit.advertises(port))
            .collect();
        if advertising.iter().any(|unit| unit.hidden) {
            return Err(RejectReason::Hidden);
        }
        let mut refusal = RejectReason::NotRegistered;
        for unit in advertising {
            if !serving(&unit.state) {
                if refusal == RejectReason::NotRegistered {
                    refusal = RejectReason::NotRunning;
                }
                continue;
            }
            return Ok(AuthorizedTarget {
                port,
                address: unit
                    .ports
                    .iter()
                    .find(|p| p.port == port)
                    .and_then(|p| p.address),
                unit: unit.key.clone(),
            });
        }
        Err(refusal)
    }
}

/// Unit states whose ports may be dialed: services that started or are starting
/// (their port may already answer), running containers, services a previous
/// box daemon left running (`orphaned`), which the box's own proxy routes too,
/// and external ports while their project runs a lane (`external`).
pub fn serving(state: &str) -> bool {
    matches!(
        state,
        "ready" | "starting" | "running" | "orphaned" | "external"
    )
}

/// A target that passed [`Registry::authorize`]; only this crate constructs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedTarget {
    port: u16,
    address: Option<std::net::IpAddr>,
    unit: UnitKey,
}

impl AuthorizedTarget {
    /// Loopback port to dial on the box.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The one loopback address to dial, when the unit's port names it
    /// ([`PortInfo::address`]).
    pub fn address(&self) -> Option<std::net::IpAddr> {
        self.address
    }

    /// Unit that advertises the port.
    pub fn unit(&self) -> &UnitKey {
        &self.unit
    }
}

/// A registry change delivered to `serve`.
#[derive(Debug, Clone)]
pub enum RegistryUpdate {
    /// Apply one change.
    Event(RegistryEvent),
    /// Replace everything, for example after the source dropped events.
    Resync(RegistrySnapshot),
}

/// Initial state plus the ordered changes after it.
pub struct RegistryFeed {
    /// State at subscription time.
    pub snapshot: RegistrySnapshot,
    /// Changes after `snapshot`. An error or the end of the stream ends the
    /// session: a client must never keep mirroring state the box stopped
    /// reporting.
    pub updates: BoxStream<'static, Result<RegistryUpdate, BoxError>>,
}

/// Box-side supplier of registry state, implemented by the box daemon.
#[async_trait]
pub trait RegistrySource: Send + Sync {
    /// Returns a consistent snapshot and every change after it, for the client
    /// described by `client`.
    async fn subscribe(&self, client: &ClientInfo) -> Result<RegistryFeed, BoxError>;

    /// Takes the latest mirror report of `client`, which subscribed before
    /// and negotiated [`crate::MIRROR_REPORT`]. Reports of one session arrive
    /// in order, newer ones replacing older ones that were not taken yet. The
    /// default drops them.
    async fn report(&self, client: &ClientInfo, report: serde_json::Value) {
        let _ = (client, report);
    }
}

/// In-process [`RegistrySource`] backed by a broadcast channel.
///
/// Usable by the box daemon directly and by tests. Lagging subscribers get a
/// fresh snapshot instead of a gap.
#[derive(Clone)]
pub struct SharedRegistry {
    inner: Arc<Mutex<SharedInner>>,
}

struct SharedInner {
    registry: Registry,
    events: broadcast::Sender<RegistryUpdate>,
}

impl SharedRegistry {
    /// Starts from `snapshot`; `capacity` bounds buffered events per subscriber.
    pub fn new(snapshot: RegistrySnapshot, capacity: usize) -> Self {
        let (events, _) = broadcast::channel(capacity.max(1));
        Self {
            inner: Arc::new(Mutex::new(SharedInner {
                registry: Registry::from_snapshot(snapshot),
                events,
            })),
        }
    }

    /// Applies `event` and publishes it to subscribers.
    pub fn publish(&self, event: RegistryEvent) {
        let mut inner = self.lock();
        inner.registry.apply(event.clone());
        // No receivers is fine: nobody is connected.
        let _ = inner.events.send(RegistryUpdate::Event(event));
    }

    /// Replaces the whole registry and tells subscribers to resync.
    pub fn replace(&self, snapshot: RegistrySnapshot) {
        let mut inner = self.lock();
        inner.registry.replace(snapshot.clone());
        let _ = inner.events.send(RegistryUpdate::Resync(snapshot));
    }

    /// Returns the current state.
    pub fn current(&self) -> Registry {
        self.lock().registry.clone()
    }

    /// Locks shared state; a panicked publisher leaves valid data, so poison is ignored.
    fn lock(&self) -> std::sync::MutexGuard<'_, SharedInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes snapshot and receiver under one lock so no event falls between them.
    fn snapshot_and_receiver(&self) -> (RegistrySnapshot, broadcast::Receiver<RegistryUpdate>) {
        let inner = self.lock();
        (inner.registry.snapshot(), inner.events.subscribe())
    }
}

#[async_trait]
impl RegistrySource for SharedRegistry {
    /// Serves every client the same state.
    async fn subscribe(&self, _client: &ClientInfo) -> Result<RegistryFeed, BoxError> {
        let (snapshot, receiver) = self.snapshot_and_receiver();
        let source = self.clone();
        let updates =
            futures_util::stream::unfold((source, receiver), |(source, mut receiver)| async move {
                match receiver.recv().await {
                    Ok(update) => Some((Ok(update), (source, receiver))),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Older queued events could roll back a fresh snapshot,
                        // so the receiver is replaced together with it.
                        let (snapshot, receiver) = source.snapshot_and_receiver();
                        Some((Ok(RegistryUpdate::Resync(snapshot)), (source, receiver)))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                }
            })
            .boxed();
        Ok(RegistryFeed { snapshot, updates })
    }
}

/// Client-side registry state that outlives individual sessions.
///
/// Each session's snapshot replaces the value, so mirrors keep their view
/// (and listeners) across reconnects until the box says otherwise.
#[derive(Clone)]
pub struct ClientRegistry {
    state: Arc<watch::Sender<Registry>>,
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientRegistry {
    /// Starts empty.
    pub fn new() -> Self {
        Self {
            state: Arc::new(watch::Sender::new(Registry::default())),
        }
    }

    /// Subscribes to changes; diff consecutive values to add or remove mirrors.
    pub fn subscribe(&self) -> watch::Receiver<Registry> {
        self.state.subscribe()
    }

    /// Returns a copy of the current state.
    pub fn current(&self) -> Registry {
        self.state.borrow().clone()
    }

    /// Replaces state with a snapshot from the box.
    pub(crate) fn replace(&self, snapshot: RegistrySnapshot) {
        self.state
            .send_modify(|registry| registry.replace(snapshot));
    }

    /// Applies one change from the box.
    pub(crate) fn apply(&self, event: RegistryEvent) {
        self.state.send_modify(|registry| registry.apply(event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> LaneKey {
        LaneKey {
            project: "app".into(),
            lane: "main".into(),
        }
    }

    fn unit(name: &str, port: u16, hidden: bool) -> UnitInfo {
        UnitInfo {
            key: UnitKey {
                lane: lane(),
                kind: UnitKind::Service,
                name: name.into(),
            },
            hidden,
            state: "running".into(),
            ports: vec![PortInfo {
                name: "default".into(),
                port,
                address: None,
            }],
            hosts: Vec::new(),
        }
    }

    #[test]
    fn authorization_requires_visible_registration() {
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![unit("web", 20000, false), unit("db", 20001, true)],
        });
        let ok = registry.authorize(&DialTarget::Port { port: 20000 });
        assert_eq!(ok.map(|t| t.port()), Ok(20000));
        assert_eq!(
            registry.authorize(&DialTarget::Port { port: 20001 }),
            Err(RejectReason::Hidden)
        );
        assert_eq!(
            registry.authorize(&DialTarget::Port { port: 22 }),
            Err(RejectReason::NotRegistered)
        );
        assert_eq!(registry.mirrored_ports().len(), 1);
        let mut stopped = unit("api", 20002, false);
        stopped.state = "stopped".into();
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![stopped],
        });
        assert!(
            registry.mirrored_ports().is_empty(),
            "idle units are not mirrored"
        );
        assert_eq!(
            registry.advertised_ports().into_iter().collect::<Vec<_>>(),
            [20002]
        );
        assert_eq!(
            registry.authorize(&DialTarget::Port { port: 20002 }),
            Err(RejectReason::NotRunning)
        );
    }

    /// What a client gets leaves hidden units out: a snapshot drops them; a
    /// shown unit that becomes hidden or goes away is removed; a change to a
    /// unit that was hidden, or to one never there, sends nothing; other
    /// changes pass.
    #[test]
    fn clients_get_no_hidden_units() {
        let box_copy = snapshot_of(vec![unit("web", 20000, false), unit("db", 20001, true)]);
        let names: Vec<String> = box_copy
            .clone()
            .without_hidden()
            .units
            .into_iter()
            .map(|unit| unit.key.name)
            .collect();
        assert_eq!(names, ["web"]);
        let before = Registry::from_snapshot(box_copy);
        let upsert = |name: &str, port, hidden| RegistryEvent::UnitUpserted {
            unit: unit(name, port, hidden),
        };
        let removal = |name: &str| RegistryEvent::UnitRemoved {
            unit: unit(name, 0, false).key,
        };
        // web was shown: hiding or removing it removes it on the client.
        assert_eq!(
            upsert("web", 20000, true).without_hidden(&before),
            Some(removal("web"))
        );
        assert_eq!(removal("web").without_hidden(&before), Some(removal("web")));
        // db was hidden: no change to it, removal included, names it.
        assert_eq!(upsert("db", 20001, true).without_hidden(&before), None);
        assert_eq!(removal("db").without_hidden(&before), None);
        // A hidden unit the box never had sends nothing either.
        assert_eq!(upsert("new", 20002, true).without_hidden(&before), None);
        assert_eq!(removal("gone").without_hidden(&before), None);
        // Shown, or shown again: the upsert passes.
        let visible = upsert("web", 20000, false);
        assert_eq!(visible.clone().without_hidden(&before), Some(visible));
        let unhidden = upsert("db", 20001, false);
        assert_eq!(unhidden.clone().without_hidden(&before), Some(unhidden));
        let lane_gone = RegistryEvent::LaneRemoved { lane: lane() };
        assert_eq!(lane_gone.clone().without_hidden(&before), Some(lane_gone));
    }

    /// A snapshot of `units` without lanes.
    fn snapshot_of(units: Vec<UnitInfo>) -> RegistrySnapshot {
        RegistrySnapshot {
            lanes: Vec::new(),
            units,
        }
    }

    #[test]
    fn orphaned_units_stay_dialable() {
        let mut orphan = unit("web", 20000, false);
        orphan.state = "orphaned".into();
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![orphan],
        });
        let target = registry.authorize(&DialTarget::Port { port: 20000 });
        assert_eq!(target.map(|t| t.port()), Ok(20000));
        assert!(serving("orphaned"));
    }

    #[test]
    fn lane_removal_drops_its_units() {
        let mut registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: vec![LaneInfo {
                key: lane(),
                index: 0,
                domain: None,
            }],
            units: vec![unit("web", 20000, false)],
        });
        registry.apply(RegistryEvent::LaneRemoved { lane: lane() });
        assert_eq!(registry, Registry::default());
    }

    #[test]
    fn diff_replays_into_the_next_state() {
        let lane_info = LaneInfo {
            key: lane(),
            index: 0,
            domain: None,
        };
        let before = Registry::from_snapshot(RegistrySnapshot {
            lanes: vec![lane_info.clone()],
            units: vec![unit("web", 20000, false), unit("db", 20001, false)],
        });
        let mut moved = unit("web", 20002, false);
        moved.state = "starting".into();
        let after = Registry::from_snapshot(RegistrySnapshot {
            lanes: vec![LaneInfo {
                domain: Some("app.localhost".into()),
                ..lane_info
            }],
            units: vec![moved, unit("api", 20003, true)],
        });
        let events = before.diff(&after);
        let mut replayed = before.clone();
        for event in events {
            replayed.apply(event);
        }
        assert_eq!(replayed, after);
        assert!(after.diff(&after).is_empty());
        let mut emptied = after.clone();
        for event in after.diff(&Registry::default()) {
            emptied.apply(event);
        }
        assert_eq!(emptied, Registry::default());
    }

    #[tokio::test]
    async fn lagging_subscriber_resyncs() -> Result<(), BoxError> {
        let shared = SharedRegistry::new(RegistrySnapshot::default(), 1);
        let mut feed = shared.subscribe(&ClientInfo::default()).await?;
        shared.publish(RegistryEvent::UnitUpserted {
            unit: unit("a", 1, false),
        });
        shared.publish(RegistryEvent::UnitUpserted {
            unit: unit("b", 2, false),
        });
        match feed.updates.next().await {
            Some(Ok(RegistryUpdate::Resync(snapshot))) => assert_eq!(snapshot.units.len(), 2),
            other => panic!("expected resync, got {other:?}"),
        }
        Ok(())
    }

    /// An external port in state `external` is mirrored and dialable like a
    /// running service's, and goes by `external` on the wire; in any other
    /// state (its project runs no lane) it is only kept free.
    #[test]
    fn external_ports_are_mirrored_and_dialable() {
        let mut external = unit("pgadmin", 5050, false);
        external.key.kind = UnitKind::External;
        external.state = "external".into();
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![external.clone()],
        });
        assert!(external.mirrorable());
        assert!(registry.mirrored_ports().contains_key(&5050));
        let target = registry.authorize(&DialTarget::Port { port: 5050 });
        assert_eq!(target.map(|t| t.port()), Ok(5050));
        let encoded = serde_json::to_value(&external.key).expect("encodes");
        assert_eq!(encoded["kind"], "external");

        let mut idle = external.clone();
        idle.state = "stopped".into();
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![idle],
        });
        assert!(registry.mirrored_ports().is_empty());
        assert!(registry.advertised_ports().contains(&5050));
        assert_eq!(
            registry.authorize(&DialTarget::Port { port: 5050 }),
            Err(RejectReason::NotRunning)
        );

        // A hidden unit on the same port wins, whatever lists it too.
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![external, unit("vault", 5050, true)],
        });
        assert_eq!(
            registry.authorize(&DialTarget::Port { port: 5050 }),
            Err(RejectReason::Hidden)
        );
    }

    /// An external port's declared address is the only one dialed: a `[::1]`
    /// entry does not reach a `127.0.0.1` listener and the reverse, while a
    /// port without one falls back from `127.0.0.1` to `::1`.
    #[tokio::test]
    async fn external_ports_dial_only_their_address() {
        use crate::{Dialer, LoopbackDialer};
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        let Ok(v6) = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await else {
            eprintln!("skipping: no IPv6 loopback");
            return;
        };
        let v6_port = v6.local_addr().expect("v6 addr").port();
        let v4 = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("v4 listener");
        let v4_port = v4.local_addr().expect("v4 addr").port();
        let target = |port: u16, address: Option<IpAddr>| {
            let mut unit = unit("tool", port, false);
            unit.key.kind = UnitKind::External;
            unit.state = "external".into();
            unit.ports[0].address = address;
            Registry::from_snapshot(RegistrySnapshot {
                lanes: Vec::new(),
                units: vec![unit],
            })
            .authorize(&DialTarget::Port { port })
            .expect("authorized")
        };
        let dial = |target: AuthorizedTarget| async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                LoopbackDialer.dial(&target),
            )
            .await
            .expect("dial answers")
            .is_ok()
        };
        let (on_v4, on_v6) = (
            Some(Ipv4Addr::LOCALHOST.into()),
            Some(Ipv6Addr::LOCALHOST.into()),
        );
        // Nothing listens on the other family's loopback at these ports.
        let free = |ip: IpAddr, port| std::net::TcpListener::bind((ip, port)).is_ok();
        if free(Ipv6Addr::LOCALHOST.into(), v4_port) {
            assert!(
                !dial(target(v4_port, on_v6)).await,
                "[::1] reached 127.0.0.1"
            );
        }
        if free(Ipv4Addr::LOCALHOST.into(), v6_port) {
            assert!(
                !dial(target(v6_port, on_v4)).await,
                "127.0.0.1 reached [::1]"
            );
            assert!(
                dial(target(v6_port, None)).await,
                "the fallback reaches [::1]"
            );
        }
        assert!(dial(target(v4_port, on_v4)).await);
        assert!(dial(target(v6_port, on_v6)).await);
        drop((v4, v6));
    }

    /// Tasks are listed with their state, but even a running task with ports
    /// is never mirrored, kept free or dialable.
    #[test]
    fn tasks_are_listed_but_never_exposed() {
        let mut task = unit("migrate", 20005, false);
        task.key.kind = UnitKind::Task;
        let registry = Registry::from_snapshot(RegistrySnapshot {
            lanes: Vec::new(),
            units: vec![task.clone()],
        });
        assert_eq!(registry.units().count(), 1);
        assert_eq!(
            registry.unit(&task.key).map(|unit| unit.state.as_str()),
            Some("running")
        );
        assert!(!task.mirrorable());
        assert!(registry.mirrored_ports().is_empty());
        assert!(registry.advertised_ports().is_empty());
        assert_eq!(
            registry.authorize(&DialTarget::Port { port: 20005 }),
            Err(RejectReason::NotRegistered)
        );
        let encoded = serde_json::to_value(&task.key).expect("encodes");
        assert_eq!(encoded["kind"], "task");
    }
}
