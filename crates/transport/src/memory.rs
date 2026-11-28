//! In-process transport and peer endpoints for tests, with fault injection.
//!
//! Lives here rather than in `testing` so the transport's own tests and
//! downstream mirroring tests can use it without a dependency cycle.

use std::collections::{HashMap, HashSet};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::mpsc;

use crate::mux::{mux_client, mux_server};
use crate::peer::{Admission, Alpn, PeerAddr, PeerEndpoint, PeerError, PeerId, PeerIncoming};
use crate::transport::{BoxConnection, BoxServerConnection, Transport, TransportError};

/// Buffer per direction; large enough that yamux frames rarely block.
const BUFFER: usize = 256 * 1024;

/// Creates a connected transport/listener pair.
pub fn memory_transport() -> (MemoryTransport, MemoryListener) {
    let (accept_tx, accept_rx) = mpsc::unbounded_channel();
    let transport = MemoryTransport {
        accept_tx,
        state: Arc::new(Mutex::new(TransportState::default())),
    };
    (transport, MemoryListener { accept_rx })
}

#[derive(Default)]
struct TransportState {
    refuse: Option<bool>,
    links: Vec<LinkControl>,
}

/// Client end: each `connect` creates a fresh in-memory link.
#[derive(Clone)]
pub struct MemoryTransport {
    accept_tx: mpsc::UnboundedSender<MemoryStream>,
    state: Arc<Mutex<TransportState>>,
}

impl MemoryTransport {
    /// Makes later connects fail; `permanent` selects the error class.
    pub fn refuse(&self, permanent: bool) {
        self.lock().refuse = Some(permanent);
    }

    /// Lets connects succeed again.
    pub fn accept(&self) {
        self.lock().refuse = None;
    }

    /// Number of successful connects so far.
    pub fn connections(&self) -> usize {
        self.lock().links.len()
    }

    /// Fault controls for the most recent link.
    pub fn last_link(&self) -> Option<LinkControl> {
        self.lock().links.last().cloned()
    }

    /// Locks state; poisoning only happens if a test panicked mid-update.
    fn lock(&self) -> std::sync::MutexGuard<'_, TransportState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Creates a fresh link and returns its client end as a raw pipe, for
    /// tests that put their own wrapper between the pipe and the mux.
    /// [`Transport::connect`] runs yamux over the same pipe.
    pub fn connect_pipe(&self) -> Result<MemoryStream, TransportError> {
        match self.lock().refuse {
            Some(true) => return Err(TransportError::permanent("memory transport refused")),
            Some(false) => return Err(TransportError::transient("memory transport refused")),
            None => {}
        }
        let (client, server, gate) = link();
        self.accept_tx
            .send(server)
            .map_err(|_| TransportError::transient("memory listener closed"))?;
        self.lock().links.push(LinkControl { gate });
        Ok(client)
    }
}

/// A fresh in-memory link: its client end, its box end and their shared gate.
fn link() -> (MemoryStream, MemoryStream, Arc<Mutex<Gate>>) {
    let (client, server) = tokio::io::duplex(BUFFER);
    let gate = Arc::new(Mutex::new(Gate::default()));
    let client = MemoryStream {
        inner: client,
        gate: gate.clone(),
        end: 0,
    };
    let server = MemoryStream {
        inner: server,
        gate: gate.clone(),
        end: 1,
    };
    (client, server, gate)
}

#[async_trait]
impl Transport for MemoryTransport {
    /// yamux over a fresh link from [`MemoryTransport::connect_pipe`].
    async fn connect(&self) -> Result<BoxConnection, TransportError> {
        Ok(mux_client(Box::new(self.connect_pipe()?)))
    }
}

/// Box end: yields the server side of each link.
pub struct MemoryListener {
    accept_rx: mpsc::UnboundedReceiver<MemoryStream>,
}

