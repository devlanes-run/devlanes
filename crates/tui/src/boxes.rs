//! Boxes this machine connects to, as `host.list` describes them, and their
//! lanes in the shape the project list draws. Box lanes are read-only here:
//! the box runs them, this machine only mirrors their ports and hostnames.
//!
//! A box lane keeps its project, so it joins the tab of this machine's lane
//! of the same project, and gets the reference `<lane> @<box>`
//! (`acme@1 @hetzner`), so it never collides with a local lane. [`BoxLane`]
//! names its box; nothing reads the box back out of the reference. Units
//! keep their box names.

use std::collections::BTreeMap;

use devlanes_proto::{ContainerSummary, LaneSummary, UnitSummary};
use serde::Deserialize;
use serde_json::Value;

/// One `host.list` item: a box with a live link, or one only remembered.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct BoxHost {
    pub host: String,
    /// `connected`, `connecting`, `reconnecting`, `needs_auth`,
    /// `needs_pairing`, `failed`, `stopped`, or `disconnected` for a box with
    /// no link in this daemon.
    pub state: String,
    pub error: Option<String>,
    /// While reconnecting: how long the link waits before the next attempt.
    pub retry_in_ms: Option<u64>,
    /// `ssh` or `command`.
    pub transport: Option<String>,
    pub last_connected_ms: Option<u64>,
    pub connected_since_ms: Option<u64>,
    pub ports: Vec<MirrorPort>,
    pub routes: Vec<MirrorRoute>,
    pub skipped_lanes: Vec<SkippedLane>,
    pub skipped_hosts: Vec<SkippedHost>,
    pub syncs: Vec<BoxSync>,
    pub warnings: Vec<String>,
    /// The box's lanes and units; absent once the link ended and from
    /// daemons that predate it.
    pub registry: Option<Registry>,
    #[serde(rename = "box")]
    pub info: Option<BoxInfo>,
    /// Name suffixes box hostnames must end in to be routed here.
    pub suffixes: Vec<String>,
    pub proxy: Option<ProxyInfo>,
    /// Lanes chosen for this box with `remote.use` (`acme@0`), mirrored or not.
    pub chosen_lanes: Vec<String>,
}

/// A box port the mirror tracks: a serving unit's port, bound here or not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MirrorPort {
    pub port: u16,
    /// `project@lane` on the box.
    pub lane: String,
    pub unit: String,
    /// `bound`, `in_use`, `local_lane` or `unbound`; absent from older daemons.
    pub state: Option<String>,
    pub addresses: Vec<String>,
    pub conflicts: Vec<Conflict>,
    /// The local relay port routed hostnames use when this port is not bound.
    pub relay: Option<u16>,
    pub last_error: Option<LastError>,
}

/// Why one address of a mirrored port could not be bound.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Conflict {
    pub message: String,
    /// What holds it: `nginx (pid 812)`, `box hetzner (acme@0:api)`.
    pub holder: Option<String>,
    /// The local lane whose port block holds the port.
    pub local: Option<String>,
}

/// The last connection to a mirrored port that did not reach the box.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct LastError {
    pub reason: String,
    pub message: String,
}

/// A box hostname routed by this machine's proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MirrorRoute {
    pub host: String,
    pub lane: String,
    pub unit: String,
    /// What to open on this machine; `None` for wildcards.
    pub url: Option<String>,
    pub state: Option<String>,
}

