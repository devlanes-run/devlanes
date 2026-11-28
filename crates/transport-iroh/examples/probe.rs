//! Manual network check of the peer-to-peer endpoint, never shipped.
//!
//! ```text
//! cargo build -p devlanes-transport-iroh --example probe
//! probe listen [--home DIR] [--relay URL]... [--no-port-mapping]
//! probe dial ID [--home DIR] [--addr IP:PORT]... [--relay URL]... [--no-port-mapping]
//! ```
//!
//! The build puts the binary at `target/debug/examples/probe` (`probe.exe` on
//! Windows) in the checkout; a LaunchAgent needs that absolute path. From a
//! terminal, `cargo run -p devlanes-transport-iroh --example probe -- listen`
//! builds and runs it in one step.
//!
//! `listen` admits every peer, prints the `dial` command that reaches it and
//! echoes every stream until interrupted. `dial` connects, sends one line and
//! prints the echo and how long the direct path took, or the typed error.
//! Without `--relay` nothing but the given addresses is contacted; n0's DNS
//! is never used. Each step prints one line with the time since the start, so
//! a system prompt (such as macOS's Local Network one) can be matched to the
//! step that caused it. The key lives in `DIR/<command>/p2p-key`, by default
//! under `devlanes-probe` in the temporary directory, so a listener and a
//! dialer on one machine have different ids.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use devlanes_transport::{
    Admission, Alpn, BoxError, PeerAddr, PeerEndpoint, PeerError, PeerId, PeerIncoming,
    TransportError,
};
use devlanes_transport_iroh::{IrohEndpoint, IrohOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the command line asked for.
struct Args {
    command: Command,
    home: PathBuf,
    relays: Vec<String>,
    port_mapping: bool,
}

/// The two roles of the probe.
enum Command {
    Listen,
    Dial { id: PeerId, direct: Vec<SocketAddr> },
}

/// Admits every peer: the probe checks the network, not admission.
struct Everyone;

impl Admission for Everyone {
    /// Admits `_peer` for any protocol.
    fn admit(&self, _peer: &PeerId, _alpn: Alpn) -> bool {
        true
    }
}

/// Prints one step with the seconds since `start`.
fn step(start: Instant, text: impl std::fmt::Display) {
    println!("[{:8.3}s] {text}", start.elapsed().as_secs_f64());
}

/// The usage text, for argument errors.
const USAGE: &str = "usage:
  probe listen [--home DIR] [--relay URL]... [--no-port-mapping]
  probe dial ID [--home DIR] [--addr IP:PORT]... [--relay URL]... [--no-port-mapping]";

/// Reads the command line by hand, which keeps the example free of
/// dependencies the crate does not have.
fn parse(mut words: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let command = words.next().ok_or("no command")?;
    if command == "--help" || command == "-h" {
        return Ok(None);
    }
    let mut id = None;
    let mut direct = Vec::new();
    let mut relays = Vec::new();
    let mut home = std::env::temp_dir().join("devlanes-probe");
    let mut port_mapping = true;
    while let Some(word) = words.next() {
        let mut value = |flag: &str| words.next().ok_or(format!("{flag} needs a value"));
        match word.as_str() {
            "--home" => home = value("--home")?.into(),
            "--relay" => relays.push(value("--relay")?),
            "--addr" => {
                let text = value("--addr")?;
                direct.push(
                    text.parse()
                        .map_err(|error| format!("--addr {text}: {error}"))?,
                );
            }
            "--no-port-mapping" => port_mapping = false,
            "--help" | "-h" => return Ok(None),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            text if id.is_none() => id = Some(text.to_owned()),
            text => return Err(format!("unexpected argument {text}")),
        }
    }
    let command = match command.as_str() {
        "listen" if id.is_none() && direct.is_empty() => Command::Listen,
        "dial" => {
            let id = id.ok_or("dial needs the listener's id")?;
            let id = id.parse().map_err(|error| format!("{id}: {error}"))?;
            Command::Dial { id, direct }
        }
        other => return Err(format!("unknown command {other}")),
    };
    Ok(Some(Args {
        command,
        home,
        relays,
        port_mapping,
    }))
}

/// Runs the probe and maps its outcome to the exit status.
#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args, Instant::now()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

/// Prints why the probe failed, with the `PeerError` variant when there is one.
fn failed(start: Instant, error: &BoxError) {
    let cause = error
        .downcast_ref::<TransportError>()
        .and_then(TransportError::downcast_ref::<PeerError>)
        .or_else(|| error.downcast_ref::<PeerError>());
    match cause {
        Some(cause) => step(start, format_args!("failed: {cause:?}: {cause}")),
        None => step(start, format_args!("failed: {error}")),
    }
}

/// Builds the endpoint for the command, runs it and closes the endpoint,
/// printing the failure first when there is one.
async fn run(args: Args, start: Instant) -> Result<(), ()> {
    let role = match args.command {
        Command::Listen => "listen",
        Command::Dial { .. } => "dial",
    };
    let endpoint = IrohEndpoint::new(IrohOptions {
        key_file: args.home.join(role).join("p2p-key"),
        relays: args.relays.clone(),
        address_lookup: false,
        port_mapping: args.port_mapping,
    });
    let outcome = match endpoint.id().await {
        Ok(id) => {
            step(
                start,
                format_args!(
                    "key loaded: id {id}, relays {:?}, port mapping {}",
                    args.relays, args.port_mapping
                ),
            );
            match args.command {
                Command::Listen => listen(&endpoint, start).await,
                Command::Dial { id, direct } => {
                    let relay = args.relays.first().cloned();
                    dial(&endpoint, PeerAddr { id, relay, direct }, start).await
                }
            }
        }
        Err(error) => Err(error.into()),
    };
    if let Err(error) = &outcome {
        failed(start, error);
    }
    step(start, "closing the endpoint");
    endpoint.close().await;
    step(start, "closed");
    outcome.map_err(|_| ())
}

/// Listens, prints how to dial this endpoint and echoes streams until Ctrl-C.
async fn listen(endpoint: &IrohEndpoint, start: Instant) -> Result<(), BoxError> {
    step(start, "binding and listening");
    endpoint.listen(Some(Arc::new(Everyone))).await?;
    let addr = endpoint.addr().await?;
    step(
        start,
        format_args!(
            "listening: relay {:?}, addresses {:?}",
            addr.relay, addr.direct
        ),
    );
    let mut command = format!("probe dial {}", addr.id);
    for direct in &addr.direct {
        command.push_str(&format!(" --addr {direct}"));
    }
    if let Some(relay) = &addr.relay {
        command.push_str(&format!(" --relay {relay}"));
    }
    step(start, format_args!("dial with: {command}"));
    loop {
        let incoming = tokio::select! {
            incoming = endpoint.accept() => incoming,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        let Some(incoming) = incoming else {
            return Ok(());
        };
        step(
            start,
            format_args!("connection from {} for {:?}", incoming.peer, incoming.alpn),
        );
        tokio::spawn(echo(incoming, start));
    }
}

/// Echoes every stream of one connection until it closes.
async fn echo(incoming: PeerIncoming, start: Instant) {
    let peer = incoming.peer.short();
    let mut connection = incoming.connection;
    while let Some(stream) = connection.accept().await {
        tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(stream);
            let echoed = tokio::io::copy(&mut read, &mut write).await;
            let _ = write.shutdown().await;
            if let Err(error) = echoed {
                step(start, format_args!("echo failed: {error}"));
            }
        });
    }
    step(start, format_args!("connection from {peer} closed"));
}

