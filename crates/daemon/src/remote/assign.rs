//! Which lane serves a hostname that several lanes claim: this machine's lanes
//! and the lanes of every connected box. Lanes clash when their hostnames
//! overlap, wildcards included ([`overlap`]). The same lane reference on two
//! boxes always clashes, also without hostnames, because the relays and the
//! routes that name a box port directly belong to the reference. Of a set of
//! clashing lanes one is mirrored, by this order:
//!
//! 1. a running local lane,
//! 2. the box lane chosen with `remote use`,
//! 3. a stopped local lane,
//! 4. the box lane whose box connected first.
//!
//! The others are skipped whole. [`assign`] is pure, so the mirrors, the route
//! publisher and `host.list` all read one decision, whichever box reconciles
//! first.
//!
//! The same decision names the box behind each local port and each relay
//! (see [`Listeners`](super::listeners::Listeners)). A port goes to the
//! mirrored lane that wants it, and when lanes of two boxes want one number
//! without sharing hostnames, a chosen lane gets it first, then the box that
//! connected first.

use std::collections::{BTreeMap, BTreeSet};

use super::mirror::below;

/// One of this machine's lanes with routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalLane {
    /// `project@index`.
    pub lane: String,
    /// Whether it holds its ports, so it wins over every box lane.
    pub running: bool,
    /// What holds them while it runs.
    pub hold: Option<Hold>,
    /// Its routed hostnames.
    pub hosts: BTreeSet<String>,
}

/// What keeps a running local lane's ports, which says how to free them.
/// Processes and the lane's own containers can both be up. `devlanes stop`
/// stops the processes only, so the commands name both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Hold {
    /// Units or orphans run, or a start is under way.
    pub processes: bool,
    /// Only a start under way holds the ports so far, with no unit or
    /// container up yet. The lane wins, but choices it beats are kept until
    /// something runs: a start that fails gives the box lane back.
    pub starting: bool,
    /// The lane's Compose project, while one of its containers that
    /// publishes a host port is up.
    pub compose_project: Option<String>,
    /// The lane's plain containers that are up and publish a host port, by
    /// Docker name.
    pub containers: Vec<String>,
}

impl Hold {
    /// A hold by processes only.
    pub(crate) fn processes() -> Self {
        Self {
            processes: true,
            ..Self::default()
        }
    }

    /// Whether any of the lane's own containers counts.
    pub(crate) fn has_containers(&self) -> bool {
        self.compose_project.is_some() || !self.containers.is_empty()
    }

    /// The `holds` value of `local_lane_running` and skipped lanes.
    pub(crate) fn as_str(&self) -> &'static str {
        if self.processes {
            "processes"
        } else {
            "containers"
        }
    }

    /// `holds`, and `compose_project` and `containers` while the lane's own
    /// containers count, as fields to merge into error data or a skipped lane.
    pub(crate) fn fields(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("holds".into(), self.as_str().into());
        if self.has_containers() {
            fields.insert(
                "compose_project".into(),
                self.compose_project.clone().into(),
            );
            fields.insert("containers".into(), self.containers.clone().into());
        }
        fields
    }

    /// The sentences that free the ports of local lane `lane`, one per kind
    /// of command, each ending in a period: "Stop it with devlanes stop
    /// --lane acme@0." and "Stop its containers with …", or "Stop them with
    /// …" when only containers run. Only the lane's own containers: its
    /// Compose project holds no shared service, and plain containers are
    /// named by Docker name.
    pub(crate) fn stop_sentences(&self, lane: &str) -> String {
        let mut commands = Vec::new();
        if let Some(project) = &self.compose_project {
            commands.push(format!("docker compose -p {project} stop"));
        }
        if !self.containers.is_empty() {
            commands.push(format!("docker stop {}", self.containers.join(" ")));
        }
        let containers = commands.join(" and ");
        match (self.processes, containers.is_empty()) {
            (true, true) => format!("Stop it with devlanes stop --lane {lane}."),
            (true, false) => format!(
                "Stop it with devlanes stop --lane {lane}. Stop its containers with {containers}."
            ),
            (false, _) => format!("Stop them with {containers}."),
        }
    }

    /// What runs, as the start of a sentence about local lane `lane`.
    pub(crate) fn runs(&self, lane: &str) -> String {
        if self.processes {
            format!("{lane} runs on this machine")
        } else {
            format!("{lane}'s containers run on this machine")
        }
    }
}

