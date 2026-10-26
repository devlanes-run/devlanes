//! The box's pairing gate, answered by the box daemon: `remote.pairing.verify`
//! checks a client's token, `remote.pairing.claim` trades its code.

use std::time::Duration;

use async_trait::async_trait;
use devlanes_client::{Client, Paths};
use devlanes_proto::{
    Context, Method, RpcError,
    remote::{ClaimParams, VerifyParams},
};
use devlanes_transport::{
    BoxError, ClientInfo, PairingCheck, PairingClaim, PairingGate, PairingReason,
};
use serde::Deserialize;
use tokio::time::timeout;

/// Limit for connecting to the box daemon and for its answer.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);

/// [`PairingGate`] over the box daemon's socket, for a box whose user config
/// requires pairing.
pub(crate) struct DaemonPairing {
    paths: Paths,
    /// The tunnel's `$SSH_CONNECTION`, which the daemon logs with refusals.
    address: Option<String>,
}

impl DaemonPairing {
    /// Asks the daemon behind `paths` about clients coming from `address`.
    pub(crate) fn new(paths: Paths, address: Option<String>) -> Self {
        Self { paths, address }
    }

    /// One request to the box daemon within [`DAEMON_TIMEOUT`]; `Ok(Err)` is
    /// the daemon's own refusal.
    async fn ask(
        &self,
        method: Method,
        params: serde_json::Value,
    ) -> Result<Result<serde_json::Value, RpcError>, BoxError> {
        let asked = async {
            let mut daemon = Client::connect(&self.paths).await?;
            daemon.request(method, params, Context::default()).await
        };
        match timeout(DAEMON_TIMEOUT, asked).await {
            Err(_) => Err(PairingError::Timeout.into()),
            Ok(Ok(answer)) => Ok(Ok(answer)),
            Ok(Err(devlanes_client::Error::Rpc(error))) => Ok(Err(error)),
            Ok(Err(error)) => Err(error.into()),
        }
    }
}

/// Why the tunnel could not ask the box daemon about a client.
#[derive(Debug, thiserror::Error)]
enum PairingError {
    /// The box daemon did not answer in time.
    #[error("the box daemon did not answer about the client's pairing in time")]
    Timeout,
    /// The box daemon refused the question itself.
    #[error("the box daemon refused {method}: {} ({})", .error.code, .error.data)]
    Refused {
        /// The method asked.
        method: &'static str,
        /// The daemon's error.
        error: RpcError,
    },
    /// The answer did not have the expected shape.
    #[error("the box daemon sent an unexpected answer: {0}")]
    Answer(#[from] serde_json::Error),
}

/// `remote.pairing.verify`'s answer.
#[derive(Deserialize)]
struct Verdict {
    paired: bool,
    #[serde(default)]
    reason: Option<PairingReason>,
}

/// `remote.pairing.claim`'s answer. Not `Debug`: it holds the token.
#[derive(Deserialize)]
struct Claimed {
    token: String,
}

/// `pairing_refused`'s data.
#[derive(Deserialize)]
struct Refusal {
    message: String,
    #[serde(default)]
    retry_after_ms: Option<u64>,
}

#[async_trait]
impl PairingGate for DaemonPairing {
    /// Asks the daemon whether `token` is `client`'s.
    async fn check(
        &self,
        client: &ClientInfo,
        token: Option<&str>,
    ) -> Result<PairingCheck, BoxError> {
        let params = VerifyParams {
            id: client.id.clone(),
            name: client.name.clone(),
            token: token.map(str::to_owned),
            address: self.address.clone(),
        };
        let answer = self
            .ask(Method::RemotePairingVerify, serde_json::to_value(params)?)
            .await?
            .map_err(|error| PairingError::Refused {
                method: "remote.pairing.verify",
                error,
            })?;
        let verdict: Verdict = serde_json::from_value(answer).map_err(PairingError::from)?;
        Ok(if verdict.paired {
            PairingCheck::Allowed
        } else {
            PairingCheck::Required(verdict.reason.unwrap_or(PairingReason::Invalid))
        })
    }