/// A box lane left unmirrored because its hostnames are served already.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SkippedLane {
    pub lane: String,
    pub hosts: Vec<String>,
    /// The box connected earlier that serves them; `None` when a local
    /// lane does.
    #[serde(rename = "box")]
    pub other: Option<String>,
    pub reason: String,
    /// What wins over it: `local_running`, `local_stopped`, `box_chosen` or
    /// `box_earlier`; absent from daemons without `remote.use`.
    pub cause: Option<String>,
    /// The box whose lane serves the hostnames; `None` when this machine's
    /// lane does.
    pub winner: Option<String>,
    /// The reference of the lane in use, on `winner` or here. It differs from
    /// `lane` when another project's lane clashes on a hostname.
    pub winner_lane: Option<String>,
    /// `remote.use` can make it the one in use: false while this machine's
    /// lane runs, and from daemons without `remote.use`.
    pub can_use: bool,
    /// What keeps this machine's lane running for `local_running`:
    /// `processes`, which a stop ends, or only `containers`. The daemon
    /// sends it with every `local_running` entry.
    pub holds: Option<String>,
    /// The Compose project of that lane's own containers, when they count.
    pub compose_project: Option<String>,
    /// That lane's own plain containers that count, by Docker name.
    pub containers: Vec<String>,
}

/// The commands that stop a local lane's own containers:
/// `docker compose -p acme-lane0 stop` for its Compose project and
/// `docker stop <names>` for plain containers. `None` when neither is known.
pub fn containers_command(compose_project: Option<&str>, containers: &[String]) -> Option<String> {
    let mut commands = Vec::new();
    if let Some(project) = compose_project.filter(|p| !p.is_empty()) {
        commands.push(format!(
            "docker compose -p {} stop",
            crate::text::clean(project)
        ));
    }
    if !containers.is_empty() {
        let names: Vec<String> = containers.iter().map(|c| crate::text::clean(c)).collect();
        commands.push(format!("docker stop {}", names.join(" ")));
    }
    (!commands.is_empty()).then(|| commands.join(" and "))
}

/// `acme@0's containers run on this machine. Stop them with docker compose
/// -p acme-lane0 stop, then press m.`, without the command when the daemon
/// sends none. In the TUI `m` switches once they stopped, where the CLI's
/// words end in `devlanes remote use`.
pub fn containers_run(lane: &str, command: Option<&str>) -> String {
    let lane = crate::text::clean(lane);
    match command {
        Some(command) => format!(
            "{lane}'s containers run on this machine. Stop them with {command}, then press m."
        ),
        None => format!("{lane}'s containers run on this machine. Stop them, then press m."),
    }
}

impl SkippedLane {
    /// [`containers_command`] for the local lane that keeps this one out.
    pub fn containers_command(&self) -> Option<String> {
        containers_command(self.compose_project.as_deref(), &self.containers)
    }

    /// The lane in use instead: `winner_lane`, else this lane's own
    /// reference, which the clashing lane shares unless another project's
    /// lane clashes.
    pub fn serving(&self) -> &str {
        self.winner_lane.as_deref().unwrap_or(&self.lane)
    }

    /// The box that serves its hostnames instead, `None` for this machine:
    /// `winner`, or `box` from daemons that send no `cause`.
    pub fn served_by(&self) -> Option<&str> {
        if self.cause.is_some() {
            self.winner.as_deref()
        } else {
            self.other.as_deref()
        }
    }
}

/// One of this machine's stopped lanes whose hostnames a chosen box lane
/// took: `{lane, winner}` of `host.list`'s `yielded_local_lanes`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct YieldedLane {
    pub lane: String,
    /// The box that serves the lane's hostnames now.
    pub winner: String,
}

/// A box lane as the list draws it, with the box it runs on.
#[derive(Debug, Clone, Default)]
pub struct BoxLane {
    /// The box's name.
    pub host: String,
    /// The lane's own reference on the box: `acme@1`.
    pub lane: String,
    /// The lane in the list's shape, with the reference `acme@1 @hetzner`.
    pub summary: LaneSummary,
}

/// A box hostname outside the names this machine routes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SkippedHost {
    pub host: String,
    pub lane: String,
    pub unit: String,
}

/// A folder the box syncs here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BoxSync {
    pub lane: String,
    pub unit: String,
    /// The source folder on the box.
    pub from: String,
    /// Where it lands on this machine.
    pub to: String,
    /// `comparing`, `syncing`, `watching`, `failed`, `interrupted`.
    pub state: String,
    pub written: u64,
    pub error: Option<String>,
}

