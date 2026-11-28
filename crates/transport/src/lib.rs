//! Transport-independent remote protocol between a client daemon and a box.
//!
//! A [`Transport`] yields one [`Connection`]; the box serves the matching
//! [`ServerConnection`]. On the box, `devlanes tunnel --stdio` first writes
//! [`TUNNEL_PREAMBLE`]; transports that run it read up to that line with
//! [`read_preamble`], then run yamux over their one byte pipe with
//! [`mux_client`], and the tunnel with [`mux_server`]. Only the client opens
//! streams, at most [`MAX_STREAMS`] at once:
//!
//! - one control stream: protocol check, features, client identity, box info,
//!   registry snapshot, registry events and pings (every 10 s, dead after 20 s
//!   without a pong), and with [`MIRROR_REPORT`] the client's reports of what
//!   it mirrors ([`Session::report`], [`RegistrySource::report`]);
//! - data streams to registered endpoints ([`Session::open`]);
//! - sync subscriptions ([`Session::subscribe_sync`]). With [`SYNC_ZSTD`], the
//!   box compresses file chunks and the client its input values where zstd
//!   makes them smaller, and the other end decompresses them, so sync sources
//!   and their clients only see file bytes and values.
//!
//! [`PROTOCOL_VERSION`] must match exactly on both ends and changes only for
//! breaking changes. Anything additive is a named feature instead: each end
//! lists the features it implements in the handshake ([`FEATURES`]), and a side
//! uses a feature only when the other side lists it too
//! ([`Session::has_feature`], [`ClientInfo::features`]).
//!
//! Before the welcome, the box's [`PairingGate`] decides whether it serves the
//! client: by its id and remote token, or after the client traded a one-time
//! code for a token ([`pairing`]). A client it does not serve learns nothing
//! about the box. A box that takes no connections at all answers every hello
//! with [`refuse`] instead, which the client reports as a permanent
//! [`SessionError::Refused`].
//!
//! The box side is [`serve`], driven by injected [`BoxServices`]. [`Link`]
//! reconnects with backoff and keeps one [`ClientRegistry`] across sessions.
//! It stops for a [`TransportError::Permanent`] or
//! [`TransportError::AuthFailed`] failure and waits for
//! [`Link::resume`] after a [`TransportError::NeedsAuth`] one or a session
//! that needs pairing ([`SessionError::is_needs_pairing`]).
//! [`PeerEndpoint`] is the peer-to-peer side: one endpoint per daemon that
//! dials boxes by key and accepts the clients its [`Admission`] lets in.
//!
//! [`memory`] provides an in-process transport with fault injection for tests.
//! With the `test-support` feature, `conformance` holds the checks every
//! connection type must pass, for transports in other crates.

#[cfg(feature = "test-support")]
pub mod conformance;
pub mod memory;
mod mux;
pub mod pairing;
mod peer;
mod preamble;
mod reconnect;
mod registry;
mod server;
mod session;
mod splice;
mod sync;
mod transport;
mod wire;

pub use mux::{mux_client, mux_server};
pub use pairing::{NoPairing, PairingCheck, PairingClaim, PairingClient, PairingGate};
pub use peer::{
    Admission, Alpn, PAIR_ALPN, PeerAddr, PeerEndpoint, PeerError, PeerId, PeerIdError,
    PeerIncoming, TUNNEL_ALPN,
};
pub use preamble::{PREAMBLE_LIMIT, Preamble, PreambleError, TUNNEL_PREAMBLE, read_preamble};
pub use reconnect::{Backoff, Link, LinkError, LinkState};
pub use registry::{
    AuthorizedTarget, ClientRegistry, HostHeader, HostInfo, LaneInfo, LaneKey, PortInfo, Registry,
    RegistryEvent, RegistryFeed, RegistrySnapshot, RegistrySource, RegistryUpdate, SharedRegistry,
    UnitInfo, UnitKey, UnitKind, serving,
};
pub use server::{
    BoxServices, ClientInfo, Dialer, LoopbackDialer, ServeError, ServeOptions, refuse, serve,
};
pub use session::{
    DataStream, MAX_STREAMS, OpenError, Session, SessionEnd, SessionError, SessionOptions, Timing,
};
pub use splice::{HALF_CLOSE_IDLE, splice};
pub use sync::{
    NoSync, SyncError, SyncFeed, SyncInput, SyncInputEvent, SyncMessage, SyncSource,
    SyncStreamError, SyncSubscription,
};
pub use transport::{
    BoxConnection, BoxDuplex, BoxError, BoxServerConnection, Connection, ConnectionError, Duplex,
    EXIT_DAEMON_STOPPED, Joined, ServerConnection, Transport, TransportError, stdio,
};
pub use wire::{
    BoxInfo, DialTarget, EXTERNAL_UNITS, FEATURES, MIRROR_REPORT, PROTOCOL_VERSION, PairingReason,
    Prefixed, RejectReason, SYNC_ZSTD, WireError,
};
