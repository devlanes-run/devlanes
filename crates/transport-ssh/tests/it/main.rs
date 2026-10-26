//! Real SSH checks, normally run through `e2e/ssh/run.sh` against a throwaway sshd container.
//! `windows_sshd_box` instead needs a Windows box (Windows' OpenSSH Server, devlanes
//! installed for the account it logs in as, no daemon running there yet), so `run.sh`
//! skips it. Run it from any client, Linux, macOS, WSL or Windows, with the variables
//! below pointing at that box:
//!
//! ```sh
//! DEVLANES_TEST_SSH_DEST=acme@winbox cargo nextest run -p devlanes-transport-ssh \
//!   --run-ignored only -E 'test(windows_sshd_box)'
//! ```
//!
//! Environment:
//! - `DEVLANES_TEST_SSH_DEST`: ssh destination, default `localhost`.
//! - `DEVLANES_TEST_SSH_PORT`, `DEVLANES_TEST_SSH_KEY`, `DEVLANES_TEST_SSH_KNOWN_HOSTS`:
//!   optional; a key also disables the agent so only that key is offered.
//! - `DEVLANES_TEST_SSH_ALLOW_MISSING=1`: the box has no devlanes; the tunnel test skips.
//! - `DEVLANES_TEST_SSH_LANE=1`: `run.sh` started a lane with an echo service and a
//!   synced folder on the box; the mirroring test needs it.
//! - `DEVLANES_TEST_SSH_PASSPHRASE_KEY`, `DEVLANES_TEST_SSH_PASSPHRASE` and
//!   `DEVLANES_TEST_SSH_ASKPASS_EXE` (a built `devlanes` binary): a key the box
//!   accepts whose passphrase the askpass test answers through the relay; the
//!   test skips without them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{DialTarget, Session, SessionOptions, SyncMessage, Transport};
use devlanes_transport_ssh::{
    AskpassConfig, AskpassPrompt, PromptKind, PromptRelay, SshOptions, SshTransport,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn allow_missing() -> bool {
    env("DEVLANES_TEST_SSH_ALLOW_MISSING").as_deref() == Some("1")
}

/// The file that discards known hosts.
#[cfg(unix)]
const NO_FILE: &str = "/dev/null";
#[cfg(windows)]
const NO_FILE: &str = "NUL";

/// Builds a transport from the test environment; paths are quoted for ssh's option parser.
fn transport(home: &Path) -> SshTransport {
    SshTransport::new(destination(), options(home, env("DEVLANES_TEST_SSH_KEY")))
}

/// The box from the test environment.
fn destination() -> String {
    env("DEVLANES_TEST_SSH_DEST").unwrap_or_else(|| "localhost".to_owned())
}

/// Options from the test environment, offering only `key` when given.
fn options(home: &Path, key: Option<String>) -> SshOptions {
    let mut options = SshOptions::new(home);
    if let Some(port) = env("DEVLANES_TEST_SSH_PORT") {
        options.ssh_options.push(format!("Port={port}"));
    }
    if let Some(key) = key {
        options.ssh_options.extend([
            format!("IdentityFile=\"{key}\""),
            "IdentitiesOnly=yes".to_owned(),
            // An agent such as 1Password could prompt or offer other keys.
            "IdentityAgent=none".to_owned(),
        ]);
    }
    if let Some(known_hosts) = env("DEVLANES_TEST_SSH_KNOWN_HOSTS") {
        options.ssh_options.extend([
            format!("UserKnownHostsFile=\"{known_hosts}\""),
            format!("GlobalKnownHostsFile={NO_FILE}"),
            "StrictHostKeyChecking=yes".to_owned(),
        ]);
    }
    options
}

/// Short home so the control socket fits in `sun_path`.
#[cfg(unix)]
fn short_home() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new().prefix("dl").tempdir_in("/tmp")
}

/// A home; Windows keeps no control socket in it.
#[cfg(windows)]
fn short_home() -> std::io::Result<tempfile::TempDir> {
    tempfile::tempdir()
}

#[tokio::test]
#[ignore = "needs ssh"]
async fn ssh_master_and_probe() -> TestResult {
    tokio::time::timeout(Duration::from_secs(60), async {
        let home = short_home()?;
        let transport = transport(home.path());
        let probe = transport.probe().await?;
        assert!(!probe.platform.os.is_empty(), "{probe:?}");
        assert!(!probe.platform.arch.is_empty(), "{probe:?}");
        if !allow_missing() {
            assert!(
                probe.program.is_some(),
                "devlanes not found on the box: {probe:?}"
            );
        }
        // The cached probe and the live master are reused rather than relaunched.
        assert_eq!(transport.probe().await?, probe);
        // Windows has no master; the second probe is the remembered answer.
        #[cfg(unix)]
        {
            let masters = std::fs::read_dir(home.path().join("run/ssh"))?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".log"))
                .count();
            assert_eq!(masters, 1);
        }
        transport.close().await;
        Ok(())
    })
    .await?
}