impl MemoryListener {
    /// Waits for the next link and runs yamux over its box end; `None` once
    /// every transport clone is dropped.
    pub async fn accept(&mut self) -> Option<BoxServerConnection> {
        let pipe = self.accept_rx.recv().await?;
        Some(mux_server(Box::new(pipe)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Open,
    Frozen,
    Severed,
    /// Broken by the box refusing the client's key ([`PeerEndpoint::close_peer`]),
    /// so the client's failure reason says so as it does over iroh.
    Refused,
}

/// Link state plus parked tasks, indexed by `[end][read=0|write=1]`.
#[derive(Default)]
struct Gate {
    mode: Mode,
    wakers: [[Option<Waker>; 2]; 2],
}

impl Gate {
    /// Records the task so a mode change can wake it, whatever it is waiting on.
    fn park(&mut self, end: usize, direction: usize, waker: &Waker) {
        let slot = &mut self.wakers[end][direction];
        if !slot.as_ref().is_some_and(|w| w.will_wake(waker)) {
            *slot = Some(waker.clone());
        }
    }

    fn set(&mut self, mode: Mode) {
        self.mode = mode;
        for waker in self.wakers.iter_mut().flatten().filter_map(Option::take) {
            waker.wake();
        }
    }
}

/// Fault injection for one link.
#[derive(Clone)]
pub struct LinkControl {
    gate: Arc<Mutex<Gate>>,
}

impl LinkControl {
    /// Stops all traffic without closing, like a dead network path.
    pub fn freeze(&self) {
        self.lock().set(Mode::Frozen);
    }

    /// Resumes traffic after [`LinkControl::freeze`].
    pub fn thaw(&self) {
        self.lock().set(Mode::Open);
    }

    /// Breaks the link; both ends see connection resets.
    pub fn sever(&self) {
        self.lock().set(Mode::Severed);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One end of an in-memory link.
pub struct MemoryStream {
    inner: DuplexStream,
    gate: Arc<Mutex<Gate>>,
    end: usize,
}

impl MemoryStream {
    /// Checks the gate before an operation; `Some` short-circuits it.
    fn check<T>(&self, direction: usize, cx: &Context<'_>) -> Option<Poll<io::Result<T>>> {
        let mut gate = self.gate.lock().unwrap_or_else(PoisonError::into_inner);
        gate.park(self.end, direction, cx.waker());
        match gate.mode {
            Mode::Open => None,
            Mode::Frozen => Some(Poll::Pending),
            Mode::Severed => Some(Poll::Ready(Err(io::Error::from(
                io::ErrorKind::ConnectionReset,
            )))),
            Mode::Refused => Some(Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                PeerError::Refused.to_string(),
            )))),
        }
    }
}

impl AsyncRead for MemoryStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Some(poll) = self.check(0, cx) {
            return poll;
        }
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for MemoryStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(poll) = self.check(1, cx) {
            return poll;
        }
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(poll) = self.check(1, cx) {
            return poll;
        }
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(poll) = self.check(1, cx) {
            return poll;
        }
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Numbers the ids of memory endpoints, so endpoints of separate hubs in one
/// test process never share an id and a test's ids do not change between runs.
static NEXT_PEER: AtomicU64 = AtomicU64::new(1);

/// Locks a mutex; poisoning only happens if a test panicked mid-update.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// In-memory peer endpoints that reach each other through one shared hub,
/// with yamux over memory links as their connections. Uses no sockets and
/// no clock.
#[derive(Clone, Default)]
pub struct MemoryPeers {
    hub: Arc<Mutex<Hub>>,
}

/// The endpoints a hub knows and the ones dials cannot reach directly.
#[derive(Default)]
struct Hub {
    peers: HashMap<PeerId, Weak<MemoryPeer>>,
    no_direct_path: HashSet<PeerId>,
}

impl MemoryPeers {
    /// An empty hub.
    pub fn new() -> Self {
        Self::default()
    }

    /// A new endpoint on this hub with an id no other endpoint has.
    pub fn endpoint(&self) -> Arc<MemoryPeer> {
        let number = NEXT_PEER.fetch_add(1, Ordering::Relaxed);
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&number.to_be_bytes());
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let peer = Arc::new(MemoryPeer {
            id: PeerId(id),
            hub: self.hub.clone(),
            state: Mutex::new(PeerState {
                admission: None,
                incoming: Some(incoming_tx),
                links: Vec::new(),
            }),
            incoming: tokio::sync::Mutex::new(incoming),
        });
        locked(&self.hub)
            .peers
            .insert(peer.id, Arc::downgrade(&peer));
        peer
    }

    /// With `false`, dials to `id` fail with [`PeerError::NoDirectPath`], as
    /// when hole punching finds no path to a box; `true` lets them through
    /// again.
    pub fn set_reachable(&self, id: PeerId, direct: bool) {
        let mut hub = locked(&self.hub);
        if direct {
            hub.no_direct_path.remove(&id);
        } else {
            hub.no_direct_path.insert(id);
        }
    }
}

/// One endpoint of a [`MemoryPeers`] hub.
pub struct MemoryPeer {
    id: PeerId,
    hub: Arc<Mutex<Hub>>,
    state: Mutex<PeerState>,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<PeerIncoming>>,
}

/// What a memory endpoint admits and the links it is part of.
struct PeerState {
    admission: Option<Arc<dyn Admission>>,
    /// Where admitted connections queue for `accept`; `None` once closed.
    incoming: Option<mpsc::UnboundedSender<PeerIncoming>>,
    links: Vec<PeerLink>,
}

/// One link a memory endpoint is part of. Weak, so a link both ends dropped
/// is forgotten at the next dial.
struct PeerLink {
    remote: PeerId,
    /// Whether the other peer dialed this endpoint, which `close_peer` ends.
    incoming: bool,
    gate: Weak<Mutex<Gate>>,
}

impl MemoryPeer {
    /// Whether the endpoint has an admission, as after `listen(Some(..))`.
    pub fn is_listening(&self) -> bool {
        self.state().admission.is_some()
    }

