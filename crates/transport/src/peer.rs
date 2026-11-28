//! Peer-to-peer connections between daemons: the endpoint a daemon dials and
//! listens with, and what it needs to know about the peers on the other side.
//!
//! The daemon reaches boxes through [`PeerEndpoint`] without knowing the
//! network library behind it; `devlanes-transport-iroh` implements it over
//! iroh, and [`crate::memory::MemoryPeers`] in memory for tests. A
//! [`Ticket`] carries a box's address and a one-time secret to the client
//! that pairs with it.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

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
    /// The relay the peer was on, such as `https://relay1.devlanes.run`.
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

/// The secret a ticket carries: 16 random bytes the box keeps in memory
/// until the ticket is used or expires. `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq)]
pub struct TicketSecret(pub [u8; 16]);

impl fmt::Debug for TicketSecret {
    /// A placeholder, so a logged ticket never shows its secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TicketSecret(<redacted>)")
    }
}

impl Drop for TicketSecret {
    /// Zeroes the secret, so it does not stay in freed memory.
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// What a client needs to pair with a box once: where the box is and the
/// secret it hands out with `devlanes daemon pair`. The expiry is not in it;
/// the box keeps that with the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    /// The box's key and where it may be reached.
    pub addr: PeerAddr,
    /// What the box takes, once, as proof the client holds the ticket.
    pub secret: TicketSecret,
    /// The client forgets the box once it stays unreachable for an hour.
    pub ephemeral: bool,
}

/// Why text is not a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TicketError {
    /// It does not start with the ticket prefix.
    #[error("a pairing ticket starts with `{TICKET_PREFIX}`")]
    Prefix,
    /// It has characters a ticket cannot have.
    #[error("the pairing ticket has characters a ticket cannot have")]
    Base32,
    /// Its checksum does not match, as after a typing mistake.
    #[error("the pairing ticket is damaged; copy it again from `devlanes daemon pair`")]
    Checksum,
    /// The checksum matches but the fields do not fit together.
    #[error("the pairing ticket's fields are invalid")]
    Layout,
    /// It sets flags this devlanes does not know, as a newer one would.
    #[error("the pairing ticket comes from a newer devlanes; update devlanes on this machine")]
    Flags,
}

/// How every ticket starts, from `devlanes-proto`, so the CLI tells a ticket
/// from a box name without this crate.
const TICKET_PREFIX: &str = devlanes_proto::remote::TICKET_PREFIX;
/// Flag bit of [`Ticket::ephemeral`]; the other bits must be 0.
const EPHEMERAL: u8 = 1;
/// Longest relay URL a ticket holds, in bytes.
const MAX_TICKET_RELAY: usize = 200;
/// Most addresses a ticket holds.
const MAX_TICKET_ADDRS: usize = 8;
/// Bytes of the SHA-256 checksum at the end.
const CHECKSUM_LEN: usize = 4;
/// Bytes of the largest ticket: flags, id, secret, the longest relay URL
/// with its length, and the most IPv6 addresses with their count and the
/// checksum.
const MAX_TICKET_BYTES: usize =
    1 + 32 + 16 + 1 + MAX_TICKET_RELAY + 1 + MAX_TICKET_ADDRS * (1 + 16 + 2) + CHECKSUM_LEN;
/// Characters of the largest ticket's text with its prefix: unpadded base32
/// takes 8 characters per 5 bytes, rounded up.
const MAX_TICKET_TEXT: usize = TICKET_PREFIX.len() + (MAX_TICKET_BYTES * 8).div_ceil(5);

impl Ticket {
    /// Longest relay URL a ticket holds, in bytes.
    pub const MAX_RELAY: usize = MAX_TICKET_RELAY;