/// A connected box and its mirrorable lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BoxLanes {
    pub host: String,
    /// Place in connect order: lower connected earlier.
    pub serial: u64,
    /// Hostnames this machine would route, by lane reference.
    pub lanes: BTreeMap<String, BTreeSet<String>>,
    /// The ports of each lane's mirrorable units, by lane reference.
    pub ports: BTreeMap<String, Vec<PortUse>>,
}

/// One port of a box unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PortUse {
    pub port: u16,
    pub unit: String,
    /// The unit serves, so the port is bound here.
    pub serving: bool,
    /// One of the lane's hostnames routes to it.
    pub routed: bool,
}

/// The box lane behind a local port or relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortOwner {
    pub host: String,
    pub lane: String,
    pub unit: String,
}

/// Why a lane is not mirrored: the kind of lane that serves its hostnames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Cause {
    LocalRunning,
    BoxChosen,
    LocalStopped,
    BoxEarlier,
}

impl Cause {
    /// The name `host.list` and `lane_skipped` use.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::LocalRunning => "local_running",
            Self::BoxChosen => "box_chosen",
            Self::LocalStopped => "local_stopped",
            Self::BoxEarlier => "box_earlier",
        }
    }
}

/// A box lane that is not mirrored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skip {
    pub cause: Cause,
    /// The box whose lane serves the hostnames, or `None` for a local lane.
    pub winner: Option<String>,
    /// The serving lane's reference.
    pub winner_lane: String,
    /// This lane's hostnames that clash with a mirrored lane.
    pub hosts: BTreeSet<String>,
    /// Local lanes that serve any of them.
    pub local_lanes: BTreeSet<String>,
    /// What holds the ports of the running local lane in use, when that is
    /// the cause.
    pub hold: Option<Hold>,
}

/// What [`assign`] decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Assignment {
    /// Box lanes that are mirrored, as `(box, lane)`. A lane not listed here
    /// is not, also one the assignment has not seen yet.
    pub mirrored: BTreeSet<(String, String)>,
    /// Box lanes that are skipped, by `(box, lane)`.
    pub skipped: BTreeMap<(String, String), Skip>,
    /// This machine's stopped lanes that gave their hostnames to a chosen box
    /// lane, with that box.
    pub yielded: BTreeMap<String, String>,
    /// Choices whose box lane loses to a running local lane, and choices for
    /// the reference of a running local lane, connected or not, lane to box.
    /// Starting a local lane deletes them.
    pub cleared: BTreeMap<String, String>,
    /// Box lanes that are mirrored because a choice names them, as
    /// `(box, lane)`. A mirror says nothing when one of them stops being
    /// mirrored, since the choice's change already said so.
    pub chosen: BTreeSet<(String, String)>,
    /// The box lane behind each local port: ports of serving units of
    /// mirrored lanes.
    pub listeners: BTreeMap<u16, PortOwner>,
    /// The box lane behind each relay, by lane and box port: routed ports of
    /// mirrored lanes. A relay belongs to the lane, so forks of one lane
    /// share it and a switch leaves its port as it is.
    pub relays: BTreeMap<(String, u16), PortOwner>,
}

impl Assignment {
    /// Whether box `host` mirrors its lane `lane`.
    pub(crate) fn mirrors(&self, host: &str, lane: &str) -> bool {
        self.mirrored.contains(&(host.to_owned(), lane.to_owned()))
    }

    /// Why box `host` does not mirror its lane `lane`, if it is skipped.
    pub(crate) fn skip(&self, host: &str, lane: &str) -> Option<&Skip> {
        self.skipped.get(&(host.to_owned(), lane.to_owned()))
    }
}

/// Who a candidate lane belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Owner<'a> {
    Local,
    Box(&'a str),
}

/// A lane competing for its hostnames, ranked by the order in the module docs.
struct Candidate<'a> {
    /// Sorts winners first: the cause it beats others with, then connect
    /// order, box and lane, so equal ranks never depend on input order.
    rank: (Cause, u64, &'a str, &'a str),
    owner: Owner<'a>,
    lane: &'a str,
    hosts: &'a BTreeSet<String>,
    hold: Option<&'a Hold>,
}

