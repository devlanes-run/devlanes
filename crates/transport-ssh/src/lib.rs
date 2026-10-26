//! SSH [`devlanes_transport::Transport`] over system OpenSSH.
//!
//! On Unix, one ControlMaster per box lives under the given control directory
//! and each session runs `devlanes tunnel --stdio` on the box over it. Windows'
//! OpenSSH has no ControlMaster, so there each session is one `ssh.exe` that
//! probes the box and then execs the tunnel. Either way the session is a
//! [`devlanes_transport_command::CommandTransport`] run, which reads the
//! tunnel's preamble and reports how it ended. Prompts go through
//! [`askpass`]; a box without devlanes fails with [`SshError::NotInstalled`].

pub mod askpass;
mod probe;
#[cfg(unix)]
mod ssh;
#[cfg(windows)]
#[path = "ssh_windows.rs"]
mod ssh;

pub use askpass::{
    AskpassConfig, AskpassError, AskpassPrompt, PromptKind, PromptRelay, run_askpass,
};
pub use probe::{BoxShell, INSTALL_DIR, RemotePlatform, RemoteProbe};
pub use ssh::{SshError, SshOptions, SshTransport};

/// How long a started tunnel gets to write its preamble line. It writes the
/// line before anything else, so only a box that runs an older release (whose
/// tunnel waits for the client) or a stuck login shell takes this long.
pub(crate) const PREAMBLE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// The tunnel process, as its failures name it.
pub(crate) const TUNNEL: &str = "remote `devlanes tunnel --stdio`";

/// Quotes `value` for a POSIX shell using single quotes.
pub(crate) fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// 128 bits from the OS random source as 32 lowercase hex digits, for names
/// another process must not predict: relay pipes and session sentinels.
#[cfg(windows)]
pub(crate) fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use devlanes_transport::TransportError;
    use devlanes_transport_command::CommandError;

    use super::*;

    #[test]
    fn quoting_survives_embedded_single_quotes() {
        assert_eq!(super::sh_quote("it's"), r"'it'\''s'");
    }

    /// A box without devlanes points at the setup guide, and the error's
    /// permanence decides whether the link stops.
    #[test]
    fn a_missing_binary_points_at_the_setup_guide() {
        let error = SshError::NotInstalled {
            destination: "acme".to_owned(),
            permanent: true,
        };
        assert!(error.is_permanent());
        let message = error.to_string();
        assert!(
            message.starts_with("devlanes is not installed on acme"),
            "{message}"
        );
        assert!(
            message.contains("docs/guides/remote-setup.md#on-the-box"),
            "{message}"
        );
        assert!(TransportError::from(error).is_permanent());
    }

    /// A tunnel failure keeps the command transport's class: the box's
    /// stopped daemon retries with the hint, a protocol mismatch stops the link.
    #[test]
    fn tunnel_failures_keep_their_class() {
        let stopped = SshError::Tunnel(Box::new(CommandError::DaemonStopped {
            name: "acme".to_owned(),
        }));
        let message = stopped.to_string();
        assert!(
            message.contains("the devlanes daemon is not running on acme"),
            "{message}"
        );
        assert!(!TransportError::from(stopped).is_permanent());
        let mismatch = SshError::Tunnel(Box::new(CommandError::Preamble {
            error: devlanes_transport::PreambleError::Protocol {
                found: devlanes_transport::PROTOCOL_VERSION + 1,
                output: Vec::new(),
            },
            permanent: true,
            stderr: String::new(),
        }));
        assert!(
            mismatch.to_string().contains("install the same release"),
            "{mismatch}"
        );
        assert!(TransportError::from(mismatch).is_permanent());
    }
}