    /// The ticket as text: the prefix, then its bytes in lowercase base32.
    /// Keeps the first 8 addresses, and leaves out a relay URL longer than
    /// [`Ticket::MAX_RELAY`], which the box checks before it mints one.
    pub fn encode(&self) -> String {
        let mut bytes = Vec::with_capacity(128);
        bytes.push(if self.ephemeral { EPHEMERAL } else { 0 });
        bytes.extend_from_slice(&self.addr.id.0);
        bytes.extend_from_slice(&self.secret.0);
        let relay = self
            .addr
            .relay
            .as_deref()
            .filter(|relay| relay.len() <= MAX_TICKET_RELAY)
            .unwrap_or_default();
        // At most 200, so the length fits in one byte.
        bytes.push(u8::try_from(relay.len()).unwrap_or(0));
        bytes.extend_from_slice(relay.as_bytes());
        let addrs: Vec<&SocketAddr> = self.addr.direct.iter().take(MAX_TICKET_ADDRS).collect();
        bytes.push(u8::try_from(addrs.len()).unwrap_or(0));
        for addr in addrs {
            match addr.ip() {
                std::net::IpAddr::V4(ip) => {
                    bytes.push(4);
                    bytes.extend_from_slice(&ip.octets());
                }
                std::net::IpAddr::V6(ip) => {
                    bytes.push(6);
                    bytes.extend_from_slice(&ip.octets());
                }
            }
            bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        let checksum = Sha256::digest(&bytes);
        bytes.extend_from_slice(&checksum[..CHECKSUM_LEN]);
        let text = data_encoding::BASE32_NOPAD
            .encode(&bytes)
            .to_ascii_lowercase();
        format!("{TICKET_PREFIX}{text}")
    }

    /// Reads a ticket in either case, with whitespace around it. The
    /// checksum is checked before the fields, so a mistyped ticket fails
    /// here instead of reaching the box as a wrong secret. Text longer than
    /// the largest ticket is refused before it is decoded.
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        let text = text.trim();
        if text.len() > MAX_TICKET_TEXT {
            return Err(TicketError::Layout);
        }
        let body = text
            .get(..TICKET_PREFIX.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(TICKET_PREFIX))
            .and_then(|_| text.get(TICKET_PREFIX.len()..))
            .ok_or(TicketError::Prefix)?;
        let bytes = data_encoding::BASE32_NOPAD_NOCASE
            .decode(body.as_bytes())
            .map_err(|_| TicketError::Base32)?;
        let split = bytes
            .len()
            .checked_sub(CHECKSUM_LEN)
            .ok_or(TicketError::Layout)?;
        let (fields, checksum) = bytes.split_at(split);
        // Not secret: a checksum against typing mistakes needs no constant time.
        if Sha256::digest(fields)[..CHECKSUM_LEN] != *checksum {
            return Err(TicketError::Checksum);
        }
        let mut reader = Fields(fields);
        let flags = reader.byte()?;
        if flags & !EPHEMERAL != 0 {
            return Err(TicketError::Flags);
        }
        let id = PeerId(reader.array()?);
        let secret = TicketSecret(reader.array()?);
        let relay_len = usize::from(reader.byte()?);
        if relay_len > MAX_TICKET_RELAY {
            return Err(TicketError::Layout);
        }
        let relay = match reader.take(relay_len)? {
            [] => None,
            relay => Some(
                std::str::from_utf8(relay)
                    .map_err(|_| TicketError::Layout)?
                    .to_owned(),
            ),
        };
        let count = usize::from(reader.byte()?);
        if count > MAX_TICKET_ADDRS {
            return Err(TicketError::Layout);
        }
        let mut direct = Vec::with_capacity(count);
        for _ in 0..count {
            let ip: std::net::IpAddr = match reader.byte()? {
                4 => std::net::Ipv4Addr::from(reader.array::<4>()?).into(),
                6 => std::net::Ipv6Addr::from(reader.array::<16>()?).into(),
                _ => return Err(TicketError::Layout),
            };
            let port = u16::from_be_bytes(reader.array()?);
            direct.push(SocketAddr::new(ip, port));
        }
        if !reader.0.is_empty() {
            return Err(TicketError::Layout);
        }
        Ok(Self {
            addr: PeerAddr { id, relay, direct },
            secret,
            ephemeral: flags & EPHEMERAL != 0,
        })
    }
}

