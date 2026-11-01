//! `daemon.status`'s `setup`: the steps of `devlanes setup` the daemon can tell
//! are missing on this machine. Trust stores belong to the user, so the client
//! checks the CA itself.

use serde_json::{Value, json};

use crate::engine::Inner;

impl Inner {
    /// `{"missing": [..]}`: `hosts` when the hosts helper cannot write
    /// `/etc/hosts` until setup runs, `resolver` when the resolver strategy's
    /// TLD does not reach the DNS server.
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
        if self.resolver_unrouted().await {
            missing.push("resolver");
        }
        json!({"missing": missing})
    }
}