/// The box's registry, as the remote protocol's `RegistrySnapshot` shapes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub lanes: Vec<RegistryLane>,
    pub units: Vec<RegistryUnit>,
}

/// `{project, lane}`: the lane is the box's lane index as text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct LaneKey {
    pub project: String,
    pub lane: String,
}

impl LaneKey {
    /// `project@lane`, the box's own reference.
    pub fn reference(&self) -> String {
        format!("{}@{}", self.project, self.lane)
    }
}

/// A lane of the box.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RegistryLane {
    pub key: LaneKey,
    pub index: u32,
    pub domain: Option<String>,
}

/// A unit's identity in the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UnitKey {
    pub lane: LaneKey,
    /// `service`, `container` or `task`.
    pub kind: String,
    pub name: String,
}

/// A service, container or task of the box.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RegistryUnit {
    pub key: UnitKey,
    pub hidden: bool,
    pub state: String,
    pub ports: Vec<RegistryPort>,
    pub hosts: Vec<RegistryHost>,
}

/// An allocated port of a box unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RegistryPort {
    pub name: String,
    pub port: u16,
}

/// A hostname of a box unit and the box port it reaches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RegistryHost {
    pub host: String,
    pub port: u16,
}

/// What the box said about itself in its handshake.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BoxInfo {
    pub version: Option<String>,
    pub https_port: Option<u16>,
    pub ports_range: Option<String>,
}

/// This machine's proxy as the box needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ProxyInfo {
    pub https_port: Option<u16>,
    /// The box's HTTPS port when it differs from this machine's.
    pub box_port: Option<u16>,
    /// The proxy serves `box_port` for this box's hostnames.
    pub open: bool,
}

