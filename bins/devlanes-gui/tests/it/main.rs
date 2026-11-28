//! `devlanes-gui` as the daemon and the installers run it, as a separate
//! process. Every child has a timeout and is killed when its test ends.

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use devlanes_gui_client::{ClientOptions, connect};
use devlanes_gui_proto::{PROTOCOL_VERSION, Role, Token, ready::Ready};
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
    let mut command = Command::new(EXE);
    // A process group of its own, so a Ctrl-Break reaches the host alone.
    #[cfg(windows)]
    command.creation_flags(0x0000_0200);
    let mut child = command
        .args([
            "host",
            "--backend",
            "test-pattern",
            "--listen",
            "127.0.0.1:0",
            "--service",
            "acme:desktop",
        ])
        .env(devlanes_gui_proto::ready::TOKEN_ENV, "acme-token")
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

    // The daemon's strict reader takes it, and the whole line shows a field
    // renamed or added by mistake.
    let ready = Ready::parse(&line).expect("a ready line the daemon reads");
    let address: SocketAddr = ready.address;
    let value: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
    assert_eq!(
        value,
        serde_json::json!({
            "ready": 1,
            "devlanes": devlanes_version::VERSION,
            "protocol": PROTOCOL_VERSION,
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
    // Held open: a host whose control connection closes before the unit's
    // identity came stops by itself.
    let (_daemon, _control_events) = timeout(TIMEOUT, connect(address, control))
        .await
        .expect("control hello in time")
        .expect("the token from the environment is accepted");

    drop(client);
    let (status, signalled) = timeout(TIMEOUT, stop(&mut child))
        .await
        .expect("the host exits in time");
    // A signal is a clean stop; a terminated process is not.
    if signalled {
        assert!(status.success(), "{status:?}");
    }
    // The port may be bound again by anything once it is free, so the check
    // is that no GUI host answers there.
    let after = ClientOptions::new(Role::Tool, "acme-test:2", "test");
    assert!(
        timeout(TIMEOUT, connect(address, after))
            .await
            .expect("an answer in time")
            .is_err(),
        "a GUI host still answers on {address}"
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
/// Returns the status and that the stop was a signal.
#[cfg(unix)]
async fn stop(child: &mut tokio::process::Child) -> (std::process::ExitStatus, bool) {
    use rustix::process::{Pid, Signal, kill_process};
    let pid = child
        .id()
        .and_then(|id| Pid::from_raw(id as i32))
        .expect("pid");
    kill_process(pid, Signal::TERM).expect("SIGTERM");
    (child.wait().await.expect("wait"), true)
}

/// Stops the host as the Windows runner does, with Ctrl-Break to its own
/// process group, which leaves the test's console group alone. A test
/// process without a console cannot send one; the host is then terminated
/// and the stop is not a signal.
#[cfg(windows)]
async fn stop(child: &mut tokio::process::Child) -> (std::process::ExitStatus, bool) {
    let signalled = child
        .id()
        .is_some_and(|pid| devlanes_sys_windows::console::ctrl_break_group(pid).is_ok());
    if !signalled {
        child.kill().await.expect("kill");
    }
    (child.wait().await.expect("wait"), signalled)
}