/// Decides which lanes are mirrored, given this machine's `local` lanes, the
/// connected `boxes` and the `choices` (lane to box). Local lanes never clash
/// with each other (the local routes settle that), and neither do lanes of one
/// box (the box settles that).
pub(super) fn assign(
    local: &[LocalLane],
    boxes: &[BoxLanes],
    choices: &BTreeMap<String, String>,
) -> Assignment {
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for lane in local {
        let cause = if lane.running {
            Cause::LocalRunning
        } else {
            Cause::LocalStopped
        };
        candidates.push(Candidate {
            rank: (cause, 0, "", &lane.lane),
            owner: Owner::Local,
            lane: &lane.lane,
            hosts: &lane.hosts,
            hold: lane.hold.as_ref().filter(|_| lane.running),
        });
    }
    for host in boxes {
        for (lane, hosts) in &host.lanes {
            let chosen = choices.get(lane).is_some_and(|chosen| *chosen == host.host);
            let cause = if chosen {
                Cause::BoxChosen
            } else {
                Cause::BoxEarlier
            };
            candidates.push(Candidate {
                rank: (cause, host.serial, &host.host, lane),
                owner: Owner::Box(&host.host),
                lane,
                hosts,
                hold: None,
            });
        }
    }
    candidates.sort_by(|a, b| a.rank.cmp(&b.rank));

    let mut assignment = Assignment::default();
    let mut winners: Vec<&Candidate<'_>> = Vec::new();
    for candidate in &candidates {
        let clashing: Vec<&&Candidate<'_>> = winners
            .iter()
            .filter(|winner| match (&winner.owner, &candidate.owner) {
                (Owner::Local, Owner::Local) => false,
                (Owner::Box(a), Owner::Box(b)) if a == b => false,
                (Owner::Box(_), Owner::Box(_)) => {
                    winner.lane == candidate.lane || clash(winner.hosts, candidate.hosts)
                }
                _ => clash(winner.hosts, candidate.hosts),
            })
            .collect();
        let Some(first) = clashing.first() else {
            if let Owner::Box(host) = candidate.owner {
                let key = (host.to_owned(), candidate.lane.to_owned());
                if candidate.rank.0 == Cause::BoxChosen {
                    assignment.chosen.insert(key.clone());
                }
                assignment.mirrored.insert(key);
            }
            winners.push(candidate);
            continue;
        };
        match candidate.owner {
            // Only a chosen box lane ranks above a stopped local lane.
            Owner::Local => {
                if let Owner::Box(winner) = first.owner {
                    assignment
                        .yielded
                        .insert(candidate.lane.to_owned(), winner.to_owned());
                }
            }
            Owner::Box(host) => {
                let hosts = candidate
                    .hosts
                    .iter()
                    .filter(|name| {
                        clashing
                            .iter()
                            .any(|winner| winner.hosts.iter().any(|held| overlap(held, name)))
                    })
                    .cloned()
                    .collect();
                let local_lanes = clashing
                    .iter()
                    .filter(|winner| winner.owner == Owner::Local)
                    .map(|winner| winner.lane.to_owned())
                    .collect();
                let cause = first.rank.0;
                let settled = first.hold.is_none_or(|hold| !hold.starting);
                if candidate.rank.0 == Cause::BoxChosen && cause == Cause::LocalRunning && settled {
                    assignment
                        .cleared
                        .insert(candidate.lane.to_owned(), host.to_owned());
                }
                let winner = match first.owner {
                    Owner::Local => None,
                    Owner::Box(winner) => Some(winner.to_owned()),
                };
                assignment.skipped.insert(
                    (host.to_owned(), candidate.lane.to_owned()),
                    Skip {
                        cause,
                        winner,
                        winner_lane: first.lane.to_owned(),
                        hosts,
                        local_lanes,
                        hold: (cause == Cause::LocalRunning)
                            .then(|| first.hold.cloned().unwrap_or_else(Hold::processes)),
                    },
                );
            }
        }
    }
    // Also for a box that is not connected: its lane would lose once it is.
    // A start that has nothing up yet keeps the choice, in case it fails.
    let settled =
        |lane: &&LocalLane| lane.running && lane.hold.as_ref().is_none_or(|hold| !hold.starting);
    for lane in local.iter().filter(settled) {
        if let Some(host) = choices.get(&lane.lane) {
            assignment.cleared.insert(lane.lane.clone(), host.clone());
        }
    }
    assign_ports(&mut assignment, boxes, choices);
    assignment
}

