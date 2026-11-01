//! `daemon.status`'s `setup`: the steps of `devlanes setup` the daemon can tell
//! are missing on this machine. Trust stores belong to the user, so the client
//! checks the CA itself.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{engine::Inner, model::lock};

/// How long a resolver check answers `daemon.status` before it is read again.
pub(crate) const RESOLVER_CHECK_TTL: Duration = Duration::from_secs(60);

impl Inner {
    /// `{"missing": [..]}`: `hosts` when the hosts helper cannot write
    /// `/etc/hosts` until setup runs, `resolver` when the resolver strategy's
    /// TLD does not reach the DNS server. Every `daemon.status` asks, so the
    /// resolver check, which reads the OS's files or registry, is cached for
    /// [`RESOLVER_CHECK_TTL`] (`Inner::resolver_check`).
    pub(crate) async fn setup_status(&self) -> Value {
        let mut missing = Vec::new();
        if self
            .integ
            .hosts
            .as_ref()
            .is_some_and(|hosts| hosts.needs_setup())
        {
            missing.push("hosts");
        }
        let cached = *lock(&self.resolver_check);
        let unrouted = match cached {
            Some((read, unrouted)) if read.elapsed() < RESOLVER_CHECK_TTL => unrouted,
            _ => {
                let unrouted = self.resolver_unrouted().await;
                *lock(&self.resolver_check) = Some((Instant::now(), unrouted));
                unrouted
            }
        };
        if unrouted {
            missing.push("resolver");
        }
        json!({"missing": missing})
    }
}
