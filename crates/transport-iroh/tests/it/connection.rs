//! The connection checks of `devlanes_transport::conformance` on iroh: a
//! client endpoint dials a listening box endpoint over loopback with no
//! relay, as one session would.

use std::sync::Arc;
use std::time::Duration;

use devlanes_transport::conformance::{self, Pair};
use devlanes_transport::{Alpn, BoxError, PeerAddr, PeerEndpoint, PeerId, TUNNEL_ALPN};
use iroh::endpoint::{PortmapperConfig, QuicTransportConfig, VarInt, presets};
use iroh::{Endpoint, Watcher};

use super::{Everyone, Home, TestResult, bounded};

/// Most checks finish in well under a second on loopback.
const CHECK: Duration = Duration::from_secs(10);

/// A session's connection between two iroh endpoints. Breaking the link
/// closes it from the box with `close_peer`, a close the client did not ask
/// for; the endpoints and their home live as long as the pair.
async fn iroh() -> Result<Pair, BoxError> {
    let home = Home::new()?;
    let client = home.endpoint("client", Vec::new());
    let boxed = home.endpoint("box", Vec::new());
    boxed.listen(Some(Arc::new(Everyone))).await?;
    let connection = client.connect(&boxed.addr().await?, Alpn::Tunnel).await?;
    let incoming = boxed.accept().await.ok_or("no connection")?;
    let peer = incoming.peer;
    Ok(Pair {
        client: connection,
        server: incoming.connection,
        sever: Box::new(move || {
            let _alive = (&home, &client);
            boxed.close_peer(&peer);
        }),
    })
}

/// A box whose process vanishes sends no close, so only QUIC's idle timeout
/// can end the client's connection; it does, with a reason to show. The box
/// is a raw iroh endpoint in its own runtime, shut down without closing
/// anything, with an idle timeout of 2 s, which the client's connection
/// takes as the lower of the two sides' timeouts.
#[tokio::test]
async fn a_box_that_vanishes_ends_the_connection_with_a_reason() -> TestResult {
    bounded(Duration::from_secs(20), async {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let (addr_tx, addr) = tokio::sync::oneshot::channel();
        runtime.spawn(async move {
            let config = QuicTransportConfig::builder()
                .max_idle_timeout(Some(VarInt::from_u32(2_000).into()))
                // Keeps the connection alive while the box runs.
                .keep_alive_interval(Duration::from_millis(500))
                .build();
            let Ok(boxed) = Endpoint::builder(presets::Minimal)
                // iroh maps ports by default, which probes the router.
                .portmapper_config(PortmapperConfig::Disabled)
                .clear_relay_transports()
                .transport_config(config)
                .alpns(vec![TUNNEL_ALPN.to_vec()])
                .bind()
                .await
            else {
                return;
            };
            let mut watcher = boxed.watch_addr();
            while watcher.get().ip_addrs().next().is_none() {
                if watcher.updated().await.is_err() {
                    return;
                }
            }
            let addr = boxed.addr();
            let _ = addr_tx.send(PeerAddr {
                id: PeerId(*boxed.id().as_bytes()),
                relay: None,
                direct: addr.ip_addrs().copied().collect(),
            });
            let mut held = Vec::new();
            while let Some(incoming) = boxed.accept().await {
                if let Ok(connection) = incoming.await {
                    held.push(connection);
                }
            }
        });
        let addr = addr.await?;

        let home = Home::new()?;
        let client = home.endpoint("client", Vec::new());
        let connection = client.connect(&addr, Alpn::Tunnel).await?;
        let vanished = tokio::time::Instant::now();
        runtime.shutdown_background();
        connection.closed().await;
        // Well past the 2 s idle timeout for a loaded runner, and still far
        // from the 30 s the client's own timeout would take.
        assert!(
            vanished.elapsed() < Duration::from_secs(10),
            "closed after {:?}",
            vanished.elapsed()
        );
        let failure = connection.failure_within(Duration::ZERO).await;
        assert!(failure.is_some(), "no reason for a box that vanished");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn iroh_last_inbound_moves_with_what_the_peer_sends() -> TestResult {
    bounded(CHECK, async {
        conformance::last_inbound_moves_with_what_the_peer_sends(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_a_half_closed_stream_keeps_the_other_direction() -> TestResult {
    bounded(CHECK, async {
        conformance::a_half_closed_stream_keeps_the_other_direction(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_a_dropped_stream_ends_cleanly_after_its_bytes() -> TestResult {
    bounded(CHECK, async {
        conformance::a_dropped_stream_ends_cleanly_after_its_bytes(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_max_streams_stay_open_at_once() -> TestResult {
    bounded(Duration::from_secs(20), async {
        conformance::max_streams_stay_open_at_once(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_open_never_waits_while_streams_churn_at_the_limit() -> TestResult {
    bounded(Duration::from_secs(30), async {
        conformance::open_never_waits_while_streams_churn_at_the_limit(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_the_first_stream_written_is_accepted_first() -> TestResult {
    bounded(CHECK, async {
        conformance::the_first_stream_written_is_accepted_first(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_finished_bytes_survive_the_box_dropping_the_connection() -> TestResult {
    bounded(CHECK, async {
        conformance::finished_bytes_survive_the_box_dropping_the_connection(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_a_client_shutdown_ends_the_connection() -> TestResult {
    bounded(CHECK, async {
        conformance::a_client_shutdown_ends_the_connection(iroh().await?).await
    })
    .await
}

#[tokio::test]
async fn iroh_a_broken_link_reports_why() -> TestResult {
    bounded(CHECK, async {
        conformance::a_broken_link_reports_why(iroh().await?).await
    })
    .await
}
