//! Pairing: a box serves only clients it paired with, unless its owner turns
//! that off.
//!
//! The box side is a [`PairingGate`] in [`crate::BoxServices`]. Before the
//! welcome, [`crate::serve`] asks it about the client's id and remote token. A
//! client the gate does not accept gets [`crate::PairingReason`] instead of
//! the welcome and may answer once with a one-time code, which the gate trades
//! for a new token. The client side is a [`PairingClient`] in
//! [`crate::SessionOptions`]: it supplies the stored token, asks a person for
//! the code and keeps the token the box sent.

use std::time::Duration;

use async_trait::async_trait;

use crate::server::ClientInfo;
use crate::transport::BoxError;
use crate::wire::PairingReason;

/// Whether the box serves a client, as its [`PairingGate`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingCheck {
    /// Serve it: its token is valid, or the box does not require pairing.
    Allowed,
    /// Ask it for a one-time code first.
    Required(PairingReason),
}

/// The outcome of trading a one-time code.
#[derive(Clone, PartialEq, Eq)]
pub enum PairingClaim {
    /// The code was valid; the client presents `token` from now on.
    Paired {
        /// The new remote token.
        token: String,
    },
    /// The code was wrong, expired or used, or the box refuses codes for now.
    Refused {
        /// What the client shows its user.
        message: String,
        /// How long every code is refused, after too many failures.
        retry_after: Option<Duration>,
    },
}

impl std::fmt::Debug for PairingClaim {
    /// Leaves the token out.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paired { .. } => f.write_str("Paired"),
            Self::Refused {
                message,
                retry_after,
            } => f
                .debug_struct("Refused")
                .field("message", message)
                .field("retry_after", retry_after)
                .finish(),
        }
    }
}

/// Box side: who may connect.
#[async_trait]
pub trait PairingGate: Send + Sync {
    /// Whether `client`, presenting `token` (or none), is served without a code.
    async fn check(
        &self,
        client: &ClientInfo,
        token: Option<&str>,
    ) -> Result<PairingCheck, BoxError>;

    /// Trades `code` for a remote token for `client`.
    async fn claim(&self, client: &ClientInfo, code: &str) -> Result<PairingClaim, BoxError>;
}

/// A box that serves every client, for tests and `remote.require_pairing: false`.
pub struct NoPairing;

#[async_trait]
impl PairingGate for NoPairing {
    /// Always [`PairingCheck::Allowed`].
    async fn check(&self, _: &ClientInfo, _: Option<&str>) -> Result<PairingCheck, BoxError> {
        Ok(PairingCheck::Allowed)
    }

    /// Never asked, since nothing is required; refuses all the same.
    async fn claim(&self, _: &ClientInfo, _: &str) -> Result<PairingClaim, BoxError> {
        Ok(PairingClaim::Refused {
            message: "this box does not pair clients".into(),
            retry_after: None,
        })
    }
}

/// Client side: the token for one box and the person who has the code.
#[async_trait]
pub trait PairingClient: Send + Sync {
    /// The remote token this client keeps for the box, if any.
    async fn token(&self) -> Option<String>;

    /// A one-time code for the box, asked for `reason`; `None` when nobody
    /// gives one, which leaves the session needing pairing.
    async fn code(&self, reason: PairingReason) -> Option<String>;

    /// Keeps the token the box sent for later connections.
    async fn store(&self, token: String) -> Result<(), BoxError>;
}
