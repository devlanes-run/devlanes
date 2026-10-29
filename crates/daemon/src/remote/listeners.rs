//! Local listeners for box ports, shared by every box's mirror. See
//! [`Listeners`].

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex, Weak},
};

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::{
    HostConn,
    assign::{Assignment, PortOwner},
    mirror::{Bound, accept_loop, bind, unbind},
};
use crate::{engine::Inner, model::lock};

/// The listeners this daemon holds for box ports: one per local port number,
/// bound on the mirror addresses, and one relay per box lane and port, on an
/// ephemeral `127.0.0.1` port the proxy dials when it cannot use the number
/// itself.
///
/// A listener belongs to the daemon, not to the box whose mirror bound it.
/// Each has a [`Target`]: the box lane its new connections go to, taken from
/// the assignment's `listeners` and `relays`. When the assignment moves a
/// lane to another box, [`Listeners::apply`] repoints every listener and
/// relay of the lane in one step and closes the connections to the old box.
/// One box serves a lane reference at a time, which the assignment
/// guarantees, so a relay keyed by lane and port has one owner.
/// Nothing is unbound, so no other program can take the port meanwhile and
/// no two boxes ever race to bind it. Routes name the same local port before
/// and after, so the route table only changes which box it names. When the
/// new box runs the lane on other port numbers, its routes name its own
/// relay, and the old relay keeps leading to the old box, each connection
/// closed once answered: a request the proxy sends before it has the new
/// routes still gets an answer.
///
/// A box's mirror binds a listener when its lane is the owner and the port is
/// free, and asks for a relay for each route. It unbinds a listener only
/// when the port is no longer wanted and no other box lane owns it
/// ([`Listeners::release`]), when the port is held elsewhere on this machine,
/// or when a local lane takes the port back. A listener whose port has no
/// owner left is retired like a relay and unbound by the next reconcile
/// ([`Listeners::sweep`]). A relay stays while any box's routes use it.
///
/// Targets are only set under the remote assignment lock, from the current
/// assignment. A mirror that reconciles with an older assignment can bind or
/// release, but never point a listener at a box that lost its lane.
#[derive(Default)]
pub(crate) struct Listeners {
    ports: Mutex<BTreeMap<u16, PortSlot>>,
    relays: Mutex<BTreeMap<(String, u16), RelaySlot>>,
}

/// One local port number and the accept loops on its addresses.
struct PortSlot {
    bound: Vec<Bound>,
    target: Arc<Target>,
}

/// One relay: its ephemeral port, its accept loop and the boxes whose routes
/// use it, by connect serial.
struct RelaySlot {
    local: u16,
    _bound: Bound,
    target: Arc<Target>,
    users: BTreeSet<u64>,
}

/// Where the connections a listener accepts go now.
#[derive(Default)]
pub(super) struct Target(Mutex<Option<Owner>>);

/// A box lane a listener leads to, with the token that closes the connections
/// accepted for it.
#[derive(Clone)]
pub(super) struct Owner {
    /// Weak, so a retired listener keeps no disconnected box alive.
    pub conn: Weak<HostConn>,
    /// The box's connect serial and name.
    pub serial: u64,
    pub host: String,
    pub lane: String,
    pub unit: String,
    pub conns: CancellationToken,
}

impl Owner {
    /// A target for `lane`'s `unit` on `conn`.
    pub(super) fn new(conn: &Arc<HostConn>, lane: String, unit: String) -> Self {
        Self {
            conn: Arc::downgrade(conn),
            serial: conn.serial,
            host: conn.name.clone(),
            lane,
            unit,
            conns: CancellationToken::new(),
        }
    }

    /// Whether `self` and `other` are the same box lane.
    fn same(&self, other: &Owner) -> bool {
        self.serial == other.serial && self.lane == other.lane
    }

    /// Whether the assignment no longer names it ([`Target::retire`]).
    fn retired(&self) -> bool {
        self.conns.is_cancelled()
    }
}

impl Target {
    /// The box lane new connections go to, if any.
    pub(super) fn current(&self) -> Option<Owner> {
        lock(&self.0).clone()
    }

    /// Points at `owner`. A different box lane than before closes the
    /// connections accepted for the old one. The same one keeps them.
    fn point(&self, owner: Option<Owner>) {
        let mut current = lock(&self.0);
        match (current.as_mut(), owner) {
            (Some(now), Some(next)) if now.same(&next) && !now.retired() => {
                now.unit = next.unit;
            }
            (_, next) => {
                if let Some(old) = current.take() {
                    old.conns.cancel();
                }
                *current = next;
            }
        }
    }

