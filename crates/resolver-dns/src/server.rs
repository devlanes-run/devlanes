use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{DnsError, Zone, answer};

/// Default port: unprivileged, so the daemon never needs root to serve DNS.
#[cfg(unix)]
pub const DEFAULT_PORT: u16 = 15353;

/// Default port: 53, because an NRPT rule names only a server address and
/// Windows always queries it on port 53. Any user may bind it on Windows.
#[cfg(windows)]
pub const DEFAULT_PORT: u16 = 53;

/// Large enough for any query a stub resolver sends, EDNS options included.
const UDP_BUFFER: usize = 4096;

/// Bounds TCP clients; the OS resolver uses TCP only as a UDP fallback.
const MAX_TCP_CONNECTIONS: usize = 64;

/// Closes TCP connections that send nothing, so stuck clients cannot hold permits.
const TCP_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Pause after a failed TCP accept, typically EMFILE.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Attempts to find a free port for both UDP and TCP when binding port 0.
const EPHEMERAL_ATTEMPTS: usize = 16;

/// `127.0.0.1:15353`.
#[cfg(unix)]
pub fn default_listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT)
}

/// `127.53.0.1:53`: port 53 on `127.0.0.1` may belong to another local DNS
/// server, and the rest of `127/8` is loopback too.
#[cfg(windows)]
pub fn default_listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 53, 0, 1)), DEFAULT_PORT)
}

/// Names of the two protocols in errors.
const UDP: &str = "UDP";
const TCP: &str = "TCP";

/// First port and size of the dynamic range IANA sets aside, which Windows
/// and macOS use for ephemeral ports; ports the fallback of an ephemeral bind
/// tries.
const DYNAMIC_START: u16 = 49152;
const DYNAMIC_PORTS: u64 = 16384;

/// Steps between the ports the fallback tries: odd, so 16384 steps visit every
/// port once, and near the golden ratio of the range, so any 17 of them in a
/// row lie hundreds of ports apart, out of a 100-port reserved block.
const STRIDE: u64 = 10125;

/// Ports of the dynamic range, from a place `seed` picks, each far from the
/// ones before it.
fn spread_ports(seed: u64) -> impl Iterator<Item = u16> {
    (0..DYNAMIC_PORTS).map(move |step| {
        let offset = (seed % DYNAMIC_PORTS + step * STRIDE) % DYNAMIC_PORTS;
        DYNAMIC_START.saturating_add(u16::try_from(offset).unwrap_or(0))
    })
}

/// A per-process random start for [`spread_ports`], so two daemons do not try
/// the same ports in the same order.
fn seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(u64::from(std::process::id()));
    hasher.finish()
}

/// A `protocol` bind failure on `addr`.
#[cfg(unix)]
fn bind_error(addr: SocketAddr, protocol: &'static str, source: io::Error) -> DnsError {
    DnsError::Bind {
        addr,
        protocol,
        source,
    }
}

/// A `protocol` bind failure on `addr`. When another program holds the
/// address, the message says how to find it, since Windows names no owner in
/// the error.
#[cfg(windows)]
fn bind_error(addr: SocketAddr, protocol: &'static str, source: io::Error) -> DnsError {
    // WSAEACCES (10013): another socket holds the address exclusively.
    let held = source.kind() == io::ErrorKind::AddrInUse || source.raw_os_error() == Some(10013);
    let source = if held && addr.port() != 0 {
        let (netstat, cmdlet) = if protocol == TCP {
            ("tcp", "Get-NetTCPConnection")
        } else {
            ("udp", "Get-NetUDPEndpoint")
        };
        io::Error::new(
            source.kind(),
            format!(
                "{source}; another program holds port {port} on {ip}; find it with \
                 `netstat -ano -p {netstat} | findstr {addr}` (the last column is its process id) \
                 or `{cmdlet} -LocalAddress {ip} -LocalPort {port}` in PowerShell",
                port = addr.port(),
                ip = addr.ip(),
            ),
        )
    } else {
        source
    };
    DnsError::Bind {
        addr,
        protocol,
        source,
    }
}

