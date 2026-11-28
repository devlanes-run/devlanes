//! The connection checks of `devlanes_transport::conformance`, run over
//! yamux on the memory transport. Each `#[tokio::test]` runs one check on a
//! fresh [`memory`] pair.

use std::time::Duration;

use devlanes_transport::conformance::{self, Pair};
use devlanes_transport::memory::memory_transport;
use devlanes_transport::{BoxError, Transport};

use super::{TestResult, bounded};

/// yamux over the memory transport's pipe.
async fn memory() -> Result<Pair, BoxError> {
    let (transport, mut listener) = memory_transport();
    let client = transport.connect().await?;
    let server = listener.accept().await.ok_or("no link")?;
    let link = transport.last_link().ok_or("no link control")?;
    Ok(Pair {
        client,
        server,
        sever: Box::new(move || link.sever()),
    })
}

#[tokio::test]
async fn memory_last_inbound_moves_with_what_the_peer_sends() -> TestResult {
    bounded(async {
        conformance::last_inbound_moves_with_what_the_peer_sends(memory().await?).await
    })
    .await
}

#[tokio::test]
async fn memory_a_half_closed_stream_keeps_the_other_direction() -> TestResult {
    bounded(async {
        conformance::a_half_closed_stream_keeps_the_other_direction(memory().await?).await
    })
    .await
}

#[tokio::test]
async fn memory_a_dropped_stream_ends_cleanly_after_its_bytes() -> TestResult {
    bounded(async {
        conformance::a_dropped_stream_ends_cleanly_after_its_bytes(memory().await?).await
    })
    .await
}

#[tokio::test]
async fn memory_max_streams_stay_open_at_once() -> TestResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        conformance::max_streams_stay_open_at_once(memory().await?).await
    })
    .await?
}

#[tokio::test]
async fn memory_open_never_waits_while_streams_churn_at_the_limit() -> TestResult {
    tokio::time::timeout(Duration::from_secs(30), async {
        conformance::open_never_waits_while_streams_churn_at_the_limit(memory().await?).await
    })
    .await?
}

#[tokio::test]
async fn memory_the_first_stream_written_is_accepted_first() -> TestResult {
    bounded(async {
        conformance::the_first_stream_written_is_accepted_first(memory().await?).await
    })
    .await
}

#[tokio::test]
async fn memory_finished_bytes_survive_the_box_dropping_the_connection() -> TestResult {
    bounded(async {
        conformance::finished_bytes_survive_the_box_dropping_the_connection(memory().await?).await
    })
    .await
}

#[tokio::test]
async fn memory_a_client_shutdown_ends_the_connection() -> TestResult {
    bounded(async { conformance::a_client_shutdown_ends_the_connection(memory().await?).await })
        .await
}

#[tokio::test]
async fn memory_a_broken_link_reports_why() -> TestResult {
    bounded(async { conformance::a_broken_link_reports_why(memory().await?).await }).await
}