/// Answers passphrase prompts with `passphrase` and records every prompt.
struct Passphrase {
    passphrase: String,
    asked: Mutex<Vec<AskpassPrompt>>,
}

#[async_trait]
impl PromptRelay for Passphrase {
    async fn prompt(&self, prompt: AskpassPrompt) -> Option<String> {
        let answer = (prompt.kind == PromptKind::Secret
            && prompt.prompt.to_lowercase().contains("passphrase"))
        .then(|| self.passphrase.clone());
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(prompt);
        answer
    }
}

/// A passphrase-protected key: ssh asks through `SSH_ASKPASS`, which runs the
/// built `devlanes` binary, which asks this relay; the scripted answer gets
/// the connection through.
#[tokio::test]
#[ignore = "needs ssh"]
async fn ssh_askpass_answers_a_passphrase() -> TestResult {
    let (Some(key), Some(passphrase), Some(exe)) = (
        env("DEVLANES_TEST_SSH_PASSPHRASE_KEY"),
        env("DEVLANES_TEST_SSH_PASSPHRASE"),
        env("DEVLANES_TEST_SSH_ASKPASS_EXE"),
    ) else {
        eprintln!("skipping ssh_askpass_answers_a_passphrase: no passphrase key set up");
        return Ok(());
    };
    tokio::time::timeout(Duration::from_secs(60), async {
        let home = short_home()?;
        let relay = Arc::new(Passphrase {
            passphrase,
            asked: Mutex::new(Vec::new()),
        });
        let mut options = options(home.path(), Some(key));
        options.askpass = Some(AskpassConfig {
            exe: PathBuf::from(exe),
            relay: relay.clone(),
        });
        let transport = SshTransport::new(destination(), options);
        let probe = transport.probe().await?;
        assert!(!probe.platform.os.is_empty(), "{probe:?}");
        let asked = relay
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].destination, destination());
        assert!(asked[0].prompt.contains("passphrase"), "{asked:?}");
        transport.close().await;
        Ok(())
    })
    .await?
}

#[tokio::test]
#[ignore = "needs ssh"]
async fn ssh_tunnel_session() -> TestResult {
    tokio::time::timeout(Duration::from_secs(60), async {
        let home = short_home()?;
        let transport = transport(home.path());
        if transport.probe().await?.program.is_none() {
            if allow_missing() {
                eprintln!("skipping ssh_tunnel_session: devlanes is not installed on the box");
                return Ok(());
            }
            return Err("devlanes is not installed on the box".into());
        }
        // A working tunnel writes its preamble (which `connect` reads), then waits
        // for the client's hello or starts the mux (yamux opens with an RTT ping);
        // an unimplemented one exits at once, and its stderr is the error.
        let mut raw = match transport.connect().await {
            Ok(raw) => raw,
            Err(error) => {
                let message = error.to_string();
                let lower = message.to_lowercase();
                if lower.contains("not_supported_yet") || lower.contains("not supported yet") {
                    eprintln!(
                        "skipping ssh_tunnel_session: box reports not_supported_yet: {message}"
                    );
                    return Ok(());
                }
                return Err(error.into());
            }
        };
        let mut byte = [0u8; 1];
        match tokio::time::timeout(Duration::from_secs(3), raw.read(&mut byte)).await {
            Err(_) | Ok(Ok(1..)) => {}
            Ok(Err(error)) => return Err(error.into()),
            Ok(Ok(_)) => return Err("tunnel closed its output without an error".into()),
        }
        drop(raw);
        let session = Session::connect(&transport, SessionOptions::new("ssh-test")).await?;
        assert!(!session.server().is_empty());
        // The box reports its `ports.range` from its daemon.
        assert!(
            session.box_info().ports_range.is_some(),
            "{:?}",
            session.box_info()
        );
        session.close();
        transport.close().await;
        Ok(())
    })
    .await?
}

/// The PID of the box daemon, from `devlanes daemon status --json` run over
/// a plain ssh session with the program and shell `probe` found.
async fn box_daemon_pid(
    home: &Path,
    probe: &devlanes_transport_ssh::RemoteProbe,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let program = probe
        .program
        .as_deref()
        .ok_or("devlanes not found on the box")?;
    let command = probe
        .shell
        .command(program, "daemon status --json")
        .ok_or("the box's shell cannot run devlanes")?;
    let options = options(home, env("DEVLANES_TEST_SSH_KEY"));
    let mut ssh = tokio::process::Command::new("ssh");
    ssh.args(["-T", "-o", "BatchMode=yes"]);
    for option in &options.ssh_options {
        ssh.arg("-o").arg(option);
    }
    let output = ssh
        .arg("--")
        .arg(destination())
        .arg(command)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("daemon status: {error}: {stdout}"))?;
    Ok(status["pid"]
        .as_u64()
        .ok_or_else(|| format!("no pid: {status}"))?)
}

