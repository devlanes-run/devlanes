//! The peer checks of `devlanes_transport::conformance` on iroh endpoints
//! over loopback with no relay. They prove `MemoryPeers`, which the daemon's
//! tests use, behaves like the real endpoint.

use std::sync::Arc;
use std::time::Duration;

use devlanes_transport::PeerEndpoint;
use devlanes_transport::conformance::{self, Peers};

use super::{Everyone, Home, TestResult, bounded};

/// Every check dials a few times on loopback.
const CHECK: Duration = Duration::from_secs(10);

/// Three endpoints in one home; the home lives until the check ends.
fn iroh(home: &Home) -> Peers {
    Peers {
        dialers: [
            home.endpoint("first", Vec::new()),
            home.endpoint("second", Vec::new()),
        ],
        listener: home.endpoint("box", Vec::new()),
    }
}

/// Stopping a listener that never started touches nothing: the daemon stops
/// listening after every revoke and ticket expiry, also on a machine that
/// never used P2P, and binding there would contact the relay and the router.
/// The key file is the trace binding leaves, since a bind reads the key first.
#[tokio::test]
async fn stopping_an_endpoint_that_never_listened_binds_nothing() -> TestResult {
    let home = Home::new()?;
    bounded(CHECK, async {
        let endpoint = home.endpoint("box", Vec::new());
        endpoint.listen(None).await?;
        assert!(
            !home.key_file("box").exists(),
            "listen(None) created the key, so it bound"
        );
        endpoint.listen(Some(Arc::new(Everyone))).await?;
        assert!(home.key_file("box").exists(), "listen(Some) did not bind");
        endpoint.close().await;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn iroh_an_unadmitted_peer_is_refused_before_any_stream() -> TestResult {
    let home = Home::new()?;
    bounded(
        CHECK,
        conformance::an_unadmitted_peer_is_refused_before_any_stream(iroh(&home)),
    )
    .await
}

#[tokio::test]
async fn iroh_close_peer_ends_only_that_peers_connections() -> TestResult {
    let home = Home::new()?;
    bounded(
        CHECK,
        conformance::close_peer_ends_only_that_peers_connections(iroh(&home)),
    )
    .await
}

#[tokio::test]
async fn iroh_a_closed_endpoint_ends_its_connections_cleanly() -> TestResult {
    let home = Home::new()?;
    bounded(
        CHECK,
        conformance::a_closed_endpoint_ends_its_connections_cleanly(iroh(&home)),
    )
    .await
}

#[tokio::test]
async fn iroh_a_listener_that_stopped_accepts_nothing() -> TestResult {
    let home = Home::new()?;
    bounded(
        CHECK,
        conformance::a_listener_that_stopped_accepts_nothing(iroh(&home)),
    )
    .await
}
