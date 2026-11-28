//! The endpoint's ports across restarts, read from the addresses it reports
//! for a ticket: a client that stored them must reach the box again after
//! the box's daemon restarts, also with no relay.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use devlanes_transport::{BoxError, PeerAddr, PeerEndpoint};

use super::{Home, TestResult, bounded};

/// Every check binds a few endpoints on this machine.
const CHECK: Duration = Duration::from_secs(20);

/// The IPv4 and IPv6 ports `addr` reports.
fn ports(addr: &PeerAddr) -> (Option<u16>, Option<u16>) {
    let port = |v4: bool| {
        addr.direct
            .iter()
            .find(|direct| direct.is_ipv4() == v4)
            .map(SocketAddr::port)
    };
    (port(true), port(false))
}

/// The ports the endpoint of `home` named `name` reports, after which it is
/// closed and dropped, as a daemon that stops.
async fn run_once(home: &Home, name: &str) -> Result<(Option<u16>, Option<u16>), BoxError> {
    let endpoint = home.endpoint(name, Vec::new());
    let addr = endpoint.addr().await?;
    endpoint.close().await;
    drop(endpoint);
    Ok(ports(&addr))
}

/// Binds a UDP socket on `addr`, waiting, bounded by the caller, while the
/// endpoint that had it releases it: iroh frees its sockets in tasks that
/// finish after `close`.
async fn hold(addr: SocketAddr) -> UdpSocket {
    loop {
        if let Ok(socket) = UdpSocket::bind(addr) {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until the ports are free again, so the next endpoint is not handed
/// another port only because the last one has not let go yet.
async fn released((v4, v6): (Option<u16>, Option<u16>)) {
    if let Some(port) = v4 {
        drop(hold((Ipv4Addr::UNSPECIFIED, port).into()).await);
    }
    if let Some(port) = v6 {
        // Bound only when this machine has IPv6, as iroh's socket was.
        if UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).is_ok() {
            drop(hold((Ipv6Addr::UNSPECIFIED, port).into()).await);
        }
    }
}

/// An endpoint built again with the same options, as after a daemon restart,
/// reports the ports it reported before.
#[tokio::test]
async fn a_restarted_endpoint_keeps_its_ports() -> TestResult {
    bounded(CHECK, async {
        let home = Home::new()?;
        let first = run_once(&home, "box").await?;
        assert!(first.0.is_some(), "no IPv4 port reported: {first:?}");
        released(first).await;
        assert_eq!(run_once(&home, "box").await?, first);
        Ok(())
    })
    .await
}

/// With its saved IPv4 port taken by another program, the endpoint still
/// binds, on another port, and keeps that one from then on.
#[tokio::test]
async fn a_taken_port_is_replaced_and_kept() -> TestResult {
    bounded(CHECK, async {
        let home = Home::new()?;
        let first = run_once(&home, "box").await?;
        let saved = first.0.ok_or("no IPv4 port reported")?;
        let blocker = hold((Ipv4Addr::UNSPECIFIED, saved).into()).await;
        let second = run_once(&home, "box").await?;
        let moved = second.0.ok_or("no IPv4 port reported")?;
        assert_ne!(moved, saved, "bound the port another socket holds");
        drop(blocker);
        released(second).await;
        assert_eq!(run_once(&home, "box").await?.0, Some(moved));
        Ok(())
    })
    .await
}