    /// Closes the connections to the current box lane, but keeps leading new
    /// ones there, each closed once answered, until the listener goes. For a
    /// lane that went to a box using other port numbers: a request sent
    /// before the proxy or a client has the new numbers still gets an
    /// answer, from the old box.
    fn retire(&self) {
        if let Some(now) = lock(&self.0).as_ref() {
            now.conns.cancel();
        }
    }

    /// The connect serial of the box it leads to, unless retired.
    fn serial(&self) -> Option<u64> {
        lock(&self.0)
            .as_ref()
            .filter(|owner| !owner.retired())
            .map(|owner| owner.serial)
    }
}

/// Finds the live connection of a [`PortOwner`] and makes it a target.
pub(super) type Resolve<'a> = &'a dyn Fn(&PortOwner) -> Option<Owner>;

impl Listeners {
    /// Points every listener and relay at its owner in `assignment`. Called
    /// with the assignment lock held, right after the assignment changed.
    pub(super) fn apply(&self, assignment: &Assignment, resolve: Resolve<'_>) {
        // Only lanes that moved to a box with other port numbers, or that no
        // box serves now, find no owner. Forks of one lane share the numbers
        // and are repointed. A retired port goes with the next sweep.
        for (port, slot) in lock(&self.ports).iter() {
            match assignment.listeners.get(port).and_then(resolve) {
                Some(owner) => slot.target.point(Some(owner)),
                None => slot.target.retire(),
            }
        }
        for (key, slot) in lock(&self.relays).iter() {
            match assignment.relays.get(key).and_then(resolve) {
                Some(owner) => slot.target.point(Some(owner)),
                None => slot.target.retire(),
            }
        }
    }

    /// Serves `listener`, bound on `address` for box port `port`. The first
    /// listener of a port leads to `owner`, read from the current assignment
    /// with its lock held. Later ones share the port's target.
    pub(super) fn listen(
        &self,
        port: u16,
        address: IpAddr,
        listener: TcpListener,
        owner: Option<Owner>,
        engine: Weak<Inner>,
    ) {
        let mut ports = lock(&self.ports);
        let slot = ports.entry(port).or_insert_with(|| {
            let target = Arc::new(Target::default());
            target.point(owner);
            PortSlot {
                bound: Vec::new(),
                target,
            }
        });
        slot.bound.push(Bound {
            address,
            task: tokio::spawn(accept_loop(listener, port, slot.target.clone(), engine)),
        });
    }

    /// The addresses `port` is bound on.
    pub(super) fn addresses(&self, port: u16) -> Vec<IpAddr> {
        lock(&self.ports)
            .get(&port)
            .map(|slot| slot.bound.iter().map(|bound| bound.address).collect())
            .unwrap_or_default()
    }

    /// Whether `127.0.0.1:<port>`, which the proxy dials first, leads to a box
    /// running lane `lane`: on every fork of the lane, a route can name the
    /// port itself and stay the same across a switch.
    pub(super) fn serves_v4(&self, port: u16, lane: &str) -> bool {
        lock(&self.ports).get(&port).is_some_and(|slot| {
            slot.bound
                .iter()
                .any(|bound| bound.address == IpAddr::V4(Ipv4Addr::LOCALHOST))
                && slot
                    .target
                    .current()
                    .is_some_and(|owner| owner.lane == lane && !owner.retired())
        })
    }

    /// Unbinds `port` whoever it leads to, and waits until it is closed.
    pub(super) async fn remove(&self, port: u16) {
        let slot = lock(&self.ports).remove(&port);
        if let Some(slot) = slot {
            slot.target.point(None);
            unbind(slot.bound).await;
        }
    }

    /// Unbinds `port` unless it leads to another box than the one with
    /// connect serial `serial`, which then keeps it.
    pub(super) async fn release(&self, port: u16, serial: u64) {
        let slot = {
            let mut ports = lock(&self.ports);
            let mine = ports
                .get(&port)
                .is_some_and(|slot| slot.target.serial().is_none_or(|owner| owner == serial));
            if mine { ports.remove(&port) } else { None }
        };
        if let Some(slot) = slot {
            slot.target.point(None);
            unbind(slot.bound).await;
        }
    }