/// A Windows box over its own sshd (Windows' OpenSSH Server, with devlanes
/// installed for the user `DEVLANES_TEST_SSH_DEST` logs in as, `localhost`
/// by default): the probe reports Windows and the devlanes it found, a
/// session over the tunnel gets the box's handshake, and the daemon that
/// session's tunnel started is the one still running after every ssh
/// session closed and the transport connected again.
#[tokio::test]
#[ignore = "needs windows sshd"]
async fn windows_sshd_box() -> TestResult {
    tokio::time::timeout(Duration::from_secs(180), async {
        let home = short_home()?;
        let transport = transport(home.path());
        let probe = transport.probe().await?;
        assert_eq!(probe.platform.os, "windows", "{probe:?}");
        assert!(probe.program.is_some(), "{probe:?}");
        let session = Session::connect(&transport, SessionOptions::new("windows-sshd")).await?;
        let label = session.server().to_owned();
        assert!(label.starts_with("devlanes "), "{label}");
        assert!(
            !label.contains("(daemon stops with the session)"),
            "{label}"
        );
        assert!(
            session.box_info().ports_range.is_some(),
            "{:?}",
            session.box_info()
        );
        let first = box_daemon_pid(home.path(), &probe).await?;
        session.close();
        transport.close().await;

        // A new session would start a new daemon if the first had ended
        // with its ssh session.
        let transport = self::transport(home.path());
        let session = Session::connect(&transport, SessionOptions::new("windows-sshd")).await?;
        assert_eq!(box_daemon_pid(home.path(), &probe).await?, first);
        session.close();
        transport.close().await;
        Ok(())
    })
    .await?
}

#[tokio::test]
#[ignore = "needs ssh"]
async fn ssh_mirrors_a_port_and_syncs_a_file() -> TestResult {
    if env("DEVLANES_TEST_SSH_LANE").as_deref() != Some("1") {
        eprintln!("skipping ssh_mirrors_a_port_and_syncs_a_file: no lane set up on the box");
        return Ok(());
    }
    tokio::time::timeout(Duration::from_secs(60), async {
        let home = short_home()?;
        let transport = transport(home.path());
        let session = Session::connect(&transport, SessionOptions::new("ssh-test")).await?;
        // The box's running echo service is in the registry with its port.
        let mut registry = session.registry().subscribe();
        let port = loop {
            let found = registry
                .borrow_and_update()
                .units()
                .find(|unit| unit.key.name == "echo" && unit.state == "ready" && !unit.hidden)
                .and_then(|unit| unit.ports.first().map(|port| port.port));
            if let Some(port) = found {
                break port;
            }
            registry.changed().await?;
        };
        let mut stream = session.open(DialTarget::Port { port }).await?;
        stream.write_all(b"over ssh\n").await?;
        let mut echoed = [0u8; 9];
        stream.read_exact(&mut echoed).await?;
        assert_eq!(&echoed, b"over ssh\n");
        drop(stream);

        // One sync entry, followed from an empty destination.
        let mut list = session
            .subscribe_sync(serde_json::json!({"op": "list"}))
            .await?;
        let entries = match list.next().await {
            Some(Ok(SyncMessage::Manifest(manifest))) => manifest["entries"].clone(),
            other => return Err(format!("no sync list: {other:?}").into()),
        };
        let entry = entries
            .as_array()
            .and_then(|entries| entries.first())
            .ok_or("the box lists no sync entry")?;
        let follow = serde_json::json!({
            "op": "follow",
            "lane": entry["lane"],
            "unit": entry["unit"],
            "index": entry["index"],
        });
        let mut feed = session.subscribe_sync(follow).await?;
        // An empty destination: no manifest batches, just the end marker.
        feed.finish_input().await?;
        let mut contents = Vec::new();
        while let Some(message) = feed.next().await {
            match message? {
                SyncMessage::Chunk {
                    path, data, last, ..
                } if path == "hello.txt" => {
                    contents.extend_from_slice(&data);
                    if last {
                        break;
                    }
                }
                _ => {}
            }
        }
        assert_eq!(contents, b"hello from the box\n");
        session.close();
        transport.close().await;
        Ok(())
    })
    .await?
}