/// A bound DNS server over UDP and TCP on the same port.
#[derive(Debug)]
pub struct DnsServer {
    udp: UdpSocket,
    tcp: TcpListener,
    zone: Arc<Zone>,
}

/// Whether binding the UDP twin of an ephemeral TCP port failed in a way
/// another port fixes.
#[cfg(unix)]
fn retry_twin(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::AddrInUse
}

/// Whether binding the UDP twin of an ephemeral TCP port failed in a way
/// another port fixes. Binds inside the ranges Hyper-V and WSL reserve fail
/// with access denied.
#[cfg(windows)]
fn retry_twin(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
    )
}

impl DnsServer {
    /// Binds UDP and TCP on `listen`.
    ///
    /// Port 0 picks one ephemeral port free for both protocols, which tests use.
    pub async fn bind(listen: SocketAddr, zone: Zone) -> Result<Self, DnsError> {
        if listen.port() != 0 {
            let udp = UdpSocket::bind(listen)
                .await
                .map_err(|e| bind_error(listen, UDP, e))?;
            let tcp = TcpListener::bind(listen)
                .await
                .map_err(|e| bind_error(listen, TCP, e))?;
            return Ok(Self::new(udp, tcp, zone));
        }
        // TCP first: Windows keeps its ephemeral TCP ports out of the TCP
        // ranges Hyper-V and WSL reserve, but its ephemeral UDP ports can
        // land inside those TCP ranges.
        let tcp = TcpListener::bind(listen)
            .await
            .map_err(|e| bind_error(listen, TCP, e))?;
        let addr = tcp.local_addr().map_err(|e| bind_error(listen, TCP, e))?;
        let mut last = match UdpSocket::bind(addr).await {
            Ok(udp) => return Ok(Self::new(udp, tcp, zone)),
            // The TCP port's UDP twin is taken, or on Windows inside a
            // reserved UDP range (WSAEACCES); another port will do.
            Err(error) if retry_twin(&error) => (addr, UDP, error),
            Err(error) => return Err(bind_error(addr, UDP, error)),
        };
        drop(tcp);
        // Windows and macOS hand out ephemeral ports close to each other, so
        // asking again lands next to the port that just failed, inside the
        // same run of taken ports or, on Windows, reserved block (Hyper-V
        // reserves 100 at a time); asking the OS again failed on both in CI.
        // Ports far apart from each other get out of it. Linux binds any port
        // of this range as well, so one fallback serves every OS.
        for port in spread_ports(seed()).take(EPHEMERAL_ATTEMPTS) {
            let addr = SocketAddr::new(listen.ip(), port);
            let tcp = match TcpListener::bind(addr).await {
                Ok(tcp) => tcp,
                Err(error) if retry_twin(&error) => {
                    last = (addr, TCP, error);
                    continue;
                }
                Err(error) => return Err(bind_error(addr, TCP, error)),
            };
            match UdpSocket::bind(addr).await {
                Ok(udp) => return Ok(Self::new(udp, tcp, zone)),
                Err(error) if retry_twin(&error) => last = (addr, UDP, error),
                Err(error) => return Err(bind_error(addr, UDP, error)),
            }
        }
        let (addr, protocol, source) = last;
        Err(DnsError::NoSharedPort {
            ip: listen.ip(),
            attempts: EPHEMERAL_ATTEMPTS + 1,
            addr,
            protocol,
            source,
        })
    }

    /// Wraps bound sockets.
    fn new(udp: UdpSocket, tcp: TcpListener, zone: Zone) -> Self {
        Self {
            udp,
            tcp,
            zone: Arc::new(zone),
        }
    }

