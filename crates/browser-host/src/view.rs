//! The view the popup shows, composed from three daemon replies.
//!
//! The view is built from typed fields only, so environments, logs and other
//! daemon data never reach the extension.

use std::collections::{BTreeMap, BTreeSet};

use devlanes_proto::{LaneSummary, Page};
use serde::{Deserialize, Serialize};

/// What the popup shows: the boxes and every project's lanes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub boxes: Vec<BoxEntry>,
    pub projects: Vec<Project>,
}

/// A box this machine knows, with its link state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxEntry {
    pub name: String,
    pub state: String,
}

/// A project and its lanes, by index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub lanes: Vec<Lane>,
}

/// One lane reference, with every machine that has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    #[serde(rename = "ref")]
    pub reference: String,
    pub index: u32,
    pub name: String,
    pub domain: Option<String>,
    /// `Some(None)` for this machine, `Some(Some(box))` for a box and `None`,
    /// left out of the JSON, when nothing serves the lane.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "present_or_null"
    )]
    pub served_by: Option<Option<String>>,
    pub sources: Vec<Source>,
    pub routes: Vec<Route>,
}

/// A machine that has the lane: this machine when `host` is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    #[serde(rename = "box")]
    pub host: Option<String>,
    pub state: String,
    pub branch: Option<String>,
    pub serving: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Skipped>,
}

/// Why the daemon doesn't mirror a box's lane, and whether a plain `use` works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub cause: String,
    pub can_use: bool,
}

/// A hostname this machine routes for the lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub host: String,
    /// `None` for a wildcard host such as `*.app.acme.localhost`, which has
    /// no single address to open.
    pub url: Option<String>,
    pub service: String,
    pub state: String,
}

/// `served_by`'s three cases: absent, `null` and a box name.
mod present_or_null {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Writes the inner option, so `Some(None)` becomes `null`.
    pub fn serialize<S: Serializer>(
        value: &Option<Option<String>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .as_ref()
            .and_then(Option::as_ref)
            .serialize(serializer)
    }

    /// Reads a present field, `null` included, as `Some`.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<String>>, D::Error> {
        Option::<String>::deserialize(deserializer).map(Some)
    }
}

/// The `hosts {all: true}` reply, with the fields the view uses.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Hosts {
    #[serde(default)]
    pub items: Vec<HostRoute>,
}

/// One route of `hosts`. `lane` is the lane reference, a box lane's for a
/// mirrored route.
#[derive(Debug, Clone, Deserialize)]
pub struct HostRoute {
    pub host: String,
    /// `None` for a wildcard host, which has no single address.
    #[serde(default)]
    pub url: Option<String>,
    pub lane: String,
    pub unit: String,
    pub state: String,
}

/// The `host.list` reply, with the fields the view uses.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HostList {
    #[serde(default)]
    pub items: Vec<HostItem>,
    #[serde(default)]
    pub yielded_local_lanes: Vec<Yielded>,
}

/// One box of `host.list`.
#[derive(Debug, Clone, Deserialize)]
pub struct HostItem {
    pub host: String,
    pub state: String,
    /// Absent while the link is down.
    #[serde(default)]
    pub registry: Option<Registry>,
    #[serde(default)]
    pub skipped_lanes: Vec<SkippedLane>,
}

/// A box's registry: its lanes and units.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub lanes: Vec<RegistryLane>,
    #[serde(default)]
    pub units: Vec<RegistryUnit>,
}

/// A box lane. `key.lane` is its index as text.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryLane {
    pub key: LaneKey,
    pub index: u32,
    #[serde(default)]
    pub domain: Option<String>,
}

/// Names a box lane.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub struct LaneKey {
    pub project: String,
    pub lane: String,
}

impl LaneKey {
    /// `project@lane`, the reference the daemon gives a box lane.
    fn reference(&self) -> String {
        format!("{}@{}", self.project, self.lane)
    }
}

/// A box unit, with the lane it belongs to and its state.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryUnit {
    pub key: UnitKey,
    pub state: String,
}

/// Names a box unit.
#[derive(Debug, Clone, Deserialize)]
pub struct UnitKey {
    pub lane: LaneKey,
}

/// A box lane this machine doesn't mirror.
#[derive(Debug, Clone, Deserialize)]
pub struct SkippedLane {
    /// The box lane's reference.
    pub lane: String,
    pub cause: String,
    pub can_use: bool,
}

