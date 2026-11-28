# devlanes-transport-iroh

The peer-to-peer transport for `devlanes remote connect`, over iroh. It implements `PeerEndpoint` from `devlanes-transport`; the daemon uses it only through that trait, and only `bins/devlanes` may construct it. Each session runs over QUIC streams on a direct path between the two machines.

## Usage

```rust
use std::sync::Arc;
use devlanes_transport::{Admission, Alpn, PeerAddr, PeerEndpoint, PeerId};
use devlanes_transport_iroh::{IrohEndpoint, IrohOptions};

struct Pinned(Vec<PeerId>);

impl Admission for Pinned {
    fn admit(&self, peer: &PeerId, alpn: Alpn) -> bool {
        alpn == Alpn::Tunnel && self.0.contains(peer)
    }
}

async fn run(box_addr: PeerAddr, client: PeerId) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = IrohEndpoint::new(IrohOptions {
        key_file: "/home/acme/.devlanes/p2p-key".into(),
        relays: vec!["https://relay.example.invalid".into()],
        address_lookup: true,
        port_mapping: true,
    });
    // On a box: serve the pinned client.
    endpoint.listen(Some(Arc::new(Pinned(vec![client])))).await?;
    if let Some(incoming) = endpoint.accept().await {
        println!("{} connected", incoming.peer.short());
    }
    // On a client: dial a box; fails unless a direct path forms.
    let _connection = endpoint.connect(&box_addr, Alpn::Tunnel).await?;
    endpoint.close().await;
    Ok(())
}
```

## API

- `IrohEndpoint::new`: one endpoint per daemon; binds on first use.
- `IrohOptions`: the key file, the relays (empty for none), n0's address lookup and port mapping.
- `PeerEndpoint` methods: `id`, `addr` for a ticket, `connect`, `listen`, `accept`, `close_peer`, `close`.

## Network use

Nothing is bound or contacted until `addr`, `connect` or `listen(Some(..))` needs it; `id` only reads or creates the key file, and `listen(None)` on an endpoint that never bound does nothing. Binding opens a UDP socket for IPv4 and one for IPv6 and, with relays, keeps a connection to the nearest relay open and probes every relay in the list. With `address_lookup`, a listening endpoint publishes its relay URL (never its IP addresses) to n0's DNS at `dns.iroh.link`, and a dial looks the box up there. With `port_mapping`, it asks the router for a port mapping over UPnP, NAT-PMP and PCP. A dial fails with `PeerError::NoDirectPath` unless a direct path forms within 15 seconds, and a session closes once it has had no direct path for about 3 seconds, so session data crosses a relay for at most about 3 seconds.

A dial contacts only the relays in `relays`. A ticket's relay, or one a box names in its address record, that is not in the list is dropped, and the dial tries the box's direct addresses alone, so a pasted ticket cannot make this machine send a request to an address the ticket chose. To reach a box on another relay, add that relay to the list. Text the other side controls (close reasons, a dropped relay's host) has control and format characters removed and is cut to 200 bytes before it reaches an error or a log line.

The first `addr` after a bind waits up to 5 seconds for the relay; later calls return at once with what is known, the home relay once one answered or else the first configured relay. The bind runs on its own, so a caller that stops waiting does not stop it.

Once bound, the endpoint stays up while it listens, while any connection is open and for 5 minutes after the last call that needed it. Then it closes its sockets, its relay connection and its network checks, and the next `addr`, `connect` or `listen(Some(..))` binds again, so a machine that made one ticket does not hold a relay connection for the daemon's whole life. `close` is final; after it every call fails with `PeerError::Closed`.

An endpoint that listens binds the same UDP ports at every start, so a client that stored the box's addresses from a ticket still reaches it after the box's daemon restarts, also without a relay. When a saved port is taken, it binds another one and logs that clients on a network without a relay must pair again. `listen(None)` forgets the ports: the daemon calls it when no ticket is outstanding and no client is paired, so nothing depends on them any more. An endpoint that only dials keeps no ports and binds new ones at each start, so a laptop does not use the same source port on every network it joins; a machine that is both a box and a client keeps its ports for its dials too while it listens. With `port_mapping`, a kept port also keeps the router's mapping on the same public port across restarts; handshakes that arrive through it are still refused without the right ALPN and an admitted key.

## Files

The crate writes up to two files, both in the directory of `IrohOptions::key_file`:

- the key file, which holds the 32-byte secret key. It is created private to the user (mode 0600 on Unix), and a key other accounts can read is refused.
- `p2p-port`, written by `listen(Some(..))` and removed by `listen(None)`: the UDP ports the endpoint bound, one line per address family. A bind asks for these ports only when the file exists. It is not secret; a missing or unreadable file means any port, and ports below 1024 are ignored.

## License

Apache-2.0
