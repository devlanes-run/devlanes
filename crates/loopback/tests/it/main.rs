use std::{io, net::Ipv6Addr, time::Duration};

use tokio::net::TcpListener;

/// A server listening only on `::1` is reached without waiting out the refusal
/// on `127.0.0.1`, which Windows stretches to 2 s by sending the SYN again.
#[tokio::test]
async fn reaches_a_port_listening_only_on_ipv6() -> io::Result<()> {
    let listener = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let started = std::time::Instant::now();
    devlanes_loopback::connect_loopback(port, Duration::from_secs(10)).await?;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "took {:?}",
        started.elapsed()
    );
    Ok(())
}
