//! The peer checks of `devlanes_transport::conformance`, run on memory
//! endpoints of one hub. The daemon's tests stand on `MemoryPeers`, so it
//! has to pass the same checks as the iroh endpoint.

use devlanes_transport::conformance::{self, Peers};
use devlanes_transport::memory::MemoryPeers;

use super::{TestResult, bounded};

/// Three endpoints of a fresh hub.
fn memory() -> Peers {
    let hub = MemoryPeers::new();
    Peers {
        dialers: [hub.endpoint(), hub.endpoint()],
        listener: hub.endpoint(),
    }
}

#[tokio::test]
async fn memory_an_unadmitted_peer_is_refused_before_any_stream() -> TestResult {
    bounded(conformance::an_unadmitted_peer_is_refused_before_any_stream(memory())).await
}

#[tokio::test]
async fn memory_close_peer_ends_only_that_peers_connections() -> TestResult {
    bounded(conformance::close_peer_ends_only_that_peers_connections(
        memory(),
    ))
    .await
}

#[tokio::test]
async fn memory_a_listener_that_stopped_accepts_nothing() -> TestResult {
    bounded(conformance::a_listener_that_stopped_accepts_nothing(
        memory(),
    ))
    .await
}

#[tokio::test]
async fn memory_a_closed_endpoint_ends_its_connections_cleanly() -> TestResult {
    bounded(conformance::a_closed_endpoint_ends_its_connections_cleanly(
        memory(),
    ))
    .await
}
