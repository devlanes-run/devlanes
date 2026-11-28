//! The peer-to-peer transport for `devlanes remote connect`, over iroh 1.5.
//!
//! [`devlanes_transport::PeerEndpoint`] is implemented over one iroh endpoint
//! per daemon. Sessions use one QUIC stream per protocol stream and run only
//! on a direct path between the two machines: the relay helps them find each
//! other and is never used for session data.

#[allow(dead_code)]
mod connection;
#[allow(dead_code)]
mod key;
