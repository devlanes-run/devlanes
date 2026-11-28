//! Peer-to-peer connections between daemons: the endpoint a daemon dials and
//! listens with, and what it needs to know about the peers on the other side.
//!
//! The daemon reaches boxes through [`PeerEndpoint`] without knowing the
//! network library behind it; `devlanes-transport-iroh` implements it over
//! iroh, and [`crate::memory::MemoryPeers`] in memory for tests.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::transport::{BoxConnection, BoxServerConnection, TransportError};

/// ALPN of a session between paired peers: one QUIC stream per protocol
/// stream. The wire protocol version is checked in the hello, as over SSH,
/// so a box on another protocol version still answers with which side is
/// behind instead of failing the handshake.
pub const TUNNEL_ALPN: &[u8] = b"devlanes/tunnel/1";

/// ALPN of the one-stream pairing exchange.
pub const PAIR_ALPN: &[u8] = b"devlanes/pair/1";

/// The public key that names a peer (an iroh endpoint id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    /// The first 8 hex digits, for logs and lists where the full key is noise.
    pub fn short(&self) -> String {
        let mut text = self.to_string();
        text.truncate(8);
        text
    }
}

impl fmt::Display for PeerId {
    /// 64 lowercase hex digits, the form `FromStr` reads back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for PeerId {
    /// The short form, so a key in a debug dump stays readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", self.short())
    }
}

impl FromStr for PeerId {
    type Err = PeerIdError;

    /// Reads 64 hex digits in either case.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return Err(PeerIdError);
        }
        // Digit by digit: `u8::from_str_radix` would also take a leading `+`.
        let digit = |byte: u8| char::from(byte).to_digit(16).ok_or(PeerIdError);
        let mut id = [0u8; 32];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, [high, low]) in id.iter_mut().zip(pairs) {
            *byte = u8::try_from(digit(*high)? << 4 | digit(*low)?).map_err(|_| PeerIdError)?;
        }
        Ok(Self(id))
    }
}

/// A peer id that is not 64 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a peer id is 64 hex digits")]
pub struct PeerIdError;

/// Where a peer may be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddr {
    /// The peer's key.
    pub id: PeerId,
    /// The relay the peer was on, such as `https://relay1.devlanes.run`. A
    /// dial uses it only when it is one of the dialing endpoint's own
    /// relays: it comes from a ticket someone pasted, so it must not make
    /// this machine contact a host of the ticket's choosing.
    pub relay: Option<String>,
    /// IP addresses the peer reported, tried directly.
    pub direct: Vec<SocketAddr>,
}

/// What an incoming connection is for, from its ALPN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alpn {
    /// A session over [`TUNNEL_ALPN`].
    Tunnel,
    /// A pairing exchange over [`PAIR_ALPN`].
    Pair,
}

impl Alpn {
    /// The ALPN bytes sent in the handshake.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Tunnel => TUNNEL_ALPN,
            Self::Pair => PAIR_ALPN,
        }
    }

    /// The protocol `bytes` names, or `None` for one devlanes does not speak.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        [Self::Tunnel, Self::Pair]
            .into_iter()
            .find(|alpn| alpn.bytes() == bytes)
    }
}

/// Decides during the handshake, before any stream, which peers are served.
pub trait Admission: Send + Sync {
    /// Whether `peer` may connect for `alpn`. Runs on the accept path, so it
    /// only reads memory: no I/O, no waiting.
    fn admit(&self, peer: &PeerId, alpn: Alpn) -> bool;
}

/// A connection another peer opened and the admission let in.
pub struct PeerIncoming {
    /// The key the peer proved in the handshake.
    pub peer: PeerId,
    /// What the peer connected for.
    pub alpn: Alpn,
    /// The box's side of the connection.
    pub connection: BoxServerConnection,
}

/// This machine's peer-to-peer endpoint, one per daemon, for dialing and
/// listening. Nothing binds a socket or contacts a relay until `addr`,
/// `connect` or `listen(Some(..))` needs it.
///
/// Every method but `accept` may be called concurrently. After `close`,
/// `addr`, `connect` and `listen` fail with [`PeerError::Closed`], and `id`
/// keeps working.
#[async_trait]
pub trait PeerEndpoint: Send + Sync {
    /// This machine's peer id. Creates the key file the first time; never binds.
    async fn id(&self) -> Result<PeerId, PeerError>;

