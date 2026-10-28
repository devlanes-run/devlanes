//! Box side: `remote.registry`, the registry a connected client mirrors.
//!
//! The registry comes from the same lane summaries as `status --all` and the
//! same routes as `hosts` (this machine's own lanes only): assigned lanes, their
//! services, tasks and started containers, with ports, hostnames, hidden flags
//! and states, never environments or secrets. Tasks carry their state only.
//! `infra.external` ports go only to clients listing `external_units`.
//! `remote.report` keeps what each client mirrors.

use std::{sync::Arc, time::Duration};

use devlanes_proto::{
    Event, LaneSummary, RpcError, events,
    remote::{
        REGISTRY_CHANGE_EVENT, REGISTRY_SNAPSHOT_EVENT, RegistryParams, RemoteClient, ReportParams,
    },
};
use devlanes_transport::{
    EXTERNAL_UNITS, HostHeader, HostInfo, LaneInfo, LaneKey, PortInfo, Registry, RegistryEvent,
    RegistrySnapshot, UnitInfo, UnitKey, UnitKind,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::{ClientGuard, ClientKey};
use crate::{
    Session,
    engine::{Inner, params, required},
    integrations::RouteEntry,
    model::lock,
};

/// The declared address of each `infra.external` entry, by lane reference
/// and entry name.
pub(crate) type ExternalAddresses = std::collections::BTreeMap<(String, String), std::net::IpAddr>;

/// Changes arriving within this window cause one rebuild.
const DEBOUNCE: Duration = Duration::from_millis(100);

impl Inner {
    /// `remote.registry`: sends the registry, then each change, until the
    /// tunnel leaves or the daemon shuts down. A tunnel that reads too slowly
    /// never holds up the daemon: its bus position overflows instead, and it
    /// gets a fresh snapshot.
    pub(crate) async fn remote_registry(
        self: &Arc<Self>,
        value: Value,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: RegistryParams = params(value)?;
        // Older clients reject a unit kind they do not know.
        let external = args.features.iter().any(|name| name == EXTERNAL_UNITS);
        let listed = args.client.map(|client| ClientGuard::add(self, client));
        // Checked after the listing, which a revoke ends: a revoke either
        // deleted the pairing before this read or finds the listing.
        if args.paired && !self.listed_paired(listed.as_ref()).await? {
            return Err(client_not_paired());
        }
        let revoked = listed
            .as_ref()
            .map_or_else(CancellationToken::new, |guard| guard.revoked.clone());
        // The tunnel's reports name it, so only this request's entry takes them.
        let serial = listed.as_ref().map(|guard| guard.serial);
        // Subscribed before the snapshot is built, so no change falls between them.
        let mut changes = self.bus.subscribe();
        let disconnected = session.disconnected();
        let mut current = self.registry_now(external);
        if send_snapshot(session, &current, serial).await.is_err() {
            return Ok(json!({}));
        }
        loop {
            let mut lagged = tokio::select! {
                () = disconnected.cancelled() => return Ok(json!({})),
                () = self.shutdown.cancelled() => return Ok(json!({})),
                () = revoked.cancelled() => return Err(client_revoked()),
                change = next_change(&mut changes) => match change {
                    Some(lagged) => lagged,
                    None => return Ok(json!({})),
                },
            };
            // A start or stop emits a burst of events; one rebuild covers it.
            let settle = tokio::time::sleep(DEBOUNCE);
            tokio::pin!(settle);
            loop {
                tokio::select! {
                    () = &mut settle => break,
                    () = disconnected.cancelled() => return Ok(json!({})),
                    () = self.shutdown.cancelled() => return Ok(json!({})),
                    () = revoked.cancelled() => return Err(client_revoked()),
                    next = changes.recv() => match next {
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => lagged = true,
                        Err(broadcast::error::RecvError::Closed) => return Ok(json!({})),
                    },
                }
            }
            let next = self.registry_now(external);
            let sent = if lagged {
                send_snapshot(session, &next, serial).await
            } else {
                send_changes(session, current.diff(&next)).await
            };
            if sent.is_err() {
                return Ok(json!({}));
            }
            current = next;
        }
    }

    /// Whether the client `listed` names has a pairing; an anonymous client
    /// never has one.
    async fn listed_paired(&self, listed: Option<&ClientGuard>) -> Result<bool, RpcError> {
        let Some(ClientKey::Id(id)) = listed.map(|guard| &guard.key) else {
            return Ok(false);
        };
        Ok(!self
            .store
            .remote_pairings_of(id.clone())
            .await
            .map_err(crate::registry::store_error)?
            .is_empty())
    }

    /// The registry as of now, from `status --all`'s lanes and this machine's
    /// routes; external ports only with `external`.
    pub(super) fn registry_now(&self, external: bool) -> Registry {
        let all = self.all_lanes();
        let lanes = self.summaries(&all, None);
        let routes = self.local_route_entries();
        let addresses = external.then(|| {
            all.iter()
                .flat_map(|(project, lane)| {
                    let st = lane.st();
                    let reference = st.reference(&project.name());
                    st.view
                        .iter()
                        .flat_map(|view| &view.externals)
                        .map(|external| {
                            (
                                (reference.clone(), external.name.clone()),
                                external.address.ip(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        });
        build(lanes, &routes, addresses.as_ref())
    }

    /// `remote.report`: keeps a client's mirror report, bounded, with its
    /// `status` entry, when the report names that entry's serial, so a tunnel
    /// whose feed a newer connection of the client replaced cannot overwrite
    /// it. A report for a client that is not listed (its feed ended
    /// meanwhile) is dropped. Only a report that differs from the kept one
    /// announces a change.
    pub(crate) fn remote_report(&self, value: Value) -> Result<Value, RpcError> {
        let args: ReportParams = required(value)?;
        let key = ClientKey::Id(clean(&args.id));
        let (stored, changed) = match lock(&self.remote.clients).get_mut(&key) {
            Some((serial, client)) if args.serial == Some(*serial) => {
                let report = args.report.bounded();
                let changed = client.mirror.as_ref() != Some(&report);
                client.mirror = Some(report);
                (true, changed)
            }
            _ => (false, false),
        };
        if changed {
            self.clients_changed();
        }
        Ok(json!({"stored": stored}))
    }
}

impl ClientGuard {
    /// Lists `client` in `status` until the guard drops, replacing an entry
    /// with the same id: a client that reconnects before the box noticed its
    /// old connection drop is listed once.
    fn add(engine: &Arc<Inner>, client: RemoteClient) -> Self {
        let serial = engine
            .remote
            .next_client
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let client = RemoteClient {
            id: clean(&client.id),
            name: clean(&client.name),
            ..client
        };
        let key = if client.id.is_empty() {
            ClientKey::Anonymous(serial)
        } else {
            ClientKey::Id(client.id.clone())
        };
        let revoked = CancellationToken::new();
        lock(&engine.remote.feeds).insert(serial, (client.id.clone(), revoked.clone()));
        lock(&engine.remote.clients).insert(key.clone(), (serial, client));
        engine.clients_changed();
        Self {
            engine: engine.clone(),
            key,
            serial,
            revoked,
        }
    }
}

/// `client_not_paired`: a tunnel that requires pairing asked for the feed of
/// a client without a pairing, which a revoke just deleted.
fn client_not_paired() -> RpcError {
    RpcError::new(
        "client_not_paired",
        json!({"message": "the client is not paired with this box"}),
        4,
    )
}

/// `client_revoked`: the feed ends because `devlanes daemon revoke` deleted
/// the client's pairing, which ends its session.
fn client_revoked() -> RpcError {
    RpcError::new(
        "client_revoked",
        json!({"message": "the client's pairing was revoked on the box"}),
        4,
    )
}

/// Longest client id or name the box keeps, in bytes.
const MAX_CLIENT_TEXT: usize = 256;

/// `text` without control characters, cut to [`MAX_CLIENT_TEXT`] bytes. The
/// box shows what clients send in `status`, so a client must not be able to
/// fill it or put terminal escapes in it.
pub(super) fn clean(text: &str) -> String {
    let mut cleaned = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if cleaned.len() + c.len_utf8() > MAX_CLIENT_TEXT {
            break;
        }
        cleaned.push(c);
    }
    cleaned
}

/// Waits for a bus event that may change the registry: `Some(true)` when
/// events were lost to an overflow, `None` once the bus closed.
async fn next_change(changes: &mut broadcast::Receiver<Event>) -> Option<bool> {
    loop {
        match changes.recv().await {
            Ok(event) if relevant(&event) => return Some(false),
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => return Some(true),
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

/// Log lines and remote bookkeeping never change lanes, units or ports.
fn relevant(event: &Event) -> bool {
    event.event != events::LOG && !event.event.starts_with("remote.")
}

/// Sends the whole registry, with the serial of the request's `status` entry.
async fn send_snapshot(
    session: &Session,
    registry: &Registry,
    serial: Option<u64>,
) -> Result<(), RpcError> {
    let mut data = json!({"snapshot": registry.snapshot()});
    if let Some(serial) = serial {
        data["client_serial"] = json!(serial);
    }
    session
        .event(Event {
            seq: 0,
            event: REGISTRY_SNAPSHOT_EVENT.into(),
            data,
        })
        .await
}

/// Sends each change in order.
async fn send_changes(session: &Session, changes: Vec<RegistryEvent>) -> Result<(), RpcError> {
    for change in changes {
        session
            .event(Event {
                seq: 0,
                event: REGISTRY_CHANGE_EVENT.into(),
                data: json!({"change": change}),
            })
            .await?;
    }
    Ok(())
}

/// Maps lane summaries and routes to the registry: assigned lanes, their
/// services and started containers with ports, hostnames and hidden flags,
/// their tasks with state only, and with `external` (a client listing
/// [`EXTERNAL_UNITS`]: the declared address of each entry, by lane reference
/// and name) their `infra.external` ports. Those are in state `external`
/// while a lane of the project runs a service, task, exec or container, and
/// `stopped` otherwise, so they are mirrored only meanwhile. Exec units are
/// left out.
pub(crate) fn build(
    lanes: Vec<LaneSummary>,
    routes: &[RouteEntry],
    external: Option<&ExternalAddresses>,
) -> Registry {
    let hosts_of = |lane: &str, unit: &str| -> Vec<HostInfo> {
        routes
            .iter()
            .filter(|route| route.lane == lane && route.unit == unit)
            .map(|route| HostInfo {
                host: route.host.clone(),
                port: route.port,
                fallback: route.fallback,
                host_header: if route.host_header_upstream {
                    HostHeader::Upstream
                } else {
                    HostHeader::Keep
                },
            })
            .collect()
    };
    let ports = |ports: &std::collections::BTreeMap<String, u16>| -> Vec<PortInfo> {
        ports
            .iter()
            .map(|(name, port)| PortInfo {
                name: name.clone(),
                port: *port,
                address: None,
            })
            .collect()
    };
    // Projects with a lane whose service, task or exec unit runs, or whose
    // containers are up (a lane lists them only then).
    let running: std::collections::BTreeSet<String> = lanes
        .iter()
        .filter(|lane| {
            !lane.containers.is_empty()
                || lane.units.iter().any(|unit| {
                    unit.kind != devlanes_proto::EXTERNAL_KIND
                        && devlanes_transport::serving(&unit.state)
                })
        })
        .map(|lane| lane.project.clone())
        .collect();
    let mut snapshot = RegistrySnapshot::default();
    for lane in lanes {
        // Unassigned lanes have no ports or hostnames to mirror.
        let Some(index) = lane.index else { continue };
        let key = LaneKey {
            project: lane.project.clone(),
            lane: index.to_string(),
        };
        snapshot.lanes.push(LaneInfo {
            key: key.clone(),
            index,
            domain: lane.domain.clone(),
        });
        for unit in &lane.units {
            let mut address = None;
            let (kind, state) = match (unit.kind.as_str(), external) {
                ("" | "service", _) => (UnitKind::Service, unit.state.clone()),
                ("task", _) => (UnitKind::Task, unit.state.clone()),
                (devlanes_proto::EXTERNAL_KIND, Some(addresses)) => {
                    address = addresses
                        .get(&(lane.reference.clone(), unit.name.clone()))
                        .copied();
                    let state = if running.contains(&lane.project) {
                        devlanes_proto::EXTERNAL_KIND
                    } else {
                        "stopped"
                    };
                    (UnitKind::External, state.to_owned())
                }
                _ => continue,
            };
            let endpoints = kind.has_endpoints();
            snapshot.units.push(UnitInfo {
                key: UnitKey {
                    lane: key.clone(),
                    kind,
                    name: unit.name.clone(),
                },
                hidden: unit.hidden,
                state,
                ports: if endpoints {
                    let mut listed = ports(&unit.ports);
                    for port in &mut listed {
                        port.address = address;
                    }
                    listed
                } else {
                    Vec::new()
                },
                hosts: if endpoints {
                    hosts_of(&lane.reference, &unit.name)
                } else {
                    Vec::new()
                },
            });
        }
        for container in &lane.containers {
            snapshot.units.push(UnitInfo {
                key: UnitKey {
                    lane: key.clone(),
                    kind: UnitKind::Container,
                    name: container.name.clone(),
                },
                hidden: container.hidden,
                state: "running".into(),
                ports: ports(&container.ports),
                hosts: hosts_of(&lane.reference, &container.name),
            });
        }
    }
    Registry::from_snapshot(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::RouteStatus;

    /// Client ids and names lose control characters and are cut on a
    /// character boundary.
    #[test]
    fn client_text_is_cleaned_and_capped() {
        assert_eq!(clean("lap\u{1b}[31mtop\n"), "lap[31mtop");
        let long = "é".repeat(MAX_CLIENT_TEXT);
        assert_eq!(
            clean(&long).len(),
            MAX_CLIENT_TEXT,
            "128 two-byte characters fit"
        );
        assert_eq!(
            clean(&format!("{}x", "a".repeat(MAX_CLIENT_TEXT))).len(),
            MAX_CLIENT_TEXT
        );
    }

    #[test]
    fn maps_services_tasks_containers_hosts_and_hidden_flags() {
        let lanes: Vec<LaneSummary> = serde_json::from_value(json!([{
            "reference": "app@1", "project": "app", "index": 1, "branch": "x",
            "path": "/w", "state": "running", "config_changed": false,
            "domain": "app1.localhost",
            "units": [
                {"name": "web", "lane": "app@1", "state": "ready", "pid": 1, "exit_code": null,
                 "ports": {"default": 4100}, "urls": [], "actor": {"kind": "human"},
                 "reason": null, "kind": "service"},
                {"name": "admin", "lane": "app@1", "state": "ready", "pid": 2, "exit_code": null,
                 "ports": {"default": 4101}, "urls": [], "actor": {"kind": "human"},
                 "reason": null, "kind": "service", "hidden": true},
                {"name": "worker", "lane": "app@1", "state": "stopped", "pid": null, "exit_code": null,
                 "ports": {"default": 4103}, "urls": [], "actor": {"kind": "human"},
                 "reason": null, "kind": "service"},
                {"name": "migrate", "lane": "app@1", "state": "running", "pid": 3, "exit_code": null,
                 "ports": {"default": 4104}, "urls": [], "actor": {"kind": "human"}, "reason": null,
                 "kind": "task"},
                {"name": "shell", "lane": "app@1", "state": "running", "pid": 4, "exit_code": null,
                 "ports": {}, "urls": [], "actor": {"kind": "human"}, "reason": null, "kind": "exec"},
                {"name": "pgadmin", "lane": "app@1", "state": "external", "pid": null, "exit_code": null,
                 "ports": {"default": 5050}, "urls": [], "actor": {"kind": "human"}, "reason": null,
                 "kind": "external"},
                {"name": "vault", "lane": "app@1", "state": "external", "pid": null, "exit_code": null,
                 "ports": {"default": 8200}, "urls": [], "actor": {"kind": "human"}, "reason": null,
                 "kind": "external", "hidden": true}
            ],
            "containers": [{"name": "db", "ports": {"5432": 4102}, "hidden": false}]
        }, {
            "reference": "app@feature", "project": "app", "index": null, "branch": null,
            "path": "/v", "state": "unassigned", "config_changed": false, "units": []
        }, {
            "reference": "idle@0", "project": "idle", "index": 0, "branch": "main",
            "path": "/i", "state": "idle", "config_changed": false, "units": [
                {"name": "api", "lane": "idle@0", "state": "idle", "pid": null, "exit_code": null,
                 "ports": {"default": 4200}, "urls": [], "actor": {"kind": "human"}, "reason": null,
                 "kind": "service"},
                {"name": "docs", "lane": "idle@0", "state": "external", "pid": null, "exit_code": null,
                 "ports": {"default": 8000}, "urls": [], "actor": {"kind": "human"}, "reason": null,
                 "kind": "external"}
            ]
        }, {
            "reference": "db@0", "project": "db", "index": 0, "branch": "main",
            "path": "/d", "state": "idle", "config_changed": false, "units": [
                {"name": "pgweb", "lane": "db@0", "state": "external", "pid": null, "exit_code": null,
                 "ports": {"default": 8081}, "urls": [], "actor": {"kind": "human"}, "reason": null,
                 "kind": "external"}
            ],
            "containers": [{"name": "postgres", "ports": {"5432": 4300}, "hidden": false}]
        }]))
        .expect("lane summaries");
        let route = |host: &str, unit: &str, port: u16| RouteEntry {
            host: host.into(),
            port,
            fallback: false,
            host_header_upstream: true,
            status: RouteStatus::Ready,
            start_hint: String::new(),
            project: "app".into(),
            lane: "app@1".into(),
            unit: unit.into(),
            certificate_names: Vec::new(),
            remote: false,
            box_https_port: None,
            address: None,
            upstream_box: None,
        };
        let routes = [
            route("web.app1.localhost", "web", 4100),
            route("migrate.app1.localhost", "migrate", 4104),
            route("pgadmin.app1.localhost", "pgadmin", 5050),
        ];
        let addresses = ExternalAddresses::from([(
            ("app@1".to_owned(), "pgadmin".to_owned()),
            std::net::Ipv6Addr::LOCALHOST.into(),
        )]);
        let with_external = build(lanes.clone(), &routes, Some(&addresses));
        let external: Vec<_> = with_external
            .units()
            .filter(|u| u.key.kind == UnitKind::External)
            .collect();
        assert_eq!(external.len(), 4, "{external:?}");
        let pgweb = external
            .iter()
            .find(|u| u.key.name == "pgweb")
            .expect("pgweb is listed");
        assert_eq!(pgweb.state, "external", "its project's containers are up");
        let pgadmin = external
            .iter()
            .find(|u| u.key.name == "pgadmin")
            .expect("pgadmin is listed");
        assert_eq!(pgadmin.state, "external", "its project runs a lane");
        assert_eq!(pgadmin.hosts[0].host, "pgadmin.app1.localhost");
        assert_eq!(
            pgadmin.ports[0].address,
            Some(std::net::Ipv6Addr::LOCALHOST.into()),
            "the box dials the declared address"
        );
        let docs = external
            .iter()
            .find(|u| u.key.name == "docs")
            .expect("docs is listed");
        assert_eq!(docs.state, "stopped", "its project runs nothing");
        let mirrored = with_external.mirrored_ports();
        assert!(mirrored.contains_key(&5050));
        assert!(
            !mirrored.contains_key(&8200) && !mirrored.contains_key(&8000),
            "hidden vault and idle docs are not mirrored: {mirrored:?}"
        );
        assert!(with_external.advertised_ports().contains(&8000));
        let lanes: Vec<LaneSummary> = lanes.into_iter().filter(|l| l.project == "app").collect();
        let registry = build(lanes, &routes, None);
        assert_eq!(registry.lanes().count(), 1);
        let units: Vec<_> = registry.units().collect();
        assert_eq!(
            units.len(),
            5,
            "exec skipped, external left out without the feature: {units:?}"
        );
        let web = units
            .iter()
            .find(|u| u.key.name == "web")
            .expect("web is listed");
        assert_eq!(web.hosts.len(), 1);
        assert_eq!(web.hosts[0].host_header, HostHeader::Upstream);
        let migrate = units
            .iter()
            .find(|u| u.key.name == "migrate")
            .expect("the task is listed");
        assert_eq!(migrate.key.kind, UnitKind::Task);
        assert_eq!(migrate.state, "running");
        assert!(
            migrate.ports.is_empty() && migrate.hosts.is_empty(),
            "tasks carry their state only: {migrate:?}"
        );
        let mirrored: Vec<u16> = registry.mirrored_ports().keys().copied().collect();
        assert_eq!(
            mirrored,
            [4100, 4102],
            "hidden admin, stopped worker and the task are not mirrored"
        );
        let advertised: Vec<u16> = registry.advertised_ports().into_iter().collect();
        assert_eq!(
            advertised,
            [4100, 4102, 4103],
            "the idle worker's port is kept free"
        );
    }
}
