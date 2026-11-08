//! Connects to ports on this machine's loopback addresses, where a dev server may
//! listen on IPv4, IPv6 or both.

use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use tokio::net::{TcpSocket, TcpStream};

/// Connects to `port` on `127.0.0.1`, else on `::1`, each attempt bounded by
/// `per_address_timeout`. When neither connects, a refusal outranks the other
/// error, since callers read it as a service that may be restarting.
pub async fn connect_loopback(port: u16, per_address_timeout: Duration) -> io::Result<TcpStream> {
    let v4 = match connect((Ipv4Addr::LOCALHOST, port).into(), per_address_timeout).await {
        Ok(stream) => return Ok(stream),
        Err(error) => error,
    };
    connect((Ipv6Addr::LOCALHOST, port).into(), per_address_timeout)
        .await
        .map_err(|v6| refusal_first(v4, v6))
}

/// Connects to `address` within `timeout`. A refused loopback address fails at
/// once on every OS, so trying the next address costs nothing.
pub async fn connect(address: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    // Off loopback a lost SYN is worth sending again. A failure is ignored, as libuv
    // does: without the option a refusal only arrives later.
    #[cfg(windows)]
    if address.ip().is_loopback() {
        let _ = devlanes_sys_windows::net::skip_syn_retransmissions(&socket);
    }
    tokio::time::timeout(timeout, socket.connect(address))
        .await
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("connecting to {address} timed out"),
            ))
        })
}

/// The IPv4 error unless only IPv6 was refused, as when IPv4 loopback times out.
fn refusal_first(v4: io::Error, v6: io::Error) -> io::Error {
    if v4.kind() != io::ErrorKind::ConnectionRefused
        && v6.kind() == io::ErrorKind::ConnectionRefused
    {
        v6
    } else {
        v4
    }
}
