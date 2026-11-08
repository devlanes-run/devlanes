use std::{path::PathBuf, time::Duration};

use crate::Health;

/// Failures of the plain-container adapter.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("docker API: {0}")]
    Docker(#[from] bollard::errors::Error),
    #[error("docker I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot read docker config {path}: {source}")]
    DockerConfig {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("docker context {0:?} not found; check `docker context ls`")]
    UnknownContext(String),
    #[error(
        "invalid name {0:?}: use letters, digits, `_`, `.` and `-`, starting with a letter or digit"
    )]
    InvalidName(String),
    #[error("invalid volume {spec:?} for container {container:?}: {reason}")]
    InvalidVolume {
        container: String,
        spec: String,
        reason: &'static str,
    },
    #[error("invalid port block starting at {first} with size {size}")]
    InvalidBlock { first: u16, size: u16 },
    #[error("host port {port} of {container}.{name} is outside the block {first}-{last}")]
    PortOutsideBlock {
        container: String,
        name: String,
        port: u16,
        first: u16,
        last: u16,
    },
    #[error("container {container:?} uses host port {port} twice")]
    DuplicatePort { container: String, port: u16 },
    #[error("container {container:?} declares port name {name:?} twice")]
    DuplicatePortName { container: String, name: String },
    #[error(
        "a container named {name:?} already exists and devlanes does not manage it{}; remove it with `docker rm -f {name}`",
        .owner.as_ref().map(|owner| format!(" (it belongs to devlanes project {owner:?})")).unwrap_or_default()
    )]
    NameConflict { name: String, owner: Option<String> },
    #[error("container {name:?} is {health:?} instead of healthy")]
    Unhealthy { name: String, health: Health },
    #[error("container {name:?} was not healthy within {timeout:?} (last state {last:?})")]
    HealthTimeout {
        name: String,
        timeout: Duration,
        last: Health,
    },
}

impl Error {
    /// Docker answers 404 for missing containers, images and volumes.
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Error::Docker(bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            })
        )
    }

    /// The engine is not running: its Unix socket file is absent, or its socket,
    /// TCP endpoint or Windows named pipe refused the connection or does not
    /// exist. Read from the typed error and its sources, never from the message.
    /// A timeout, a permission problem, a busy pipe or an engine that answered
    /// with an error is something else: the engine may run.
    pub fn is_engine_stopped(&self) -> bool {
        let stopped = |error: &std::io::Error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        };
        match self {
            Error::Io(error) => stopped(error),
            Error::Docker(bollard::errors::Error::SocketNotFoundError(_)) => true,
            // `IOError` is transparent, so its `source` skips the I/O error itself.
            Error::Docker(bollard::errors::Error::IOError { err }) => stopped(err),
            Error::Docker(error) => {
                let mut source = std::error::Error::source(error);
                while let Some(error) = source {
                    if let Some(io) = error.downcast_ref::<std::io::Error>() {
                        return stopped(io);
                    }
                    source = error.source();
                }
                false
            }
            _ => false,
        }
    }

    /// A pause or unpause that lost a race: the container is gone (404), already
    /// paused or no longer running (409), or not paused. Docker answers the last
    /// one with a plain 500, so only its message tells it apart.
    pub(crate) fn is_state_race(&self) -> bool {
        match self {
            Error::Docker(bollard::errors::Error::DockerResponseServerError {
                status_code,
                message,
            }) => {
                matches!(status_code, 404 | 409)
                    || (*status_code == 500 && message.ends_with("is not paused"))
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Engine API error response with `status_code` and `message`.
    fn response(status_code: u16, message: &str) -> Error {
        Error::Docker(bollard::errors::Error::DockerResponseServerError {
            status_code,
            message: message.into(),
        })
    }

    /// Lost pause and unpause races count as success; other errors do not.
    #[test]
    fn pause_races_are_recognised() {
        // Messages as Docker 29 sends them.
        assert!(response(404, "No such container: x").is_state_race());
        assert!(response(409, "container abc is already paused").is_state_race());
        assert!(response(409, "container abc is not running").is_state_race());
        assert!(response(500, "Container abc is not paused").is_state_race());
        assert!(!response(500, "cannot pause: permission denied").is_state_race());
        assert!(!Error::UnknownContext("x".into()).is_state_race());
    }

    /// A request to an engine endpoint with nothing behind it reads as a
    /// stopped engine, through bollard's wrapping on each OS.
    #[tokio::test]
    async fn stopped_engine_is_recognised() {
        #[cfg(unix)]
        let (_dir, host) = {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join("docker.sock");
            // The socket file stays after the listener is gone, so the
            // request is refused instead of failing to find the file.
            drop(std::os::unix::net::UnixListener::bind(&path).expect("bind"));
            let host = format!("unix://{}", path.display());
            (dir, host)
        };
        #[cfg(windows)]
        let host = format!("npipe:////./pipe/devlanes-test-{}-none", std::process::id());
        let docker = crate::DockerInfra::from_client(
            bollard::Docker::connect_with_host(&host).expect("client"),
        );
        let error = docker.server_version().await.expect_err("nothing answers");
        assert!(error.is_engine_stopped(), "{error:?}");
        assert!(!response(500, "engine error").is_engine_stopped());
    }
}
