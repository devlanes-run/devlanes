//! Where a GUI service's host can be reached from this machine, read from
//! what `status` and `remote ls` answer. The `devlanes gui` commands, `gui
//! open` and the daemon's auto-open all find services this way.
//!
//! A service of this machine's lanes is a unit of `status` with `gui` set,
//! whose host listens on its `devlanes_gui` port. A box's lane is not in this
//! machine's `status`, so it comes from `remote ls`: the box's registry names
//! the unit's ports, and the mirror view says whether its `devlanes_gui` port
//! is bound here, on loopback under the same number.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use devlanes_proto::{GUI_PORT, LaneSummary};
use serde_json::{Value, json};

/// One GUI service and where its host listens.
#[derive(Debug, Clone)]
pub struct Service {
    /// The unit's name, such as `desktop` or `desktop.2`.
    pub unit: String,
    /// The unit's state.
    pub state: String,
    /// `running`, `stopped`, `exited` or `none`, as `status` says; for a box
    /// unit `running` while it serves, else `stopped`.
    pub host: String,
    /// Where to connect while the host runs.
    pub address: Option<SocketAddr>,
    /// Whether the display is this machine's real desktop, for a local
    /// service whose host runs; `None` for a box's, which this machine cannot
    /// know without connecting.
    pub real_desktop: Option<bool>,
    /// Whether the service runs or is on its way up (starting, restarting),
    /// as opposed to stopped, failed or exited.
    pub runs: bool,
    /// Why a running box unit's port cannot be reached from here: `{port,
    /// state, message}` from the mirror view, which the CLI words.
    pub unmirrored: Option<Value>,
}

/// A lane and its GUI services.
#[derive(Debug)]
pub struct Found {
    /// The lane's reference, such as `acme@2`.
    pub lane: String,
    /// The box the lane runs on, for a box's lane.
    pub host_box: Option<String>,
    /// The lane's GUI services.
    pub services: Vec<Service>,
    /// Every unit name of the lane, to tell a unit that is no GUI service
    /// from one that does not exist.
    pub units: Vec<String>,
}

/// Whether a unit in `state`, of this machine or of a box, runs or is on its
/// way up. A unit being restarted passes through `stopping` and `starting`,
/// which count.
pub fn unit_runs(state: &str) -> bool {
    !matches!(state, "stopped" | "failed" | "exited" | "idle")
}

/// The GUI services of a lane of this machine's `status`.
pub fn lane_services(lane: &LaneSummary) -> Vec<Service> {
    lane.units
        .iter()
        .filter_map(|unit| {
            let gui = unit.gui.as_ref()?;
            let port = unit.ports.get(GUI_PORT).copied();
            let host_runs = gui.host == devlanes_proto::GUI_HOST_RUNNING;
            Some(Service {
                unit: unit.name.clone(),
                state: unit.state.clone(),
                host: gui.host.clone(),
                address: port
                    .filter(|_| host_runs)
                    .map(|port| SocketAddr::from((Ipv4Addr::LOCALHOST, port))),
                real_desktop: gui.real_desktop.filter(|_| host_runs),
                runs: unit_runs(&unit.state),
                unmirrored: None,
            })
        })
        .collect()
}

/// Every box lane of a `remote ls` answer, each from the box this machine
/// mirrors it from, else the one `remote use` chose, else the first.
pub fn box_lanes(data: &Value) -> Vec<Found> {
    let mut references: Vec<String> = Vec::new();
    for host in data["items"].as_array().into_iter().flatten() {
        for unit in host["registry"]["units"].as_array().into_iter().flatten() {
            let lane = &unit["key"]["lane"];
            let (Some(project), Some(index)) = (lane["project"].as_str(), lane["lane"].as_str())
            else {
                continue;
            };
            let reference = format!("{project}@{index}");
            if !references.contains(&reference) {
                references.push(reference);
            }
        }
    }
    references
        .iter()
        .filter_map(|reference| box_lane(data, reference))
        .collect()
}