    /// Unbinds every port that leads nowhere since the last assignment.
    pub(super) async fn sweep(&self) {
        let slots: Vec<PortSlot> = {
            let mut ports = lock(&self.ports);
            let idle: Vec<u16> = ports
                .iter()
                .filter(|(_, slot)| slot.target.serial().is_none())
                .map(|(port, _)| *port)
                .collect();
            idle.iter().filter_map(|port| ports.remove(port)).collect()
        };
        for slot in slots {
            unbind(slot.bound).await;
        }
    }

    /// The bound ports that lead to the box with connect serial `serial`,
    /// each with its unit as `project@lane:unit`.
    pub(super) fn bound_by(&self, serial: u64) -> BTreeMap<u16, String> {
        lock(&self.ports)
            .iter()
            .filter(|(_, slot)| !slot.bound.is_empty())
            .filter_map(|(port, slot)| {
                let owner = slot.target.current()?;
                (owner.serial == serial && !owner.retired())
                    .then(|| (*port, format!("{}:{}", owner.lane, owner.unit)))
            })
            .collect()
    }

    /// The first of `ports` still bound, with the box and unit it leads to.
    pub(super) fn holder(&self, ports: &BTreeSet<u16>) -> Option<(u16, Option<(String, String)>)> {
        lock(&self.ports)
            .iter()
            .find(|(port, slot)| ports.contains(*port) && !slot.bound.is_empty())
            .map(|(port, slot)| {
                let owner = slot
                    .target
                    .current()
                    .map(|owner| (owner.host, format!("{}:{}", owner.lane, owner.unit)));
                (*port, owner)
            })
    }

    /// The local port of the relay for box port `port` of `lane`, binding one
    /// that leads to `owner` on first use, and records that the box with
    /// connect serial `serial` uses it.
    pub(super) fn relay(
        &self,
        lane: &str,
        port: u16,
        serial: u64,
        owner: Option<Owner>,
        engine: Weak<Inner>,
    ) -> std::io::Result<u16> {
        let mut relays = lock(&self.relays);
        let key = (lane.to_owned(), port);
        if let Some(slot) = relays.get_mut(&key) {
            slot.users.insert(serial);
            return Ok(slot.local);
        }
        let address = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let listener = bind(address, 0)?;
        let local = listener.local_addr()?.port();
        let target = Arc::new(Target::default());
        target.point(owner);
        let task = tokio::spawn(accept_loop(listener, port, target.clone(), engine));
        relays.insert(
            key,
            RelaySlot {
                local,
                _bound: Bound { address, task },
                target,
                users: BTreeSet::from([serial]),
            },
        );
        Ok(local)
    }

    /// The local port of the relay for box port `port` of `lane`, if bound.
    pub(super) fn relay_port(&self, lane: &str, port: u16) -> Option<u16> {
        lock(&self.relays)
            .get(&(lane.to_owned(), port))
            .map(|slot| slot.local)
    }

