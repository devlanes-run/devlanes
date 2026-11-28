//! iroh endpoints on this machine against the transport's conformance
//! checks, with no relay or an in-process relay on loopback, so nothing
//! leaves the machine.

mod connection;
mod gate;
mod peer;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use devlanes_transport::{Admission, Alpn, BoxError, PeerId};
use devlanes_transport_iroh::{IrohEndpoint, IrohOptions};
use tempfile::TempDir;

type TestResult = Result<(), BoxError>;

/// Fails the test instead of hanging when a future never completes.
async fn bounded<F: Future<Output = TestResult>>(limit: Duration, future: F) -> TestResult {
    tokio::time::timeout(limit, future).await?
}

/// Endpoints in one temporary home, each with its own key file.
struct Home {
    directory: TempDir,
}

impl Home {
    /// A fresh home.
    fn new() -> Result<Self, BoxError> {
        Ok(Self {
            directory: tempfile::tempdir()?,
        })
    }

    /// Where the endpoint named `name` keeps its key.
    fn key_file(&self, name: &str) -> PathBuf {
        self.directory.path().join(name).join("p2p-key")
    }

    /// An endpoint named `name` with `relays`, without n0's address lookup
    /// or port mapping, both of which would reach hosts outside this machine.
    fn endpoint(&self, name: &str, relays: Vec<String>) -> Arc<IrohEndpoint> {
        Arc::new(IrohEndpoint::new(IrohOptions {
            key_file: self.key_file(name),
            relays,
            address_lookup: false,
            port_mapping: false,
        }))
    }
}

/// Lets every peer in, for checks that are not about admission.
struct Everyone;

impl Admission for Everyone {
    /// Admits `_peer` for any protocol.
    fn admit(&self, _peer: &PeerId, _alpn: Alpn) -> bool {
        true
    }
}