/// Parses `host.list` leniently: an item this client cannot read is left out
/// instead of hiding every box.
pub fn parse(value: &Value) -> Vec<BoxHost> {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<BoxHost>(item.clone()).ok())
                .filter(|host| !host.host.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// `host.list`'s `yielded_local_lanes`, leniently like [`parse`]; empty from
/// daemons without `remote.use`.
pub fn parse_yielded(value: &Value) -> Vec<YieldedLane> {
    value
        .get("yielded_local_lanes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<YieldedLane>(item.clone()).ok())
                .filter(|yielded| !yielded.lane.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The reference a box lane goes by in the TUI: `acme@1 @hetzner`.
pub fn lane_reference(lane: &str, host: &str) -> String {
    format!("{lane} @{host}")
}

/// The box part of a reference [`lane_reference`] made, for a lane that is
/// gone and so has no [`BoxLane`] left to ask. Box names may hold `@`
/// (`user@host`); the lane part never holds a space.
pub fn host_of(reference: &str) -> Option<&str> {
    reference.split_once(" @").map(|(_, host)| host)
}

impl BoxHost {
    /// The link is up, so what the entry shows is current.
    pub fn connected(&self) -> bool {
        self.state == "connected"
    }

    /// Reached over ssh, so `ssh <box>` opens a shell there.
    pub fn over_ssh(&self) -> bool {
        self.transport.as_deref().is_none_or(|t| t == "ssh")
    }

    /// The box's devlanes version from its last handshake.
    pub fn version(&self) -> Option<String> {
        self.info.as_ref().and_then(|info| info.version.clone())
    }

    /// The box lane `lane` (`project@index`) of the registry.
    pub fn lane(&self, lane: &str) -> Option<&RegistryLane> {
        self.registry
            .as_ref()?
            .lanes
            .iter()
            .find(|l| l.key.reference() == lane)
    }

    /// Unit `name` of box lane `lane`.
    pub fn unit(&self, lane: &str, name: &str) -> Option<&RegistryUnit> {
        self.registry
            .as_ref()?
            .units
            .iter()
            .find(|u| u.key.lane.reference() == lane && u.key.name == name)
    }

    /// The mirror's entry for box port `port` of `lane:unit`.
    pub fn port(&self, lane: &str, unit: &str, port: u16) -> Option<&MirrorPort> {
        self.ports
            .iter()
            .find(|p| p.port == port && p.lane == lane && p.unit == unit)
    }

    /// The route for `host`, when this machine routes it.
    pub fn route(&self, host: &str) -> Option<&MirrorRoute> {
        self.routes
            .iter()
            .find(|route| route.host.eq_ignore_ascii_case(host))
    }

    /// Why box lane `lane` is not mirrored, when it is skipped.
    pub fn skipped(&self, lane: &str) -> Option<&SkippedLane> {
        self.skipped_lanes.iter().find(|s| s.lane == lane)
    }

    /// Whether `host` is outside the names this machine routes.
    pub fn refused(&self, host: &str) -> bool {
        self.skipped_hosts
            .iter()
            .any(|s| s.host.eq_ignore_ascii_case(host))
    }

    /// The syncs of `lane:unit`.
    pub fn syncs_of<'a>(
        &'a self,
        lane: &'a str,
        unit: &'a str,
    ) -> impl Iterator<Item = &'a BoxSync> {
        self.syncs
            .iter()
            .filter(move |s| s.lane == lane && s.unit == unit)
    }

    /// How to run `command` on the box, after `On <box>: ` in every hint:
    /// `ssh hetzner, then devlanes …` over ssh, the command alone for a box
    /// reached by a command, which has no shell to name.
    pub fn there(&self, command: &str) -> String {
        if self.over_ssh() {
            format!("ssh {}, then {command}", self.host)
        } else {
            command.to_owned()
        }
    }

    /// Whether box lane `lane` (`acme@0`) was chosen for this box.
    pub fn chosen(&self, lane: &str) -> bool {
        self.chosen_lanes.iter().any(|chosen| chosen == lane)
    }

    /// The box's lanes as the list draws them, by project and index.
    pub fn lanes(&self) -> Vec<BoxLane> {
        let Some(registry) = &self.registry else {
            return Vec::new();
        };
        let mut lanes: Vec<&RegistryLane> = registry.lanes.iter().collect();
        lanes.sort_by(|a, b| (&a.key.project, a.index).cmp(&(&b.key.project, b.index)));
        lanes
            .into_iter()
            .map(|lane| BoxLane {
                host: self.host.clone(),
                lane: lane.key.reference(),
                summary: self.lane_summary(registry, lane),
            })
            .collect()
    }

    /// One box lane: services and tasks as units, containers as infra.
    fn lane_summary(&self, registry: &Registry, lane: &RegistryLane) -> LaneSummary {
        let box_ref = lane.key.reference();
        let reference = lane_reference(&box_ref, &self.host);
        let mut units = Vec::new();
        let mut containers = Vec::new();
        for unit in registry
            .units
            .iter()
            .filter(|unit| unit.key.lane == lane.key)
        {
            let ports: BTreeMap<String, u16> = unit
                .ports
                .iter()
                .map(|port| (port.name.clone(), port.port))
                .collect();
            let urls = self.urls(&box_ref, unit);
            let color =
                devlanes_core::color::unit_color(&[], &unit.key.name, None, &BTreeMap::new());
            if unit.key.kind == "container" {
                containers.push(ContainerSummary {
                    name: unit.key.name.clone(),
                    ports,
                    hidden: unit.hidden,
                    color: Some(color.into()),
                    state: Some(unit.state.clone()),
                    urls,
                    ..ContainerSummary::default()
                });
                continue;
            }
            let kind = match unit.key.kind.as_str() {
                "task" => "task",
                devlanes_proto::EXTERNAL_KIND => devlanes_proto::EXTERNAL_KIND,
                _ => "service",
            };
            units.push(UnitSummary {
                name: unit.key.name.clone(),
                lane: reference.clone(),
                state: unit.state.clone(),
                ports,
                public_url: urls.iter().find(|url| url.starts_with("https://")).cloned(),
                urls,
                kind: kind.into(),
                role: Some(kind.into()),
                hidden: unit.hidden,
                color: Some(color.into()),
                ..UnitSummary::default()
            });
        }
        let services = || units.iter().filter(|u| u.kind == "service");
        let state = if services().any(|u| matches!(u.state.as_str(), "starting" | "queued")) {
            "starting"
        } else if services().any(|u| matches!(u.state.as_str(), "ready" | "running"))
            || !containers.is_empty()
        {
            "running"
        } else if services().any(|u| u.state == "failed") {
            "failed"
        } else {
            "idle"
        };
        LaneSummary {
            reference,
            project: lane.key.project.clone(),
            index: Some(lane.index),
            // The chips name a box lane by its number and box; the tab names the project.
            name: Some(format!("@{}", lane.index)),
            state: state.into(),
            units,
            containers,
            domain: lane.domain.clone(),
            ..LaneSummary::default()
        }
    }

    /// What opens `unit` here: its routed hostnames' URLs, then the loopback
    /// address of each port bound on this machine, IPv4 when it is bound there.
    fn urls(&self, lane: &str, unit: &RegistryUnit) -> Vec<String> {
        let mut urls: Vec<String> = unit
            .hosts
            .iter()
            .filter_map(|host| self.route(&host.host)?.url.clone())
            .collect();
        for port in &unit.ports {
            let Some(mirrored) = self
                .port(lane, &unit.key.name, port.port)
                .filter(|p| p.state.as_deref().unwrap_or("bound") == "bound")
            else {
                continue;
            };
            if mirrored.bound_v4() {
                urls.push(format!("http://127.0.0.1:{}", port.port));
            } else if mirrored.addresses.iter().any(|a| a == "::1") {
                urls.push(format!("http://[::1]:{}", port.port));
            }
        }
        urls.dedup();
        urls
    }
}

impl MirrorPort {
    /// Whether `127.0.0.1:<port>` reaches the box; the mirror may hold only
    /// `::1` when something else has the IPv4 address.
    pub fn bound_v4(&self) -> bool {
        self.addresses.iter().any(|a| a == "127.0.0.1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listed() -> Value {
        json!({"items": [
            {"host": "hetzner", "state": "connected", "transport": "ssh",
             "ports": [{"port": 20101, "lane": "acme@1", "unit": "web", "kind": "service",
                        "state": "bound", "addresses": ["127.0.0.1", "::1"], "conflicts": [],
                        "relay": null, "last_error": null}],
             "routes": [{"host": "web.acme1.localhost", "port": 20101, "lane": "acme@1",
                         "unit": "web", "url": "https://web.acme1.localhost:8443",
                         "state": "ready"}],
             "registry": {
                 "lanes": [{"key": {"project": "acme", "lane": "1"}, "index": 1,
                            "domain": "acme1.localhost"}],
                 "units": [
                    {"key": {"lane": {"project": "acme", "lane": "1"}, "kind": "service", "name": "web"},
                     "hidden": false, "state": "ready",
                     "ports": [{"name": "default", "port": 20101}],
                     "hosts": [{"host": "web.acme1.localhost", "port": 20101}]},
                    {"key": {"lane": {"project": "acme", "lane": "1"}, "kind": "task", "name": "migrate"},
                     "hidden": false, "state": "exited", "ports": [], "hosts": []},
                    {"key": {"lane": {"project": "acme", "lane": "1"}, "kind": "container", "name": "db"},
                     "hidden": false, "state": "running",
                     "ports": [{"name": "5432", "port": 20110}], "hosts": []}
                 ]}},
            {"host": "old", "state": "disconnected", "transport": "ssh"},
            {"host": "broken", "ports": "not a list"}
        ]})
    }

    /// Unreadable items are left out; the rest keep their fields.
    #[test]
    fn host_list_parses_leniently() {
        let boxes = parse(&listed());
        let names: Vec<&str> = boxes.iter().map(|b| b.host.as_str()).collect();
        assert_eq!(names, ["hetzner", "old"]);
        assert!(boxes[1].registry.is_none());
        assert_eq!(parse(&json!({})), Vec::<BoxHost>::new());
    }

    /// `remote.use`'s fields: the chosen lanes, what wins over a skipped
    /// lane, and this machine's lanes not in use. Daemons without them
    /// leave a skipped lane unusable and name the earlier box by `box`.
    #[test]
    fn choices_parse_with_what_wins_over_a_skipped_lane() {
        let listed = json!({
            "items": [{"host": "fork-b", "state": "connected", "chosen_lanes": ["acme@1"],
                "skipped_lanes": [
                    {"lane": "acme@0", "hosts": [], "box": "fork-a", "reason": "r",
                     "cause": "box_chosen", "winner": "fork-a", "winner_lane": "shop@0",
                     "can_use": true},
                    {"lane": "acme@2", "hosts": [], "box": null, "reason": "r",
                     "cause": "local_running", "winner": null, "can_use": false},
                    {"lane": "acme@3", "hosts": [], "box": "old", "reason": "r"},
                ]}],
            "yielded_local_lanes": [{"lane": "acme@0", "winner": "fork-a"}, {"winner": "x"}],
        });
        let host = &parse(&listed)[0];
        assert!(host.chosen("acme@1") && !host.chosen("acme@0"));
        let chosen = host
            .skipped("acme@0")
            .map(|s| (s.served_by(), s.serving(), s.can_use));
        assert_eq!(chosen, Some((Some("fork-a"), "shop@0", true)));
        // Without `winner_lane`, the lane's own reference.
        assert_eq!(
            host.skipped("acme@2").map(SkippedLane::serving),
            Some("acme@2")
        );
        let running = host.skipped("acme@2").map(|s| (s.served_by(), s.can_use));
        assert_eq!(running, Some((None, false)));
        let older = host.skipped("acme@3").map(|s| (s.served_by(), s.can_use));
        assert_eq!(older, Some((Some("old"), false)));
        assert_eq!(
            parse_yielded(&listed),
            [YieldedLane {
                lane: "acme@0".into(),
                winner: "fork-a".into()
            }]
        );
        assert!(parse_yielded(&json!({"items": []})).is_empty());
        assert_eq!(host_of("acme@1 @me@gpu"), Some("me@gpu"));
        assert_eq!(host_of("acme@1"), None);
    }

    /// A box lane is labelled with the box, lists services and tasks as
    /// units and containers as infra, and opens through the routed URL first.
    #[test]
    fn box_lanes_carry_the_box_and_the_urls_to_open_here() {
        let boxes = parse(&listed());
        let lanes = boxes[0].lanes();
        assert_eq!(lanes.len(), 1);
        assert_eq!(
            (lanes[0].host.as_str(), lanes[0].lane.as_str()),
            ("hetzner", "acme@1")
        );
        let lane = &lanes[0].summary;
        assert_eq!(lane.reference, "acme@1 @hetzner");
        // The local project's own name, so the lane joins its tab.
        assert_eq!(lane.project, "acme");
        assert_eq!(lane.name.as_deref(), Some("@1"));
        assert_eq!(lane.state, "running");
        let web = &lane.units[0];
        assert_eq!(
            web.urls,
            ["https://web.acme1.localhost:8443", "http://127.0.0.1:20101"]
        );
        assert_eq!(
            web.public_url.as_deref(),
            Some("https://web.acme1.localhost:8443")
        );
        assert_eq!(lane.units[1].kind, "task");
        // Held on 127.0.0.1 by something else, the port opens on ::1 only.
        let mut v6 = listed();
        v6["items"][0]["ports"][0]["addresses"] = json!(["::1"]);
        let urls = &parse(&v6)[0].lanes()[0].summary.units[0].urls;
        assert_eq!(urls[1], "http://[::1]:20101");
        assert_eq!(lane.containers[0].name, "db");
        assert_eq!(
            boxes[0].there("devlanes stop acme@1:web"),
            "ssh hetzner, then devlanes stop acme@1:web"
        );
    }
}