    /// Locks the endpoint's state.
    fn state(&self) -> MutexGuard<'_, PeerState> {
        locked(&self.state)
    }

    /// Records a link to `remote`, dropping links both ends have let go.
    fn record(&self, remote: PeerId, incoming: bool, gate: &Arc<Mutex<Gate>>) {
        let mut state = self.state();
        state.links.retain(|link| link.gate.strong_count() > 0);
        state.links.push(PeerLink {
            remote,
            incoming,
            gate: Arc::downgrade(gate),
        });
    }
}

#[async_trait]
impl PeerEndpoint for MemoryPeer {
    /// The id the hub gave this endpoint.
    async fn id(&self) -> Result<PeerId, PeerError> {
        Ok(self.id)
    }

    /// The id alone: memory endpoints have no relay and no addresses.
    async fn addr(&self) -> Result<PeerAddr, PeerError> {
        Ok(PeerAddr {
            id: self.id,
            relay: None,
            direct: Vec::new(),
        })
    }

    /// Asks the target's admission, then links the two endpoints, in the
    /// order iroh fails: a box that is not listening refuses the handshake,
    /// a refused key closes the connection before the direct-path wait ends,
    /// and only an admitted dial can time out without a direct path. The
    /// timeout takes no time here.
    async fn connect(&self, addr: &PeerAddr, alpn: Alpn) -> Result<BoxConnection, TransportError> {
        if self.state().incoming.is_none() {
            return Err(PeerError::Closed.into());
        }
        let (target, direct) = {
            let hub = locked(&self.hub);
            let target = hub.peers.get(&addr.id).and_then(Weak::upgrade);
            (target, !hub.no_direct_path.contains(&addr.id))
        };
        let unreachable = || PeerError::Unreachable("no endpoint with that id is listening".into());
        let target = target.ok_or_else(unreachable)?;
        let (admission, incoming) = {
            let state = target.state();
            (state.admission.clone(), state.incoming.clone())
        };
        let (Some(admission), Some(incoming)) = (admission, incoming) else {
            return Err(unreachable().into());
        };
        if !admission.admit(&self.id, alpn) {
            return Err(PeerError::Refused.into());
        }
        if !direct {
            return Err(PeerError::NoDirectPath {
                waited: Duration::ZERO,
            }
            .into());
        }
        let (client, server, gate) = link();
        let arrived = PeerIncoming {
            peer: self.id,
            alpn,
            connection: mux_server(Box::new(server)),
        };
        if incoming.send(arrived).is_err() {
            return Err(unreachable().into());
        }
        self.record(target.id, false, &gate);
        target.record(self.id, true, &gate);
        Ok(mux_client(Box::new(client)))
    }

    /// Sets the admission new dials are checked against.
    async fn listen(&self, admission: Option<Arc<dyn Admission>>) -> Result<(), PeerError> {
        let mut state = self.state();
        if state.incoming.is_none() {
            return Err(PeerError::Closed);
        }
        state.admission = admission;
        Ok(())
    }

    /// The next admitted connection, in the order they were dialed.
    async fn accept(&self) -> Option<PeerIncoming> {
        self.incoming.lock().await.recv().await
    }

    /// Breaks the links `peer` dialed, with the refused-key reason.
    fn close_peer(&self, peer: &PeerId) {
        for link in &self.state().links {
            if link.incoming
                && link.remote == *peer
                && let Some(gate) = link.gate.upgrade()
            {
                locked(&gate).set(Mode::Refused);
            }
        }
    }

    /// Breaks every link, refuses new dials either way and ends `accept`
    /// once the queued connections are taken.
    async fn close(&self) {
        let links = {
            let mut state = self.state();
            state.admission = None;
            state.incoming = None;
            std::mem::take(&mut state.links)
        };
        for gate in links.iter().filter_map(|link| link.gate.upgrade()) {
            locked(&gate).set(Mode::Severed);
        }
        locked(&self.hub).peers.remove(&self.id);
    }
}
