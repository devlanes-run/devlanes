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

The key file holds the 32-byte secret key. It is created private to the user (mode 0600 on Unix), and a key other accounts can read is refused.

## License

Apache-2.0
