//! The peer-to-peer transport for `devlanes remote connect`, over iroh 1.5.
//!
//! [`devlanes_transport::PeerEndpoint`] is implemented over one iroh endpoint
//! per daemon. Sessions use one QUIC stream per protocol stream. A dial fails
//! unless a direct path between the two machines forms, and a session closes
//! once it has had no direct path for about 3 seconds; the relay helps peers
//! find each other and carries session data for at most about 3 seconds.

mod connection;
mod endpoint;
mod key;
mod text;

pub use endpoint::{IrohEndpoint, IrohOptions};