/// The choices, as `(lane, box)`, that a new choice of lane `lane` with
/// hostnames `hosts` replaces: those of connected box lanes whose hostnames
/// overlap. `owner` is the chosen box, whose own lanes never clash with
/// `lane`, or `None` for this machine's lane, which also replaces any choice
/// of its own reference.
pub(super) fn clashing_choices(
    lane: &str,
    hosts: &BTreeSet<String>,
    owner: Option<&str>,
    boxes: &[BoxLanes],
    choices: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    choices
        .iter()
        .filter(|(chosen, host)| {
            if owner == Some(host.as_str()) {
                return false;
            }
            if chosen.as_str() == lane {
                // A box choice of `lane` overwrites it anyway.
                return owner.is_none();
            }
            boxes
                .iter()
                .find(|other| other.host == **host)
                .and_then(|other| other.lanes.get(*chosen))
                .is_some_and(|theirs| clash(theirs, hosts))
        })
        .map(|(chosen, host)| (chosen.clone(), host.clone()))
        .collect()
}

/// Fills in the box lane behind each local port and relay, from the mirrored
/// lanes. Chosen lanes come first, then connect order, box and lane.
fn assign_ports(
    assignment: &mut Assignment,
    boxes: &[BoxLanes],
    choices: &BTreeMap<String, String>,
) {
    let mut lanes: Vec<(bool, u64, &str, &str, &[PortUse])> = Vec::new();
    for host in boxes {
        for (lane, ports) in &host.ports {
            if assignment.mirrors(&host.host, lane) {
                let chosen = choices.get(lane).is_some_and(|chosen| *chosen == host.host);
                lanes.push((!chosen, host.serial, &host.host, lane, ports));
            }
        }
    }
    lanes.sort_by(|a, b| (a.0, a.1, a.2, a.3).cmp(&(b.0, b.1, b.2, b.3)));
    for (_, _, host, lane, ports) in lanes {
        for used in ports {
            let owner = || PortOwner {
                host: host.to_owned(),
                lane: lane.to_owned(),
                unit: used.unit.clone(),
            };
            if used.serving {
                assignment.listeners.entry(used.port).or_insert_with(owner);
            }
            if used.routed {
                assignment
                    .relays
                    .entry((lane.to_owned(), used.port))
                    .or_insert_with(owner);
            }
        }
    }
}

/// Whether any hostname of `a` overlaps one of `b`.
fn clash(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    a.iter().any(|x| b.iter().any(|y| overlap(x, y)))
}

