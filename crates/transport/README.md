# devlanes-transport

The remote protocol between a client's daemon and the daemon on a box, independent of how the bytes travel. The client daemon uses `Session` or `Link` to mirror the box (registry, data streams, syncs); the box daemon runs `serve` behind `devlanes tunnel --stdio`. Transports such as `devlanes-transport-ssh` implement `Transport`.

## Usage

Client side, reconnecting with backoff:

```rust
use std::sync::Arc;
use devlanes_transport::{Backoff, DialTarget, Link, SessionOptions, Transport};

async fn client(transport: Arc<dyn Transport>) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = SessionOptions::new("devlanes 0.1.0");
    options.client_id = "0f3c...".into(); // kept by the client machine across restarts
    options.client_name = "laptop".into();
    let link = Link::spawn(transport, options, Backoff::default());
    let session = link.connected().await?;
    println!("box proxy on {:?}", session.box_info().https_port);

    // Ports of non-hidden services and containers, bound locally under the same numbers.
    let ports: Vec<u16> = link.registry().current().mirrored_ports().into_keys().collect();
    if let Some(&port) = ports.first() {
        let _stream = session.open(DialTarget::Port { port }).await?;
    }
    Ok(())
}
```

Box side, over the tunnel's stdio:

```rust
use std::sync::Arc;
use devlanes_transport::{
    BoxServices, LoopbackDialer, NoSync, RegistrySnapshot, ServeOptions, SharedRegistry,
    TUNNEL_PREAMBLE, serve, stdio,
};
use tokio::io::AsyncWriteExt;

async fn tunnel() -> Result<(), Box<dyn std::error::Error>> {
    // Before any mux byte, so the client can skip what a login shell printed.
    let mut stdout = tokio::io::stdout();
    stdout.write_all(TUNNEL_PREAMBLE.as_bytes()).await?;
    stdout.flush().await?;
    let registry = SharedRegistry::new(RegistrySnapshot::default(), 256);
    let services = BoxServices {
        registry: Arc::new(registry.clone()),
        dialer: Arc::new(LoopbackDialer),
        sync: Arc::new(NoSync),
    };
    serve(stdio(), services, ServeOptions::new("devlanes 0.1.0")).await?;
    Ok(())
}
```

Publish changes with `registry.publish(event)`. A daemon that only has full snapshots can publish `old.diff(&new)`. A transport that runs the tunnel reads its output with `read_preamble(stream, wait)` and hands `Preamble::stream` to the session. A preamble naming another protocol revision is `PreambleError::Protocol`, which `PreambleError::is_permanent` reports as permanent.

## API

- `Transport`: produces one duplex byte stream per `connect`.
- `read_preamble`: skips output before the tunnel's `TUNNEL_PREAMBLE` line, keeping it for errors.
- `Session`: one connection; `open`, `subscribe_sync`, `report` (what the client mirrors, with `mirror_report`), `features`, `has_feature`, `box_info`, `closed`.
- `Link`: keeps a session alive and shares one `ClientRegistry` across reconnects. A transport error classified `TransportError::NeedsAuth` parks it in `LinkState::NeedsAuth` without retrying until `Link::resume`. `TransportError::AuthFailed`, rejected credentials on a box that never connected, stops it like `Permanent`, unless the caller knows a prompt went unanswered and turns it into `NeedsAuth` with `into_needs_auth`.
- `Registry`: snapshot plus events; `mirrored_ports`, `advertised_ports`, `authorize`, `diff`.
- `serve` with `BoxServices`: `RegistrySource` (told each client's `ClientInfo`, and handed its mirror reports), `Dialer`, `SyncSource`.
- `memory::memory_transport`: in-process transport with freeze and sever controls for tests.

## Config example

```yaml
# devlanes.yaml on the box
services:
  admin:
    run: ./admin-server
    # Not mirrored to machines connected with devlanes remote connect.
    hidden: true
infra:
  # Containers whose ports are not mirrored either.
  hidden: [postgres]
```

A client running `devlanes remote connect <box>` binds every other service and container port locally under the same number; the box refuses streams to `admin`'s and `postgres`'s ports.

## Behaviour

Only the client opens streams, at most 512 at once; an open beyond that fails alone and the session stays up. With the `mirror_report` feature the client reports what it mirrors on the control stream, at most once per change, and the box hands the newest report of a burst to `RegistrySource::report`. The box dials only ports of running, non-hidden services, containers and external ports (state `external`) in its registry. A client without the `external_units` feature cannot read external ports (`UnitKind::External`), so a registry source leaves them out of that client's feed. An external port may name its loopback address (`PortInfo::address`); `LoopbackDialer` then dials only that address instead of `127.0.0.1`, then `::1`. The box rejects everything else, and closes open streams when their unit stops, disappears or becomes hidden. Tasks are listed with their state but never mirrored or dialed. A data stream whose one side finished and that stays silent for 60 s is closed. There are no request streams: a client never runs commands on the box. On a sync stream the client may send input after its request (`SyncSubscription::send`, then `finish_input` for an explicit end marker); the `SyncSource` reads it from `SyncInput`, and a client sending faster than the source reads waits. With the `sync_zstd` feature the box compresses each chunk zstd makes smaller (chunks under 256 bytes go raw, and so does the rest of a file after a chunk that did not shrink), and `SyncSubscription::next` returns it decompressed; the client compresses its input values the same way. Compressed data may not decode to more than the size its frame declares, at most 8 MiB. Pings run every 10 s; a session dies after 20 s with neither a pong nor any other incoming frame, so a large transfer does not starve it. On reconnect, the box's snapshot replaces the client's registry; until then the client keeps the last known state. The session ends when the registry feed fails or ends.

Both ends must speak the same `PROTOCOL_VERSION`, which also covers devlanes' payload shapes, and a mismatch is a permanent error. It changes only for breaking changes. Additive behaviour is a named feature: both ends list theirs in the handshake (`FEATURES` by default), and a side uses a feature only when the other lists it too.

## License

Apache-2.0
