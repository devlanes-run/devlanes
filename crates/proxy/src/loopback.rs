use std::{
    future::Future,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use tokio::net::TcpStream;

/// Connects to `port` on `127.0.0.1` or `::1`, whichever accepts, preferring IPv4
/// when both do. Each address gets `timeout`. When neither connects, a refusal
/// outranks the other error, since callers read it as a service that may be restarting.
pub(crate) async fn connect_loopback(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let attempt = |ip: IpAddr| async move {
        tokio::time::timeout(timeout, TcpStream::connect((ip, port)))
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "loopback dial timed out",
                ))
            })
    };
    first_to_connect(
        attempt(Ipv4Addr::LOCALHOST.into()),
        attempt(Ipv6Addr::LOCALHOST.into()),
    )
    .await
}

/// Unix refuses a closed loopback port at once, so trying IPv6 only after IPv4
/// fails costs nothing and opens no second connection to a server on both.
#[cfg(not(windows))]
async fn first_to_connect<F>(v4: F, v6: F) -> io::Result<TcpStream>
where
    F: Future<Output = io::Result<TcpStream>>,
{
    match v4.await {
        Ok(stream) => Ok(stream),
        Err(v4) => v6.await.map_err(|v6| refusal_first(v4, v6)),
    }
}

/// Windows retries a refused loopback SYN for about 2 s, so waiting for IPv4 to
/// fail would delay every connection to a server listening only on `::1`.
#[cfg(windows)]
async fn first_to_connect<F>(v4: F, v6: F) -> io::Result<TcpStream>
where
    F: Future<Output = io::Result<TcpStream>>,
{
    tokio::pin!(v4, v6);
    tokio::select! {
        biased;
        result = &mut v4 => match result {
            Ok(stream) => Ok(stream),
            Err(v4) => v6.await.map_err(|v6| refusal_first(v4, v6)),
        },
        result = &mut v6 => match result {
            Ok(stream) => Ok(stream),
            Err(v6) => v4.await.map_err(|v4| refusal_first(v4, v6)),
        },
    }
}

/// The IPv4 error unless only IPv6 was refused, as on a machine without IPv6
/// loopback where IPv6 fails with another error.
fn refusal_first(v4: io::Error, v6: io::Error) -> io::Error {
    if v4.kind() != io::ErrorKind::ConnectionRefused
        && v6.kind() == io::ErrorKind::ConnectionRefused
    {
        v6
    } else {
        v4
    }
}
