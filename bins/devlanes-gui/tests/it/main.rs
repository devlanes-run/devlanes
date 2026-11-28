//! `devlanes-gui` as the daemon and the installers run it, as a separate
//! process. Every child has a timeout and is killed when its test ends.

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use devlanes_gui_client::{ClientOptions, connect};
use devlanes_gui_proto::{Role, Token};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

/// The binary under test.
const EXE: &str = env!("CARGO_BIN_EXE_devlanes-gui");

/// The longest any step of a test waits for the child.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Installers and the updater read `<name> <version>` from the first two
/// words of `version`.
#[tokio::test]
async fn version_names_the_binary_and_the_release() {
    let output = timeout(TIMEOUT, Command::new(EXE).arg("version").output())
        .await
        .expect("version in time")
        .expect("version runs");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    let words: Vec<&str> = text.split_whitespace().take(2).collect();
    assert_eq!(words, ["devlanes-gui", devlanes_version::VERSION], "{text}");
}

/// The daemon's path: the host prints one ready line once it listens, serves
/// the test pattern to a tool, takes the control token from its environment,
/// and is gone once stopped.
#[tokio::test]
async fn the_host_serves_after_its_ready_line_until_stopped() {
    let mut child = Command::new(EXE)
        .args([
            "host",
            "--backend",
            "test-pattern",
            "--listen",
            "127.0.0.1:0",
            "--service",
            "acme:desktop",
        ])
        .env("DEVLANES_GUI_TOKEN", "acme-token")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("host starts");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let line = timeout(TIMEOUT, lines.next_line())
        .await
        .expect("ready line in time")
        .expect("stdout reads")
        .expect("a ready line");

    // The whole line, so a field renamed or added by mistake shows here.
    let ready: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
    let address: SocketAddr = ready["address"]
        .as_str()
        .and_then(|text| text.parse().ok())
        .expect("an address");
    assert!(address.ip().is_loopback() && address.port() != 0, "{line}");
    assert_eq!(
        ready,
        serde_json::json!({
            "ready": 1,
            "address": address.to_string(),
            "set": {},
            "remove": [],
            "real_desktop": false,
        }),
        "{line}"
    );

    let tool = ClientOptions::new(Role::Tool, "acme-test:1", "test");
    let (client, _events) = timeout(TIMEOUT, connect(address, tool))
        .await
        .expect("hello in time")
        .expect("welcomed");
    assert_eq!(client.welcome().service, "acme:desktop");
    let shot = timeout(TIMEOUT, client.screenshot(None))
        .await
        .expect("screenshot in time")
        .expect("screenshot");
    assert!(devlanes_gui_host::pattern::shows_grid(&shot.frame));
    let control = ClientOptions::new(Role::Control, "acme-daemon:1", "daemon")
        .with_token(Token::new("acme-token"));
    timeout(TIMEOUT, connect(address, control))
        .await
        .expect("control hello in time")
        .expect("the token from the environment is accepted");

    let status = timeout(TIMEOUT, stop(&mut child))
        .await
        .expect("the host exits in time");
    // SIGTERM is a clean stop; Windows tests can only terminate the process.
    if cfg!(unix) {
        assert!(status.success(), "{status:?}");
    }
    assert!(
        tokio::net::TcpStream::connect(address).await.is_err(),
        "{address} still accepts"
    );
}

/// The host listens on loopback only and names the backends it has; either
/// mistake fails before anything listens, with one line on stderr.
#[tokio::test]
async fn the_host_refuses_other_addresses_and_unknown_backends() {
    for (backend, listen, says) in [
        (
            "test-pattern",
            "0.0.0.0:0",
            "0.0.0.0:0 is not a loopback address",
        ),
        ("test-pattern", "[::]:0", "[::]:0 is not a loopback address"),
        ("acme", "127.0.0.1:0", "unknown GUI backend \"acme\""),
    ] {
        let run = Command::new(EXE)
            .args(["host", "--backend", backend, "--listen", listen])
            .args(["--service", "acme:desktop"])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = timeout(TIMEOUT, run)
            .await
            .expect("exits in time")
            .expect("runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{listen}: {stderr}");
        assert!(output.stdout.is_empty(), "{listen}: no ready line");
        assert!(
            stderr.starts_with("devlanes-gui: ") && stderr.contains(says),
            "{stderr}"
        );
    }
}

/// Stops the host as the runner does on Unix, with SIGTERM, and waits.
#[cfg(unix)]
async fn stop(child: &mut tokio::process::Child) -> std::process::ExitStatus {
    use rustix::process::{Pid, Signal, kill_process};
    let pid = child
        .id()
        .and_then(|id| Pid::from_raw(id as i32))
        .expect("pid");
    kill_process(pid, Signal::TERM).expect("SIGTERM");
    child.wait().await.expect("wait")
}

/// Terminates the host: a console event would reach the test runner too.
#[cfg(windows)]
async fn stop(child: &mut tokio::process::Child) -> std::process::ExitStatus {
    child.kill().await.expect("kill");
    child.wait().await.expect("wait")
}
