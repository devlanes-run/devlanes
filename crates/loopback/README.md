# devlanes-loopback

Connects to ports on this machine's loopback addresses. The proxy dials local services through it, the daemon's `tcp` and `http` readiness checks probe with it, and a box's transport dials its own services with it, so all of them reach a port the same way.

## Usage

```rust
use std::{net::{Ipv6Addr, SocketAddr}, time::Duration};

async fn reach(port: u16) -> std::io::Result<()> {
    // 127.0.0.1 first, then ::1, each attempt bounded by the limit.
    let stream = devlanes_loopback::connect_loopback(port, Duration::from_secs(5)).await?;
    // A declared address is dialed alone.
    let address = SocketAddr::from((Ipv6Addr::LOCALHOST, 8000));
    let other = devlanes_loopback::connect(address, Duration::from_secs(5)).await?;
    drop((stream, other));
    Ok(())
}
```

## API

- `connect_loopback(port, per_address_timeout)`: `127.0.0.1`, else `::1`. When both fail, a refusal wins over the other error, since callers retry refusals.
- `connect(address, timeout)`: one address.

## Windows

Windows answers a reset by sending the SYN again, so a refused loopback connect takes about 2 s and a server listening only on `::1` would cost that on every connection. Loopback sockets therefore skip SYN retransmissions (`SIO_TCP_INITIAL_RTO`, as libuv does), and a refusal fails at once as on macOS and Linux. A listener whose accept backlog is full also answers with a reset, so a burst beyond the backlog now fails at once instead of being retried by the stack.

## License

Apache-2.0