/// One of this machine's lanes whose hostnames a chosen box lane took.
#[derive(Debug, Clone, Deserialize)]
pub struct Yielded {
    pub lane: String,
}

/// The daemon's name for a lane without a name of its own: `main` for lane 0,
/// else `lane<N>`, as `devlanes_resolve::lane_name` has it.
fn lane_name(index: u32) -> String {
    if index == 0 {
        "main".into()
    } else {
        format!("lane{index}")
    }
}

/// A box source's state from its units' states: `running` when one runs or
/// is ready, `starting` when one starts, `failed` when one failed and none
/// runs, and `idle` otherwise.
fn box_state<'a>(states: impl Iterator<Item = &'a str>) -> &'static str {
    let mut starting = false;
    let mut failed = false;
    for state in states {
        match state {
            // An orphan is a process a previous daemon left running, and the
            // box counts its lane as running.
            "running" | "ready" | "orphaned" => return "running",
            // A queued unit waits for a start slot, and the box counts its
            // lane as starting.
            "starting" | "queued" => starting = true,
            "failed" => failed = true,
            _ => {}
        }
    }
    if starting {
        "starting"
    } else if failed {
        "failed"
    } else {
        "idle"
    }
}

/// One lane while it is built: the local summary and each box's lane.
#[derive(Default)]
struct Building<'a> {
    local: Option<&'a LaneSummary>,
    boxes: Vec<(&'a HostItem, &'a RegistryLane)>,
}

/// Composes the view from `status {all: true}`, `hosts {all: true}` and
/// `host.list`.
pub fn compose(status: &Page<LaneSummary>, hosts: &Hosts, host_list: &HostList) -> View {
    let boxes = host_list
        .items
        .iter()
        .map(|item| BoxEntry {
            name: item.host.clone(),
            state: item.state.clone(),
        })
        .collect();
    // By project, then index, so the popup lists lanes in order.
    let mut lanes: BTreeMap<(String, u32), BTreeMap<String, Building>> = BTreeMap::new();
    for summary in &status.items {
        // A lane without an index has no ports or hostnames, and a reference
        // that no box or route can name.
        let Some(index) = summary.index else { continue };
        lanes
            .entry((summary.project.clone(), index))
            .or_default()
            .entry(summary.reference.clone())
            .or_default()
            .local = Some(summary);
    }
    for item in &host_list.items {
        // A box whose link is down has no registry, so it adds no sources.
        let Some(registry) = &item.registry else {
            continue;
        };
        for lane in &registry.lanes {
            lanes
                .entry((lane.key.project.clone(), lane.index))
                .or_default()
                .entry(lane.key.reference())
                .or_default()
                .boxes
                .push((item, lane));
        }
    }
    let yielded: BTreeSet<&str> = host_list
        .yielded_local_lanes
        .iter()
        .map(|yielded| yielded.lane.as_str())
        .collect();
    let mut projects: BTreeMap<String, Vec<Lane>> = BTreeMap::new();
    for ((project, index), by_reference) in lanes {
        for (reference, building) in by_reference {
            let lane = compose_lane(&reference, index, &building, &yielded, hosts);
            projects.entry(project.clone()).or_default().push(lane);
        }
    }
    View {
        boxes,
        projects: projects
            .into_iter()
            .map(|(name, lanes)| Project { name, lanes })
            .collect(),
    }
}