    /// The bound address, shared by UDP and TCP.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp.local_addr()
    }

    /// The zone being answered.
    pub fn zone(&self) -> &Zone {
        &self.zone
    }

    /// Serves until `shutdown` is cancelled, then closes every connection.
    ///
    /// Per-packet and per-connection errors are logged and skipped so one bad client
    /// cannot stop resolution for the whole machine.
    pub async fn serve(self, shutdown: CancellationToken) {
        let Self { udp, tcp, zone } = self;
        let permits = Arc::new(Semaphore::new(MAX_TCP_CONNECTIONS));
        let mut connections = JoinSet::new();
        let mut buf = vec![0u8; UDP_BUFFER];
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                received = udp.recv_from(&mut buf) => match received {
                    Ok((len, peer)) => {
                        if let Some(response) = answer(&buf[..len], &zone)
                            && let Err(error) = udp.send_to(&response, peer).await
                        {
                            tracing::debug!(%peer, %error, "dns udp send failed");
                        }
                    }
                    // ICMP errors from earlier sends surface here on some platforms.
                    Err(error) => tracing::debug!(%error, "dns udp receive failed"),
                },
                accepted = tcp.accept() => match accepted {
                    Ok((stream, peer)) => {
                        let Ok(permit) = permits.clone().try_acquire_owned() else {
                            tracing::debug!(%peer, "dns tcp connection limit reached");
                            continue;
                        };
                        let zone = zone.clone();
                        connections.spawn(async move {
                            if let Err(error) = serve_tcp(stream, &zone).await {
                                tracing::debug!(%peer, %error, "dns tcp connection ended");
                            }
                            drop(permit);
                        });
                    }
                    Err(error) => {
                        tracing::debug!(%error, "dns tcp accept failed");
                        // EMFILE/ENFILE keep failing until a descriptor frees up; without
                        // a pause this arm would spin and starve the UDP side.
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            () = tokio::time::sleep(ACCEPT_BACKOFF) => {}
                        }
                    }
                },
                // Reap finished connections so the set does not grow without bound.
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        connections.shutdown().await;
    }
}

/// Answers length-prefixed queries on one TCP connection (RFC 7766) until it closes.
async fn serve_tcp(mut stream: TcpStream, zone: &Zone) -> io::Result<()> {
    loop {
        let mut len = [0u8; 2];
        match tokio::time::timeout(TCP_IDLE_TIMEOUT, stream.read_exact(&mut len)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Err(_) => return Ok(()),
        }
        let mut message = vec![0u8; usize::from(u16::from_be_bytes(len))];
        tokio::time::timeout(TCP_IDLE_TIMEOUT, stream.read_exact(&mut message))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "partial dns message"))??;
        let Some(response) = answer(&message, zone) else {
            continue;
        };
        let response_len = u16::try_from(response.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "response too large"))?;
        let mut framed = Vec::with_capacity(response.len() + 2);
        framed.extend_from_slice(&response_len.to_be_bytes());
        framed.extend_from_slice(&response);
        // A client that stops reading must not pin the task and its permit forever.
        tokio::time::timeout(TCP_IDLE_TIMEOUT, stream.write_all(&framed))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "dns client stopped reading"))??;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback visits every dynamic port once, and the ports one bind
    /// tries are far enough apart that no 100-port reserved block holds two.
    #[test]
    fn spread_ports_stay_out_of_one_block() {
        let mut all: Vec<u16> = spread_ports(7).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len() as u64, DYNAMIC_PORTS);
        assert_eq!(all.first(), Some(&DYNAMIC_START));
        // A different seed shifts the sequence, so seed 0 covers every window.
        let tried: Vec<u64> = spread_ports(0)
            .take(EPHEMERAL_ATTEMPTS + 1)
            .map(u64::from)
            .collect();
        for (i, a) in tried.iter().enumerate() {
            for b in &tried[i + 1..] {
                let apart = a.abs_diff(*b);
                let apart = apart.min(DYNAMIC_PORTS - apart);
                assert!(apart >= 100, "{a} and {b} are {apart} apart");
            }
        }
    }
}