/// The unread fields of a ticket; every read fails with
/// [`TicketError::Layout`] when the bytes run out.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    /// The next `len` bytes.
    fn take(&mut self, len: usize) -> Result<&'a [u8], TicketError> {
        let (taken, rest) = self.0.split_at_checked(len).ok_or(TicketError::Layout)?;
        self.0 = rest;
        Ok(taken)
    }

    /// The next byte.
    fn byte(&mut self) -> Result<u8, TicketError> {
        Ok(self.take(1)?[0])
    }

    /// The next `N` bytes as an array.
    fn array<const N: usize>(&mut self) -> Result<[u8; N], TicketError> {
        self.take(N)?.try_into().map_err(|_| TicketError::Layout)
    }
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

    /// A ticket with the given relay, addresses and flag.
    fn ticket(relay: Option<&str>, direct: &[&str], ephemeral: bool) -> Ticket {
        Ticket {
            addr: PeerAddr {
                id: PeerId(std::array::from_fn(|index| (index * 3 + 1) as u8)),
                relay: relay.map(str::to_owned),
                direct: direct
                    .iter()
                    .map(|addr| addr.parse().expect("socket address"))
                    .collect(),
            },
            secret: TicketSecret(std::array::from_fn(|index| (index * 11) as u8)),
            ephemeral,
        }
    }

    /// Every ticket reads back as itself from its text, also uppercased and
    /// with the whitespace a paste brings.
    #[test]
    fn tickets_read_back_from_their_text() {
        let longest = format!("https://{}", "r".repeat(MAX_TICKET_RELAY - 8));
        let eight: Vec<String> = (0..8)
            .map(|n| format!("[2001:db8::{n}]:{}", 40_000 + n))
            .collect();
        let eight: Vec<&str> = eight.iter().map(String::as_str).collect();
        let tickets = [
            ticket(None, &[], false),
            ticket(Some("https://relay1.devlanes.run"), &[], false),
            ticket(None, &["192.0.2.7:4433", "[2001:db8::1]:1"], true),
            ticket(Some(&longest), &eight, true),
        ];
        for ticket in &tickets {
            let text = ticket.encode();
            assert!(text.starts_with("dlp1"), "{text}");
            assert_eq!(Ticket::parse(&text).as_ref(), Ok(ticket), "{text}");
            let pasted = format!("  {}\n", text.to_uppercase());
            assert_eq!(Ticket::parse(&pasted).as_ref(), Ok(ticket), "{pasted}");
        }
        // The sizes the CLI's ticket detection and the docs rely on.
        assert_eq!(tickets[1].encode().len(), 136);
        assert_eq!(tickets[3].encode().len(), 656);
        assert!(
            !format!("{tickets:?}").contains("11, 22"),
            "the secret is not in Debug"
        );
    }

    /// Text that is not a ticket fails with the reason, and a typing mistake
    /// is caught by the checksum.
    #[test]
    fn damaged_tickets_are_refused() {
        let text = ticket(
            Some("https://relay1.devlanes.run"),
            &["192.0.2.7:4433"],
            false,
        )
        .encode();
        let mut typo = text.clone().into_bytes();
        let middle = typo.len() / 2;
        typo[middle] = if typo[middle] == b'a' { b'b' } else { b'a' };
        let typo = String::from_utf8(typo).expect("ascii");
        // Valid checksums around fields a box would never write.
        let with_checksum = |fields: &[u8]| {
            let mut bytes = fields.to_vec();
            bytes.extend_from_slice(&Sha256::digest(fields)[..CHECKSUM_LEN]);
            format!("dlp1{}", data_encoding::BASE32_NOPAD.encode(&bytes))
        };
        let mut flagged = vec![2u8];
        flagged.extend_from_slice(&[0; 48]);
        flagged.extend_from_slice(&[0, 0]);
        let mut truncated = vec![0u8];
        truncated.extend_from_slice(&[0; 40]);
        let cases = [
            ("ssh-box".to_owned(), TicketError::Prefix),
            (text.replacen("dlp1", "dlp2", 1), TicketError::Prefix),
            (format!("{text}!"), TicketError::Base32),
            (typo, TicketError::Checksum),
            (with_checksum(&flagged), TicketError::Flags),
            (with_checksum(&truncated), TicketError::Layout),
            ("dlp1".to_owned(), TicketError::Layout),
            // Longer than any ticket: refused before it is decoded.
            (
                format!("dlp1{}", "a".repeat(MAX_TICKET_TEXT)),
                TicketError::Layout,
            ),
        ];
        for (text, error) in cases {
            assert_eq!(Ticket::parse(&text), Err(error), "{text}");
        }
    }
}