/// The box lane `reference` (`project@index`) from a `remote ls` answer. Of
/// several boxes with that lane, the one whose ports this machine mirrors,
/// else the one `remote use` chose, else the first.
pub fn box_lane(data: &Value, reference: &str) -> Option<Found> {
    let (project, index) = reference.split_once('@')?;
    let empty = Vec::new();
    let mut candidates = Vec::new();
    for host in data["items"].as_array().unwrap_or(&empty) {
        let units: Vec<&Value> = host["registry"]["units"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|unit| {
                unit["key"]["lane"]["project"] == project && unit["key"]["lane"]["lane"] == index
            })
            .collect();
        if units.is_empty() {
            continue;
        }
        let services: Vec<Service> = units
            .iter()
            .filter(|unit| unit["key"]["kind"] == "service")
            .filter_map(|unit| box_service(host, reference, unit))
            .collect();
        candidates.push(Found {
            lane: reference.to_owned(),
            host_box: host["host"].as_str().map(str::to_owned),
            units: units
                .iter()
                .filter_map(|unit| unit["key"]["name"].as_str().map(str::to_owned))
                .collect(),
            services,
        });
    }
    let chosen = |found: &Found| {
        data["items"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .any(|host| {
                host["host"].as_str() == found.host_box.as_deref()
                    && host["chosen_lanes"]
                        .as_array()
                        .is_some_and(|lanes| lanes.iter().any(|lane| lane == reference))
            })
    };
    let pick = candidates
        .iter()
        .position(|found| found.services.iter().any(|s| s.address.is_some()))
        .or_else(|| candidates.iter().position(chosen))
        .unwrap_or(0);
    (pick < candidates.len()).then(|| candidates.swap_remove(pick))
}

/// A box unit with a `devlanes_gui` port, and the address its mirror has
/// here when the port is bound.
fn box_service(host: &Value, lane: &str, unit: &Value) -> Option<Service> {
    let name = unit["key"]["name"].as_str()?;
    let port = unit["ports"]
        .as_array()?
        .iter()
        .find(|port| port["name"] == GUI_PORT)?["port"]
        .as_u64()?;
    let state = unit["state"].as_str().unwrap_or_default().to_owned();
    let serving = devlanes_transport::serving(&state);
    // The same rule as for this machine's units: a box unit that restarts
    // passes through `stopping` too.
    let runs = unit_runs(&state);
    let mirror = host["ports"].as_array().and_then(|ports| {
        ports
            .iter()
            .find(|entry| entry["port"] == port && entry["lane"] == lane && entry["unit"] == name)
    });
    // Only a loopback listener of the mirror, whatever else the list says.
    let address = mirror
        .filter(|entry| entry["state"] == "bound")
        .and_then(|entry| {
            let listed = |address: IpAddr| {
                entry["addresses"].as_array().is_some_and(|addresses| {
                    addresses
                        .iter()
                        .any(|listed| listed.as_str().and_then(|a| a.parse().ok()) == Some(address))
                })
            };
            [
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ]
            .into_iter()
            .find(|loopback| listed(*loopback))
        })
        .zip(u16::try_from(port).ok())
        .map(SocketAddr::from);
    // Why the port is not reachable here, for the CLI to word: the mirror's
    // own message when something holds the port, else its state.
    let unmirrored = (serving && address.is_none()).then(|| {
        json!({
            "port": port,
            "state": mirror.and_then(|entry| entry["state"].as_str()),
            "message": mirror
                .and_then(|entry| entry["conflicts"].as_array()?.first()?["message"].as_str()),
        })
    });
    Some(Service {
        unit: name.to_owned(),
        state,
        host: if serving { "running" } else { "stopped" }.to_owned(),
        address,
        real_desktop: None,
        runs,
        unmirrored,
    })
}