/// Whether some name matches both route patterns, each an exact name or a
/// `*.` wildcard covering any depth below its parent (but not the parent),
/// as the proxy matches them. Case and a trailing dot are ignored.
pub(super) fn overlap(a: &str, b: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A set of hostnames.
    fn names(hosts: &[&str]) -> BTreeSet<String> {
        hosts.iter().map(|host| (*host).to_owned()).collect()
    }

    /// A local lane with `hosts`.
    fn local(lane: &str, running: bool, hosts: &[&str]) -> LocalLane {
        LocalLane {
            lane: lane.into(),
            running,
            hold: running.then(Hold::processes),
            hosts: names(hosts),
        }
    }

    /// A start that has nothing up yet wins the lane but keeps the choice
    /// it beats, of a connected box or not, so a failed start gives the box
    /// lane back.
    #[test]
    fn a_start_with_nothing_up_keeps_the_choice() {
        let mut lane = local("acme@0", true, WEB);
        lane.hold = Some(Hold {
            processes: true,
            starting: true,
            ..Hold::default()
        });
        let boxes = [
            on_box("fork-a", 1, &[("acme@0", WEB)]),
            on_box("fork-b", 2, &[("acme@0", WEB)]),
        ];
        let got = assign(&[lane.clone()], &boxes, &chose(&[("acme@0", "fork-b")]));
        assert!(got.mirrored.is_empty());
        assert!(got.cleared.is_empty(), "{:?}", got.cleared);
        let got = assign(&[lane], &boxes[..1], &chose(&[("acme@0", "fork-c")]));
        assert!(got.cleared.is_empty(), "{:?}", got.cleared);
    }

    /// A skip by a lane whose own containers run names them, and the
    /// commands stop only those: the lane's Compose project and its plain
    /// containers. With processes up too, both commands are named.
    #[test]
    fn a_lane_held_by_containers_says_how_to_stop_them() {
        let hold = Hold {
            compose_project: Some("acme-lane0".into()),
            containers: vec!["devlanes-acme-lane0-cerbos".into()],
            ..Hold::default()
        };
        let mut lane = local("acme@0", true, WEB);
        lane.hold = Some(hold.clone());
        let got = assign(
            &[lane],
            &[on_box("fork-b", 1, &[("acme@0", WEB)])],
            &chose(&[]),
        );
        let skip = got.skip("fork-b", "acme@0").expect("skipped");
        assert_eq!(skip.hold.as_ref(), Some(&hold));
        assert_eq!(
            hold.stop_sentences("acme@0"),
            "Stop them with docker compose -p acme-lane0 stop and docker stop devlanes-acme-lane0-cerbos."
        );
        assert_eq!(
            hold.runs("acme@0"),
            "acme@0's containers run on this machine"
        );
        assert_eq!(
            Hold::processes().stop_sentences("acme@0"),
            "Stop it with devlanes stop --lane acme@0."
        );
        assert!(!Hold::processes().fields().contains_key("compose_project"));
        let fields = hold.fields();
        assert_eq!(fields["holds"], "containers");
        assert_eq!(fields["compose_project"], "acme-lane0");
        let both = Hold {
            processes: true,
            compose_project: Some("acme-lane0".into()),
            ..Hold::default()
        };
        assert_eq!(
            both.stop_sentences("acme@0"),
            "Stop it with devlanes stop --lane acme@0. Stop its containers with docker compose -p acme-lane0 stop."
        );
        assert_eq!(both.runs("acme@0"), "acme@0 runs on this machine");
        let fields = both.fields();
        assert_eq!(fields["holds"], "processes");
        assert_eq!(fields["compose_project"], "acme-lane0");
    }

    /// Box `host`, connected `serial`th, with `lanes` and their hostnames.
    fn on_box(host: &str, serial: u64, lanes: &[(&str, &[&str])]) -> BoxLanes {
        BoxLanes {
            host: host.into(),
            serial,
            lanes: lanes
                .iter()
                .map(|(lane, hosts)| ((*lane).to_owned(), names(hosts)))
                .collect(),
            ports: BTreeMap::new(),
        }
    }

    /// `host` with `ports` for its lanes, as `(lane, port, unit, serving, routed)`.
    fn with_ports(mut host: BoxLanes, ports: &[(&str, u16, &str, bool, bool)]) -> BoxLanes {
        for (lane, port, unit, serving, routed) in ports {
            host.ports
                .entry((*lane).to_owned())
                .or_default()
                .push(PortUse {
                    port: *port,
                    unit: (*unit).to_owned(),
                    serving: *serving,
                    routed: *routed,
                });
        }
        host
    }

    /// The owner of each listener or relay as `(port, box)`.
    fn owners<K: Clone>(map: &BTreeMap<K, PortOwner>) -> Vec<(K, String)> {
        map.iter()
            .map(|(key, owner)| (key.clone(), owner.host.clone()))
            .collect()
    }

    /// A port goes to the mirrored lane of the forks, a skipped fork owns
    /// nothing, and an idle unit gets a relay but no listener. Two boxes
    /// whose lanes share only a port number give it to the chosen lane, and
    /// without a choice to the box connected first.
    #[test]
    fn ports_follow_the_mirrored_lane() {
        let forks = |choices: &[(&str, &str)]| {
            assign(
                &[],
                &[
                    with_ports(
                        on_box("fork-a", 1, &[("acme@0", WEB)]),
                        &[
                            ("acme@0", 3000, "web", true, true),
                            ("acme@0", 3001, "api", false, true),
                        ],
                    ),
                    with_ports(
                        on_box("fork-b", 2, &[("acme@0", WEB)]),
                        &[("acme@0", 3000, "web", true, true)],
                    ),
                ],
                &chose(choices),
            )
        };
        let first = forks(&[]);
        assert_eq!(owners(&first.listeners), [(3000, "fork-a".to_owned())]);
        assert_eq!(
            owners(&first.relays),
            [
                (("acme@0".to_owned(), 3000), "fork-a".to_owned()),
                (("acme@0".to_owned(), 3001), "fork-a".to_owned()),
            ]
        );
        let switched = forks(&[("acme@0", "fork-b")]);
        assert_eq!(owners(&switched.listeners), [(3000, "fork-b".to_owned())]);
        assert_eq!(
            owners(&switched.relays),
            [(("acme@0".to_owned(), 3000), "fork-b".to_owned())]
        );

        let apart = |choices: &[(&str, &str)]| {
            assign(
                &[],
                &[
                    with_ports(
                        on_box("box-a", 1, &[("acme@0", WEB)]),
                        &[("acme@0", 4000, "db", true, false)],
                    ),
                    with_ports(
                        on_box("box-b", 2, &[("shop@0", &["web.shop.localhost"])]),
                        &[("shop@0", 4000, "db", true, false)],
                    ),
                ],
                &chose(choices),
            )
        };
        assert_eq!(owners(&apart(&[]).listeners), [(4000, "box-a".to_owned())]);
        assert_eq!(
            owners(&apart(&[("shop@0", "box-b")]).listeners),
            [(4000, "box-b".to_owned())]
        );
    }

    /// Choices from `(lane, box)` pairs.
    fn chose(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(lane, host)| ((*lane).to_owned(), (*host).to_owned()))
            .collect()
    }

    const WEB: &[&str] = &["web.acme.localhost"];
    const NONE: &[&str] = &[];

    /// Forks whose `acme@0` has no hostnames still share its port: it goes
    /// to the box connected first, and to the other one once chosen, which
    /// is then listed as mirrored by its choice.
    #[test]
    fn a_lane_without_hostnames_moves_its_ports_with_a_choice() {
        let forks = |choices: &[(&str, &str)]| {
            let fork = |name: &str, serial| {
                with_ports(
                    on_box(name, serial, &[("acme@0", NONE)]),
                    &[("acme@0", 4000, "db", true, false)],
                )
            };
            assign(
                &[],
                &[fork("fork-a", 1), fork("fork-b", 2)],
                &chose(choices),
            )
        };
        let first = forks(&[]);
        assert_eq!(owners(&first.listeners), [(4000, "fork-a".to_owned())]);
        assert!(first.chosen.is_empty());
        let switched = forks(&[("acme@0", "fork-b")]);
        assert_eq!(owners(&switched.listeners), [(4000, "fork-b".to_owned())]);
        assert!(switched.skip("fork-a", "acme@0").is_some());
        assert_eq!(
            switched.chosen.iter().cloned().collect::<Vec<_>>(),
            pairs(&[("fork-b", "acme@0")])
        );
    }

    /// A box choice replaces the choices of other lanes whose hostnames
    /// overlap, on other boxes. This machine's lane also replaces any choice
    /// of its own reference, connected or not.
    #[test]
    fn a_new_choice_replaces_the_ones_it_clashes_with() {
        let boxes = [
            on_box("fork-a", 1, &[("shop@0", &["*.acme.localhost"])]),
            on_box(
                "fork-b",
                2,
                &[("acme@0", WEB), ("acme@1", &["x.example.localhost"])],
            ),
        ];
        let choices = chose(&[
            ("shop@0", "fork-a"),
            ("acme@1", "fork-b"),
            ("acme@0", "fork-c"),
        ]);
        let hosts = names(WEB);
        assert_eq!(
            clashing_choices("acme@0", &hosts, Some("fork-b"), &boxes, &choices),
            pairs(&[("shop@0", "fork-a")])
        );
        assert_eq!(
            clashing_choices("acme@0", &hosts, None, &boxes, &choices),
            pairs(&[("acme@0", "fork-c"), ("shop@0", "fork-a")])
        );
    }

    /// What one case expects: the mirrored box lanes, and per skipped box
    /// lane its cause and winner. Yielded and cleared lanes are checked
    /// apart.
    struct Case {
        name: &'static str,
        local: Vec<LocalLane>,
        boxes: Vec<BoxLanes>,
        choices: BTreeMap<String, String>,
        mirrored: &'static [(&'static str, &'static str)],
        skipped: &'static [(&'static str, &'static str, Cause, Option<&'static str>)],
        yielded: &'static [(&'static str, &'static str)],
        cleared: &'static [(&'static str, &'static str)],
    }

    /// The table of cases the module docs' order implies.
    fn cases() -> Vec<Case> {
        let forks = || {
            vec![
                on_box("fork-a", 1, &[("acme@0", WEB)]),
                on_box("fork-b", 2, &[("acme@0", WEB)]),
            ]
        };
        vec![
            Case {
                name: "no choice: the first connected box wins",
                local: vec![],
                boxes: forks(),
                choices: chose(&[]),
                mirrored: &[("fork-a", "acme@0")],
                skipped: &[("fork-b", "acme@0", Cause::BoxEarlier, Some("fork-a"))],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "a choice beats an earlier box",
                local: vec![],
                boxes: forks(),
                choices: chose(&[("acme@0", "fork-b")]),
                mirrored: &[("fork-b", "acme@0")],
                skipped: &[("fork-a", "acme@0", Cause::BoxChosen, Some("fork-b"))],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "a running local lane beats a choice",
                local: vec![local("acme@0", true, WEB)],
                boxes: forks(),
                choices: chose(&[("acme@0", "fork-b")]),
                mirrored: &[],
                skipped: &[
                    ("fork-a", "acme@0", Cause::LocalRunning, None),
                    ("fork-b", "acme@0", Cause::LocalRunning, None),
                ],
                yielded: &[],
                cleared: &[("acme@0", "fork-b")],
            },
            Case {
                name: "a choice beats a stopped local lane",
                local: vec![local("acme@0", false, WEB)],
                boxes: forks(),
                choices: chose(&[("acme@0", "fork-b")]),
                mirrored: &[("fork-b", "acme@0")],
                skipped: &[("fork-a", "acme@0", Cause::BoxChosen, Some("fork-b"))],
                yielded: &[("acme@0", "fork-b")],
                cleared: &[],
            },
            Case {
                name: "a stopped local lane beats an unchosen box",
                local: vec![local("acme@0", false, WEB)],
                boxes: forks(),
                choices: chose(&[]),
                mirrored: &[],
                skipped: &[
                    ("fork-a", "acme@0", Cause::LocalStopped, None),
                    ("fork-b", "acme@0", Cause::LocalStopped, None),
                ],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "a box wildcard clashes with a local name",
                local: vec![local("acme@0", false, WEB)],
                boxes: vec![on_box("fork-a", 1, &[("acme@0", &["*.acme.localhost"])])],
                choices: chose(&[]),
                mirrored: &[],
                skipped: &[("fork-a", "acme@0", Cause::LocalStopped, None)],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "a box name clashes with a local wildcard",
                local: vec![local("acme@0", true, &["*.acme.localhost"])],
                boxes: vec![on_box("fork-a", 1, &[("acme@0", WEB)])],
                choices: chose(&[]),
                mirrored: &[],
                skipped: &[("fork-a", "acme@0", Cause::LocalRunning, None)],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "two projects with overlapping names clash across lanes",
                local: vec![],
                boxes: vec![
                    on_box("fork-a", 1, &[("shop@0", &["*.acme.localhost"])]),
                    on_box(
                        "fork-b",
                        2,
                        &[("acme@0", WEB), ("acme@1", &["x.example.localhost"])],
                    ),
                ],
                choices: chose(&[]),
                mirrored: &[("fork-a", "shop@0"), ("fork-b", "acme@1")],
                skipped: &[("fork-b", "acme@0", Cause::BoxEarlier, Some("fork-a"))],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "an offline chosen box means the next one wins",
                local: vec![],
                boxes: vec![
                    on_box("fork-a", 1, &[("acme@0", WEB)]),
                    on_box("fork-c", 3, &[("acme@0", WEB)]),
                ],
                choices: chose(&[("acme@0", "fork-b")]),
                mirrored: &[("fork-a", "acme@0")],
                skipped: &[("fork-c", "acme@0", Cause::BoxEarlier, Some("fork-a"))],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "lanes that clash with nothing are all mirrored",
                local: vec![local("acme@1", true, &["web.acme1.localhost"])],
                boxes: vec![
                    on_box("fork-a", 1, &[("acme@0", WEB)]),
                    on_box("fork-b", 2, &[("acme@2", &["web.acme2.localhost"])]),
                ],
                choices: chose(&[("acme@0", "fork-a")]),
                mirrored: &[("fork-a", "acme@0"), ("fork-b", "acme@2")],
                skipped: &[],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "the same lane on two boxes clashes without shared hostnames",
                local: vec![],
                boxes: vec![
                    on_box("fork-a", 1, &[("acme@0", &["a.acme.localhost"])]),
                    on_box("fork-b", 2, &[("acme@0", &["b.acme.localhost"])]),
                ],
                choices: chose(&[]),
                mirrored: &[("fork-a", "acme@0")],
                skipped: &[("fork-b", "acme@0", Cause::BoxEarlier, Some("fork-a"))],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "the same lane on two boxes clashes without any hostname",
                local: vec![],
                boxes: vec![
                    on_box("fork-a", 1, &[("acme@0", NONE)]),
                    on_box("fork-b", 2, &[("acme@0", NONE)]),
                ],
                choices: chose(&[("acme@0", "fork-b")]),
                mirrored: &[("fork-b", "acme@0")],
                skipped: &[("fork-a", "acme@0", Cause::BoxChosen, Some("fork-b"))],
                yielded: &[],
                cleared: &[],
            },
            Case {
                name: "a running local lane clears the choice of a box that is not connected",
                local: vec![local("acme@0", true, WEB)],
                boxes: vec![on_box("fork-a", 1, &[("acme@0", WEB)])],
                choices: chose(&[("acme@0", "fork-b")]),
                mirrored: &[],
                skipped: &[("fork-a", "acme@0", Cause::LocalRunning, None)],
                yielded: &[],
                cleared: &[("acme@0", "fork-b")],
            },
            Case {
                name: "lanes of one box never clash with each other",
                local: vec![],
                boxes: vec![on_box("fork-a", 1, &[("acme@0", WEB), ("acme@1", WEB)])],
                choices: chose(&[]),
                mirrored: &[("fork-a", "acme@0"), ("fork-a", "acme@1")],
                skipped: &[],
                yielded: &[],
                cleared: &[],
            },
        ]
    }

    /// Pairs of `(a, b)` as owned strings.
    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    /// Every case of [`cases`] gets the verdicts it lists.
    #[test]
    fn lanes_are_mirrored_by_the_documented_order() {
        for case in cases() {
            let got = assign(&case.local, &case.boxes, &case.choices);
            let mirrored: Vec<(String, String)> = got.mirrored.iter().cloned().collect();
            assert_eq!(mirrored, pairs(case.mirrored), "{}: mirrored", case.name);
            let skipped: Vec<(String, String, Cause, Option<String>)> = got
                .skipped
                .iter()
                .map(|((host, lane), skip)| {
                    (host.clone(), lane.clone(), skip.cause, skip.winner.clone())
                })
                .collect();
            let expected: Vec<(String, String, Cause, Option<String>)> = case
                .skipped
                .iter()
                .map(|(host, lane, cause, winner)| {
                    (
                        (*host).to_owned(),
                        (*lane).to_owned(),
                        *cause,
                        winner.map(str::to_owned),
                    )
                })
                .collect();
            assert_eq!(skipped, expected, "{}: skipped", case.name);
            let yielded: Vec<(String, String)> = got.yielded.into_iter().collect();
            assert_eq!(yielded, pairs(case.yielded), "{}: yielded", case.name);
            let cleared: Vec<(String, String)> = got.cleared.into_iter().collect();
            assert_eq!(cleared, pairs(case.cleared), "{}: cleared", case.name);
        }
    }

    /// Reversing local lanes, boxes and each box's lanes gives the same
    /// assignment: the order lanes arrive in decides nothing.
    #[test]
    fn the_result_does_not_depend_on_input_order() {
        for case in cases() {
            let forward = assign(&case.local, &case.boxes, &case.choices);
            let mut local = case.local.clone();
            local.reverse();
            let mut boxes = case.boxes.clone();
            boxes.reverse();
            assert_eq!(
                assign(&local, &boxes, &case.choices),
                forward,
                "{}",
                case.name
            );
        }
    }

    /// A skipped lane names the clashing hostnames, the local lanes that hold
    /// them and the serving lane.
    #[test]
    fn a_skip_names_what_clashes() {
        let got = assign(
            &[local("acme@0", true, &["*.acme.localhost"])],
            &[on_box(
                "fork-a",
                1,
                &[("acme@0", &["web.acme.localhost", "api.other.localhost"])],
            )],
            &chose(&[]),
        );
        let skip = got.skip("fork-a", "acme@0").expect("skipped");
        assert_eq!(skip.hosts, names(&["web.acme.localhost"]));
        assert_eq!(skip.local_lanes, names(&["acme@0"]));
        assert_eq!(skip.winner_lane, "acme@0");
        assert_eq!(skip.cause.as_str(), "local_running");
        assert!(!got.mirrors("fork-a", "acme@0"));
        assert!(got.skip("fork-b", "acme@0").is_none());
    }

    /// A stopped local lane a box lane is in use instead of frees all its names, so an
    /// unchosen box lane that only clashes with it is mirrored.
    #[test]
    fn a_yielded_local_lane_holds_no_names() {
        let got = assign(
            &[local(
                "acme@0",
                false,
                &["web.acme.localhost", "api.acme.localhost"],
            )],
            &[
                on_box("fork-a", 1, &[("acme@0", WEB)]),
                on_box("fork-b", 2, &[("shop@0", &["api.acme.localhost"])]),
            ],
            &chose(&[("acme@0", "fork-a")]),
        );
        assert!(got.mirrors("fork-a", "acme@0"));
        assert!(got.mirrors("fork-b", "shop@0"));
        assert_eq!(
            got.yielded.get("acme@0").map(String::as_str),
            Some("fork-a")
        );
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
            ("*.acme.localhost", "web.acme2.localhost"),
            ("*.acme.localhost", "*.acme2.localhost"),
            ("*.a.localhost", "*.ba.localhost"),
        ] {
            assert!(!overlap(a, b), "{a} and {b}");
            assert!(!overlap(b, a), "{b} and {a}");
        }
    }
}