    /// Where other peers reach this one, for a ticket. Binds when needed and
    /// waits, bounded, for the relay connection and for the first local
    /// addresses.
    async fn addr(&self) -> Result<PeerAddr, PeerError>;

    /// Dials `addr` for `alpn` and returns the connection once a direct path
    /// is selected. A box that refuses this machine's key is a permanent
    /// error; no direct path in time and an unreachable box are transient.
    /// The cause is a [`PeerError`], reachable with [`TransportError::downcast_ref`].
    async fn connect(&self, addr: &PeerAddr, alpn: Alpn) -> Result<BoxConnection, TransportError>;

    /// With `Some`, accepts connections the admission lets in and publishes
    /// this endpoint's relay for lookup by id; a later `Some` replaces the
    /// admission. With `None`, refuses every new connection and stops
    /// publishing; on an endpoint that never bound it binds nothing. Open
    /// connections stay either way.
    async fn listen(&self, admission: Option<Arc<dyn Admission>>) -> Result<(), PeerError>;

    /// The next admitted connection. Waits while not listening; `None` once
    /// the endpoint is closed. One caller at a time. An implementation may
    /// drop admitted connections while too many wait here.
    async fn accept(&self) -> Option<PeerIncoming>;

    /// Closes every session (`Alpn::Tunnel`) connection `peer` opened to this
    /// endpoint with the refused-key code, so its client shows that reason.
    /// The caller removes `peer` from the admission first: then, while the
    /// endpoint keeps listening, its next dial is refused as a permanent
    /// error, and a connection in the middle of its handshake is closed too.
    fn close_peer(&self, peer: &PeerId);

    /// Closes the endpoint and its connections gracefully, for daemon
    /// shutdown: clients see a clean close, with no failure reason.
    async fn close(&self);
}

/// Why a peer operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PeerError {
    /// Connected, but no direct path was selected within the wait.
    #[error(
        "no direct connection to the box formed within {} seconds, and devlanes does not \
         relay data; connect over SSH or Tailscale instead",
        .waited.as_secs()
    )]
    NoDirectPath {
        /// How long the dial waited for a direct path.
        waited: Duration,
    },
    /// The box could not be reached at all: offline, or no relay or address works.
    #[error("the box is not reachable ({0}); check that its devlanes daemon is running")]
    Unreachable(String),
    /// The box refused this machine's key.
    #[error("the box does not accept this machine; pair it again with a new ticket")]
    Refused,
    /// The box's id is 32 bytes that are not a public key, so no dial can
    /// reach it: a damaged stored pairing, or a ticket from a faulty box,
    /// since a ticket's checksum catches typing mistakes.
    #[error("the box's id ({0}) is not a valid key; pair it again with a new ticket")]
    InvalidId(PeerId),
    /// The key file could not be read or written.
    #[error("peer-to-peer key {}: {source}", .path.display())]
    Key {
        /// The key file.
        path: PathBuf,
        /// What reading, writing or checking it failed with.
        #[source]
        source: std::io::Error,
    },
    /// The endpoint could not bind its sockets.
    #[error("starting peer-to-peer networking: {0}")]
    Bind(String),
    /// The endpoint was closed.
    #[error("peer-to-peer networking was stopped")]
    Closed,
}

impl From<PeerError> for TransportError {
    /// Classifies a peer failure for reconnect decisions: only a refused key,
    /// an invalid box id and an unusable key file need a person before
    /// another attempt helps.
    fn from(error: PeerError) -> Self {
        match error {
            PeerError::Refused | PeerError::InvalidId(_) | PeerError::Key { .. } => {
                Self::permanent(error)
            }
            PeerError::NoDirectPath { .. }
            | PeerError::Unreachable(_)
            | PeerError::Bind(_)
            | PeerError::Closed => Self::transient(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer id survives its text form, and text that is not one key is
    /// rejected instead of read as a different key.
    #[test]
    fn peer_ids_read_back_their_text_and_nothing_else() {
        let id = PeerId(std::array::from_fn(|index| (index * 7) as u8));
        let text = id.to_string();
        assert_eq!(text.parse::<PeerId>(), Ok(id));
        assert_eq!(text.to_uppercase().parse::<PeerId>(), Ok(id));
        let wrong = [
            text[..62].to_string(),
            format!("{text}00"),
            format!("{}zz", &text[..62]),
            format!("+{}", &text[1..]),
            String::new(),
        ];
        for bad in &wrong {
            assert_eq!(bad.parse::<PeerId>(), Err(PeerIdError), "{bad}");
        }
    }
}