    /// Records that the box with connect serial `serial` uses only the relays
    /// in `used`, and drops the relays nobody uses.
    pub(super) fn keep_relays(&self, serial: u64, used: &BTreeSet<(String, u16)>) {
        lock(&self.relays).retain(|key, slot| {
            if !used.contains(key) {
                slot.users.remove(&serial);
            }
            let keep = !slot.users.is_empty();
            if !keep {
                slot.target.point(None);
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listener on an ephemeral `127.0.0.1` port.
    async fn listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("listener");
        let port = listener.local_addr().expect("address").port();
        (listener, port)
    }

    /// An owner of `lane` on `conn`.
    fn owner(conn: &Arc<HostConn>, lane: &str) -> Owner {
        Owner::new(conn, lane.into(), "web".into())
    }

    /// Repointing a listener keeps it bound and closes only the old owner's
    /// connections. The same owner again keeps them, and a port without an
    /// owner is retired, then swept.
    #[tokio::test]
    async fn a_switch_repoints_without_unbinding() {
        let (_state, receiver) =
            tokio::sync::watch::channel(devlanes_transport::LinkState::Stopped);
        let fork_a = super::super::tests::conn_named("fork-a", 1, receiver.clone());
        let fork_b = super::super::tests::conn_named("fork-b", 2, receiver);
        let listeners = Listeners::default();
        let (socket, port) = listener().await;
        let first = owner(&fork_a, "acme@0");
        let old = first.conns.clone();
        listeners.listen(
            port,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            socket,
            Some(first),
            Weak::new(),
        );
        assert!(listeners.serves_v4(port, "acme@0"));
        assert_eq!(
            listeners.bound_by(1).keys().copied().collect::<Vec<_>>(),
            [port]
        );

        let mut assignment = Assignment::default();
        let to = |host: &str| PortOwner {
            host: host.into(),
            lane: "acme@0".into(),
            unit: "web".into(),
        };
        assignment.listeners.insert(port, to("fork-a"));
        let resolve = |wanted: &PortOwner| {
            let conn = if wanted.host == "fork-a" {
                &fork_a
            } else {
                &fork_b
            };
            Some(owner(conn, &wanted.lane))
        };
        listeners.apply(&assignment, &resolve);
        assert!(!old.is_cancelled(), "the same owner keeps its connections");

        assignment.listeners.insert(port, to("fork-b"));
        listeners.apply(&assignment, &resolve);
        assert!(old.is_cancelled(), "fork-a's connections close");
        assert_eq!(listeners.addresses(port).len(), 1, "still bound");
        assert!(listeners.bound_by(1).is_empty());
        assert_eq!(
            listeners.bound_by(2).keys().copied().collect::<Vec<_>>(),
            [port]
        );
        assert!(
            tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok(),
            "the port accepts across the switch"
        );

        // fork-a no longer wants it, but it leads to fork-b, which keeps it.
        listeners.release(port, 1).await;
        assert_eq!(listeners.addresses(port).len(), 1);

        // Without an owner it is retired: fork-b's connections close, the
        // port stays bound until the next sweep, and it keeps fork-b alive
        // no longer than its connection.
        let kept = listeners
            .ports
            .lock()
            .ok()
            .and_then(|ports| ports.get(&port).and_then(|slot| slot.target.current()))
            .expect("fork-b owns it");
        listeners.apply(&Assignment::default(), &resolve);
        assert!(kept.conns.is_cancelled());
        assert_eq!(
            listeners.addresses(port).len(),
            1,
            "a retired port stays bound"
        );
        assert!(listeners.bound_by(2).is_empty());
        assert!(!listeners.serves_v4(port, "acme@0"));
        drop(fork_b);
        let retired = listeners
            .ports
            .lock()
            .ok()
            .and_then(|ports| ports.get(&port).and_then(|slot| slot.target.current()))
            .expect("still retired");
        assert!(
            retired.conn.upgrade().is_none(),
            "keeps no disconnected box"
        );
        listeners.sweep().await;
        assert!(listeners.addresses(port).is_empty(), "nobody owns it");
    }

    /// A relay belongs to its lane and port: every box that routes the lane
    /// shares it, a switch keeps its port, and it goes with its last user.
    #[tokio::test]
    async fn relays_are_shared_by_the_forks_of_a_lane() {
        let (_state, receiver) =
            tokio::sync::watch::channel(devlanes_transport::LinkState::Stopped);
        let fork_a = super::super::tests::conn_named("fork-a", 1, receiver.clone());
        let fork_b = super::super::tests::conn_named("fork-b", 2, receiver);
        let listeners = Listeners::default();
        let a = owner(&fork_a, "acme@0");
        let closes = a.conns.clone();
        let local = listeners
            .relay("acme@0", 3000, 1, Some(a), Weak::new())
            .expect("relay");
        let shared = listeners
            .relay("acme@0", 3000, 2, None, Weak::new())
            .expect("relay");
        assert_eq!(local, shared);
        let other = listeners
            .relay("shop@0", 3000, 2, None, Weak::new())
            .expect("relay");
        assert_ne!(local, other, "another lane gets its own relay");

        let mut assignment = Assignment::default();
        assignment.relays.insert(
            ("acme@0".into(), 3000),
            PortOwner {
                host: "fork-b".into(),
                lane: "acme@0".into(),
                unit: "web".into(),
            },
        );
        listeners.apply(&assignment, &|wanted: &PortOwner| {
            (wanted.host == "fork-b").then(|| owner(&fork_b, &wanted.lane))
        });
        assert!(closes.is_cancelled());
        assert_eq!(listeners.relay_port("acme@0", 3000), Some(local));

        listeners.keep_relays(1, &BTreeSet::new());
        assert_eq!(
            listeners.relay_port("acme@0", 3000),
            Some(local),
            "fork-b uses it"
        );
        listeners.keep_relays(2, &BTreeSet::new());
        assert_eq!(listeners.relay_port("acme@0", 3000), None);
        assert_eq!(listeners.relay_port("shop@0", 3000), None);
    }
}