/// One lane entry: its sources with the one that serves it, and its routes.
fn compose_lane(
    reference: &str,
    index: u32,
    building: &Building,
    yielded: &BTreeSet<&str>,
    hosts: &Hosts,
) -> Lane {
    let mut sources = Vec::new();
    let mut served_by = None;
    for (item, lane) in &building.boxes {
        let skipped = item
            .skipped_lanes
            .iter()
            .find(|skipped| skipped.lane == reference)
            .map(|skipped| Skipped {
                cause: skipped.cause.clone(),
                can_use: skipped.can_use,
            });
        // The daemon's assignment skips a clashing lane on every box but one,
        // so a second box that serves would be a daemon bug. Only the first
        // one counts, since the popup takes one serving source at most.
        let serving = skipped.is_none() && served_by.is_none();
        if serving {
            served_by = Some(Some(item.host.clone()));
        }
        let units = item.registry.iter().flat_map(|registry| &registry.units);
        let state = box_state(
            units
                .filter(|unit| unit.key.lane == lane.key)
                .map(|unit| unit.state.as_str()),
        );
        sources.push(Source {
            host: Some(item.host.clone()),
            state: state.into(),
            branch: None,
            serving,
            skipped,
        });
    }
    if let Some(local) = building.local {
        let serving = served_by.is_none() && !yielded.contains(reference);
        if serving {
            served_by = Some(None);
        }
        sources.insert(
            0,
            Source {
                host: None,
                state: local.state.clone(),
                branch: local.branch.clone(),
                serving,
                skipped: None,
            },
        );
    }
    let name = building
        .local
        .and_then(|local| local.name.clone())
        .unwrap_or_else(|| lane_name(index));
    let domain = building
        .local
        .and_then(|local| local.domain.clone())
        .or_else(|| {
            building
                .boxes
                .iter()
                .find_map(|(_, lane)| lane.domain.clone())
        });
    let routes = hosts
        .items
        .iter()
        .filter(|route| route.lane == reference)
        .map(|route| Route {
            host: route.host.clone(),
            url: route.url.clone(),
            service: route.unit.clone(),
            state: route.state.clone(),
        })
        .collect();
    Lane {
        reference: reference.into(),
        index,
        name,
        domain,
        served_by,
        sources,
        routes,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// A daemon input file of `tests/daemon/`: the three replies the host reads.
    #[derive(Deserialize)]
    struct Replies {
        status: Page<LaneSummary>,
        hosts: Hosts,
        host_list: HostList,
    }

    /// The view composed from `tests/daemon/<case>.json`, and the canonical
    /// fixture `tests/fixtures/view-<case>.json`, both as JSON.
    fn case(replies: &str, fixture: &str) -> (Value, Value) {
        let replies: Replies = serde_json::from_str(replies).expect("daemon replies parse");
        let view = compose(&replies.status, &replies.hosts, &replies.host_list);
        let composed = serde_json::to_value(view).expect("the view encodes");
        let fixture = serde_json::from_str(fixture).expect("the fixture parses");
        (composed, fixture)
    }

    #[test]
    fn local_lanes_without_boxes() {
        let (composed, fixture) = case(
            include_str!("../tests/daemon/no-boxes.json"),
            include_str!("../tests/fixtures/view-no-boxes.json"),
        );
        assert_eq!(composed, fixture);
    }

    #[test]
    fn local_lanes_with_a_box_whose_link_is_down() {
        let (composed, fixture) = case(
            include_str!("../tests/daemon/local.json"),
            include_str!("../tests/fixtures/view-local.json"),
        );
        assert_eq!(composed, fixture);
    }

    #[test]
    fn a_box_serves_a_lane_this_machine_yielded() {
        let (composed, fixture) = case(
            include_str!("../tests/daemon/box-mirrored.json"),
            include_str!("../tests/fixtures/view-box-mirrored.json"),
        );
        assert_eq!(composed, fixture);
    }

    #[test]
    fn a_running_local_lane_keeps_the_box_lane_skipped() {
        let (composed, fixture) = case(
            include_str!("../tests/daemon/box-skipped.json"),
            include_str!("../tests/fixtures/view-box-skipped.json"),
        );
        assert_eq!(composed, fixture);
    }

    #[test]
    fn two_boxes_with_one_chosen() {
        let (composed, fixture) = case(
            include_str!("../tests/daemon/two-boxes.json"),
            include_str!("../tests/fixtures/view-two-boxes.json"),
        );
        assert_eq!(composed, fixture);
    }

    #[test]
    fn box_states_come_from_their_units() {
        assert_eq!(box_state(["idle", "ready"].into_iter()), "running");
        assert_eq!(box_state(["failed", "running"].into_iter()), "running");
        assert_eq!(box_state(["failed", "starting"].into_iter()), "starting");
        assert_eq!(box_state(["queued"].into_iter()), "starting");
        assert_eq!(box_state(["stopped", "failed"].into_iter()), "failed");
        assert_eq!(box_state(["exited", "external"].into_iter()), "idle");
        assert_eq!(box_state([].into_iter()), "idle");
    }

    #[test]
    fn lane_names_follow_the_daemon() {
        assert_eq!(lane_name(0), "main");
        assert_eq!(lane_name(3), "lane3");
    }
}
