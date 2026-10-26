//! A stand-in for a box command in this crate's tests: it runs its arguments
//! as steps, in order, and exits 0 after the last one.
//!
//! - `--print <text>` and `--stderr <text>`: write text as given.
//! - `--line <text>`: write the text and a line break on stdout.
//! - `--env <name>`: write `<name>=<value>`, or `<name> unset`, and a line break.
//! - `--preamble`: write the tunnel preamble.
//! - `--serve`: serve an empty box over stdio until the client leaves.
//! - `--child`: start `fake-box --sleep 60` and print `child <pid>`.
//! - `--sleep <seconds>`: wait.
//! - `--exit <code>`: exit with that status.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use devlanes_transport::{
    BoxServices, LoopbackDialer, NoSync, RegistrySnapshot, ServeOptions, SharedRegistry,
    TUNNEL_PREAMBLE,
};

/// Runs the steps; a step it does not know exits 2.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    while let Some(step) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match step.as_str() {
            "--print" => out(value().as_bytes()),
            "--line" => out(format!("{}\n", value()).as_bytes()),
            "--env" => {
                let name = value();
                let line = match std::env::var_os(&name) {
                    Some(set) => format!("{name}={}\n", set.to_string_lossy()),
                    None => format!("{name} unset\n"),
                };
                out(line.as_bytes());
            }
            "--stderr" => {
                let _ = std::io::stderr().write_all(value().as_bytes());
            }
            "--preamble" => out(TUNNEL_PREAMBLE.as_bytes()),
            "--serve" => serve().await,
            "--child" => child(),
            "--sleep" => {
                let seconds = value().parse().unwrap_or(60);
                tokio::time::sleep(Duration::from_secs(seconds)).await;
            }
            "--exit" => std::process::exit(value().parse().unwrap_or(1)),
            other => {
                eprintln!("fake-box: unknown step {other}");
                std::process::exit(2);
            }
        }
    }
}

/// Writes `bytes` to stdout at once, before any later step.
fn out(bytes: &[u8]) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// Serves a box without lanes over stdio.
async fn serve() {
    let services = BoxServices {
        registry: Arc::new(SharedRegistry::new(RegistrySnapshot::default(), 4)),
        dialer: Arc::new(LoopbackDialer),
        sync: Arc::new(NoSync),
    };
    let served = devlanes_transport::serve(
        devlanes_transport::stdio(),
        services,
        ServeOptions::new("fake-box"),
    )
    .await;
    if let Err(error) = served {
        eprintln!("fake-box: {error}");
    }
}

/// Starts a sleeping copy of this program and prints its pid.
fn child() {
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(["--sleep", "60"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    });
    match started {
        Ok(child) => out(format!("child {}\n", child.id()).as_bytes()),
        Err(error) => {
            eprintln!("fake-box: {error}");
            std::process::exit(2);
        }
    }
}