    /// Asks the daemon to trade `code`; its `pairing_refused` is the client's
    /// refusal, any other error ends the session.
    async fn claim(&self, client: &ClientInfo, code: &str) -> Result<PairingClaim, BoxError> {
        let params = ClaimParams {
            id: client.id.clone(),
            name: client.name.clone(),
            code: code.to_owned(),
            address: self.address.clone(),
        };
        match self
            .ask(Method::RemotePairingClaim, serde_json::to_value(params)?)
            .await?
        {
            Ok(answer) => {
                let claimed: Claimed =
                    serde_json::from_value(answer).map_err(PairingError::from)?;
                Ok(PairingClaim::Paired {
                    token: claimed.token,
                })
            }
            Err(error) if error.code == "pairing_refused" => {
                let refusal: Refusal =
                    serde_json::from_value(error.data).map_err(PairingError::from)?;
                Ok(PairingClaim::Refused {
                    message: refusal.message,
                    retry_after: refusal.retry_after_ms.map(Duration::from_millis),
                })
            }
            Err(error) => Err(PairingError::Refused {
                method: "remote.pairing.claim",
                error,
            }
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use devlanes_proto::Outcome;
    use serde_json::json;

    use super::*;

    /// A token check reaches the daemon with the client's id, name, token and
    /// address, and a refusal comes back as pairing required.
    #[tokio::test]
    async fn token_checks_go_to_the_daemon() -> crate::test_daemon::TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let paths = crate::test_daemon::home(dir.path())?;
            let daemon = crate::test_daemon::answer_one(&paths, |_| Outcome::Ok {
                data: json!({"paired": false, "reason": "invalid"}),
            })
            .await?;
            let gate = DaemonPairing::new(paths, Some("203.0.113.4 1 198.51.100.1 22".into()));
            let client = ClientInfo {
                id: "dl_cid_x".into(),
                name: "laptop".into(),
                ..ClientInfo::default()
            };
            let check = gate.check(&client, Some("dl_rt_y")).await?;
            assert_eq!(check, PairingCheck::Required(PairingReason::Invalid));
            let request = daemon.await??;
            assert_eq!(request.method, Method::RemotePairingVerify);
            let params: VerifyParams = serde_json::from_value(request.params)?;
            assert_eq!(params.id, "dl_cid_x");
            assert_eq!(params.name, "laptop");
            assert_eq!(params.token.as_deref(), Some("dl_rt_y"));
            assert_eq!(
                params.address.as_deref(),
                Some("203.0.113.4 1 198.51.100.1 22")
            );
            Ok(())
        })
        .await?
    }

    /// The daemon's `pairing_refused` is the client's refusal, with the wait.
    #[tokio::test]
    async fn a_refused_code_keeps_the_daemon_reason() -> crate::test_daemon::TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let paths = crate::test_daemon::home(dir.path())?;
            let daemon = crate::test_daemon::answer_one(&paths, |_| Outcome::Error {
                error: RpcError::new(
                    "pairing_refused",
                    json!({"message": "too many wrong pairing codes", "retry_after_ms": 60000}),
                    4,
                ),
            })
            .await?;
            let gate = DaemonPairing::new(paths, None);
            let claim = gate.claim(&ClientInfo::default(), "ABCD-EFGH").await?;
            assert_eq!(
                claim,
                PairingClaim::Refused {
                    message: "too many wrong pairing codes".into(),
                    retry_after: Some(Duration::from_secs(60)),
                }
            );
            let request = daemon.await??;
            assert_eq!(request.method, Method::RemotePairingClaim);
            let params: ClaimParams = serde_json::from_value(request.params)?;
            assert_eq!(params.code, "ABCD-EFGH");
            Ok(())
        })
        .await?
    }
}