/// Binds, then dials `addr`, sends one line and prints its echo. Binding is
/// its own step, so a prompt it raises is told apart from one the dial to a
/// LAN address raises.
async fn dial(endpoint: &IrohEndpoint, addr: PeerAddr, start: Instant) -> Result<(), BoxError> {
    step(start, "binding");
    let own = endpoint.addr().await?;
    step(
        start,
        format_args!("bound: relay {:?}, addresses {:?}", own.relay, own.direct),
    );
    step(
        start,
        format_args!(
            "dialing {} at {:?} through relay {:?}",
            addr.id, addr.direct, addr.relay
        ),
    );
    let dialed = Instant::now();
    let connection = endpoint.connect(&addr, Alpn::Tunnel).await?;
    step(
        start,
        format_args!(
            "direct path selected {:.1} ms after the dial",
            dialed.elapsed().as_secs_f64() * 1000.0
        ),
    );
    let mut stream = connection.open().await?;
    let line = format!("probe from {}\n", endpoint.id().await?.short());
    stream.write_all(line.as_bytes()).await?;
    stream.shutdown().await?;
    let mut echo = String::new();
    stream.read_to_string(&mut echo).await?;
    step(start, format_args!("echo: {}", echo.trim_end()));
    connection.shutdown();
    connection.closed().await;
    Ok(())
}
