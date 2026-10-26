//! `devlanes remote connect` to a box reached by a command, and sessions the
//! test opens itself. The box is a second lab on this machine, and the
//! command runs a real `devlanes tunnel --stdio` with that lab's home; on
//! Windows through PowerShell or cmd.exe, as sshd runs it on a Windows box.

use std::ffi::OsString;
use std::time::Duration;

use devlanes_e2e::{Error, Lab, Result, lane, port, unit};
use devlanes_transport::{DialTarget, Session, SessionOptions};
use devlanes_transport_command::{CommandOptions, CommandTransport};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{BUDGET, need};

/// How long the client gets to connect and see the box's ports.
const CONNECT: Duration = Duration::from_secs(60);

/// The `remote ls` entry of `name` in `lab`, or `None` when it lists no such box.
fn host(lab: &Lab, name: &str) -> Result<Option<Value>> {
    let list = lab.json(lab.repo(), &["remote", "ls", "--json"])?;
    Ok(list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["host"] == name))
        .cloned())
}

/// The box lab's `devlanes` binary.
fn devlanes() -> Result<std::path::PathBuf> {
    Ok(devlanes_e2e::bin_dir()?.join(format!("devlanes{}", std::env::consts::EXE_SUFFIX)))
}

/// The `remote.boxes` command that runs `boxed`'s tunnel: `env` sets its
/// home on Unix.
#[cfg(unix)]
fn box_command(boxed: &Lab) -> Result<Value> {
    Ok(json!([
        "env",
        format!("DEVLANES_HOME={}", boxed.home().display()),
        devlanes()?,
        "tunnel",
        "--stdio",
    ]))
}

/// The `remote.boxes` command that runs `boxed`'s tunnel: PowerShell sets its
/// home and runs it as on a Windows box whose sshd uses PowerShell.
#[cfg(windows)]
fn box_command(boxed: &Lab) -> Result<Value> {
    let script = format!(
        "$env:DEVLANES_HOME={}; & {} tunnel --stdio; exit $LASTEXITCODE",
        ps_quote(&boxed.home().to_string_lossy()),
        ps_quote(&devlanes()?.to_string_lossy()),
    );
    Ok(json!([
        devlanes_e2e::system32(r"WindowsPowerShell\v1.0\powershell.exe"),
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]))
}

/// `value` as a PowerShell single-quoted string.
#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A box in `remote.boxes` connects through its command: the client mirrors
/// the box lane's ports, `remote ls` shows the command transport, the box
/// lists the client, and `disconnect` ends the tunnel.
#[test]
fn a_command_box_mirrors_its_lane() -> Result<()> {
    let boxed = Lab::new("two-services", BUDGET)?;
    let started = boxed.json(boxed.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let lanes = boxed.lanes(boxed.repo())?;
    let api = need(
        lane(&lanes, "fixture-two@0")
            .and_then(|lane| unit(lane, "api"))
            .and_then(port),
        "the box's api port",
    )?;

    // Without a devlanes.yaml, so no lane here competes for the box's hostnames.
    let client = Lab::without_config(BUDGET)?;
    let command = box_command(&boxed)?;
    let config = client.home().join("config.yaml");
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str(&format!(
        "remote:\n  boxes:\n    acme:\n      command: {command}\n"
    ));
    std::fs::write(&config, text)?;

    client.json(
        client.repo(),
        &["remote", "connect", "acme", "--background"],
    )?;
    let mirrored = client.eventually("acme's api port in remote ls", CONNECT, || {
        Ok(host(&client, "acme")?.filter(|host| {
            host["state"] == "connected"
                && host["ports"]
                    .as_array()
                    .is_some_and(|ports| ports.iter().any(|entry| entry["port"] == api))
        }))
    })?;
    assert_eq!(mirrored["transport"], "command", "{mirrored}");
    assert_eq!(mirrored["lanes"], 1, "{mirrored}");

    let status = boxed.json(boxed.repo(), &["status", "--json"])?;
    let clients = status["clients"].as_array().cloned().unwrap_or_default();
    assert_eq!(clients.len(), 1, "the box lists one client: {status}");

    client.json(client.repo(), &["remote", "disconnect", "acme"])?;
    boxed.eventually("the box's tunnel to end", CONNECT, || {
        let status = boxed.json(boxed.repo(), &["status", "--json"])?;
        Ok(status.get("clients").is_none().then_some(()))
    })?;
    Ok(())
}

/// Ways to start the box's tunnel, by name: directly, and on Windows through
/// cmd.exe and PowerShell, the shells a Windows box's sshd runs commands with.
fn tunnel_commands() -> Result<Vec<(&'static str, Vec<OsString>)>> {
    let devlanes = devlanes()?;
    let direct: Vec<OsString> = vec![devlanes.clone().into(), "tunnel".into(), "--stdio".into()];
    #[cfg(unix)]
    let commands = vec![("direct", direct)];
    #[cfg(windows)]
    let commands = {
        let mut cmd: Vec<OsString> = vec![
            devlanes_e2e::system32("cmd.exe").into(),
            "/d".into(),
            "/c".into(),
        ];
        cmd.extend(direct.iter().cloned());
        let powershell: Vec<OsString> = vec![
            devlanes_e2e::system32(r"WindowsPowerShell\v1.0\powershell.exe").into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            format!(
                "& {} tunnel --stdio; exit $LASTEXITCODE",
                ps_quote(&devlanes.to_string_lossy())
            )
            .into(),
        ];
        vec![("direct", direct), ("cmd", cmd), ("powershell", powershell)]
    };
    Ok(commands)
}

/// Every byte value, with the line breaks and the Ctrl-Z that text-mode
/// pipes would change.
fn binary_payload() -> Vec<u8> {
    let mut payload: Vec<u8> = (0..=255).collect();
    payload.extend_from_slice(b"\r\n\n\r\x1a\x00\x1a\r\n");
    payload.repeat(48)
}

/// Sends `payload` as one masked binary WebSocket message to the
/// fixture's `/ws` echo over `stream` and returns the echoed message.
async fn websocket_echo(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    payload: &[u8],
) -> Result<Vec<u8>> {
    stream
        .write_all(
            b"GET /ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\r\n",
        )
        .await?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await?);
    }
    if !head.starts_with(b"HTTP/1.1 101") {
        return Err(Error::Unexpected(format!(
            "no WebSocket upgrade: {}",
            String::from_utf8_lossy(&head)
        )));
    }
    let length = u16::try_from(payload.len())
        .map_err(|_| Error::Unexpected("payload over 64 KiB".into()))?;
    let mask = [0x5a, 0xa5, 0x0f, 0xf0];
    let mut frame = vec![0x82, 0x80 | 126];
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    stream.write_all(&frame).await?;
    let first = stream.read_u8().await?;
    let second = stream.read_u8().await?;
    if first != 0x82 || second & 0x80 != 0 {
        return Err(Error::Unexpected(format!(
            "unexpected frame header {first:#04x} {second:#04x}"
        )));
    }
    let size = match second & 0x7f {
        126 => usize::from(stream.read_u16().await?),
        127 => usize::try_from(stream.read_u64().await?)
            .map_err(|_| Error::Unexpected("echo too large".into()))?,
        small => usize::from(small),
    };
    let mut echoed = vec![0; size];
    stream.read_exact(&mut echoed).await?;
    Ok(echoed)
}

/// A session over a real `devlanes tunnel --stdio`, started directly and,
/// on Windows, through cmd.exe and PowerShell: the preamble comes first, the
/// handshake and registry work, and a data stream to the box's api service
/// carries every byte value both ways unchanged.
#[test]
fn a_tunnel_carries_every_byte() -> Result<()> {
    let boxed = Lab::new("two-services", BUDGET)?;
    let started = boxed.json(boxed.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let lanes = boxed.lanes(boxed.repo())?;
    let api = need(
        lane(&lanes, "fixture-two@0")
            .and_then(|lane| unit(lane, "api"))
            .and_then(port),
        "the box's api port",
    )?;
    // The lab's own environment, so the tunnel reaches the lab's daemon.
    let env: Vec<(OsString, OsString)> = boxed
        .command(boxed.repo(), &[])?
        .get_envs()
        .filter_map(|(name, value)| Some((name.to_os_string(), value?.to_os_string())))
        .collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let payload = binary_payload();
    for (name, argv) in tunnel_commands()? {
        let mut options = CommandOptions::new(format!("box via {name}"));
        options.env = env.clone();
        let transport = CommandTransport::new(argv, options);
        let echoed = runtime.block_on(async {
            tokio::time::timeout(CONNECT, async {
                let failed =
                    |error: &dyn std::fmt::Display| Error::Unexpected(format!("{name}: {error}"));
                let session = Session::connect(&transport, SessionOptions::new("e2e"))
                    .await
                    .map_err(|error| failed(&error))?;
                let mut registry = session.registry().subscribe();
                while !registry.borrow_and_update().units().any(|unit| {
                    unit.key.name == "api" && unit.ports.iter().any(|entry| entry.port == api)
                }) {
                    registry.changed().await.map_err(|error| failed(&error))?;
                }
                let mut stream = session
                    .open(DialTarget::Port { port: api })
                    .await
                    .map_err(|error| failed(&error))?;
                let echoed = websocket_echo(&mut stream, &payload).await?;
                session.close();
                Ok::<_, Error>(echoed)
            })
            .await
            .map_err(|_| Error::Unexpected(format!("{name}: no echo within {CONNECT:?}")))?
        })?;
        assert!(echoed == payload, "{name}: the bytes changed on the way");
    }
    Ok(())
}

/// Win32-OpenSSH runs a session's command in a job that ends every process
/// in it when the session closes, and lets them break away. A tunnel started
/// that way, with no daemon running, starts one that leaves the job and
/// outlives the session; in a job that refuses breakaway the daemon stays
/// inside, ends with it, and the tunnel says so.
#[cfg(windows)]
mod session_job {
    use std::ffi::{OsStr, OsString};
    use std::io::{BufRead, BufReader, Read};
    use std::os::windows::io::AsHandle;
    use std::sync::mpsc;
    use std::time::Duration;

    use devlanes_e2e::{Error, Lab, Result};
    use devlanes_sys_windows::job::{Job, JobOptions};
    use devlanes_sys_windows::spawn::{self, Environment, Spawn, Stdio, Stream};

    use crate::BUDGET;

    /// How long the tunnel gets for its preamble and the daemon it starts.
    const START: Duration = Duration::from_secs(60);

    /// What a tunnel that ran in a session job printed: its first stdout
    /// line, and its stderr once the job was closed.
    struct Ran {
        preamble: String,
        stderr: String,
    }

    /// Runs `lab`'s `devlanes tunnel --stdio` inside a kill-on-close job
    /// (allowing breakaway when `breakaway` is set), as sshd runs a session's
    /// command, waits for its preamble and the daemon it starts, then closes
    /// the job as the session's end does.
    fn tunnel_session(lab: &Lab, breakaway: bool) -> Result<Ran> {
        let job = Job::create(JobOptions {
            kill_on_close: true,
            die_on_unhandled_exception: true,
        })?;
        if breakaway {
            job.allow_breakaway()?;
        }
        let command = lab.command(lab.repo(), &["tunnel", "--stdio"])?;
        let program = std::path::PathBuf::from(command.get_program());
        let line: Vec<String> = std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|arg| spawn::quote_arg(&arg.to_string_lossy()))
            .collect();
        let variables: Vec<(OsString, OsString)> = command
            .get_envs()
            .filter_map(|(name, value)| Some((name.to_os_string(), value?.to_os_string())))
            .collect();
        let environment =
            Environment::new(variables).map_err(|error| Error::Setup(error.to_string()))?;
        let (stdin_read, stdin_write) = spawn::pipe()?;
        let (stdout_read, stdout_write) = spawn::pipe()?;
        let (stderr_read, stderr_write) = spawn::pipe()?;
        let child = spawn::spawn(&Spawn {
            program: &program,
            command_line: OsStr::new(&line.join(" ")),
            environment: Some(&environment),
            cwd: Some(lab.repo()),
            stdio: Stdio::Handles {
                stdin: Stream::Handle(stdin_read.as_handle()),
                stdout: Stream::Handle(stdout_write.as_handle()),
                stderr: Stream::Handle(stderr_write.as_handle()),
            },
            inherit: &[],
            job: Some(&job),
            flags: spawn::CreationFlags {
                no_window: true,
                ..spawn::CreationFlags::default()
            },
        })?;
        drop((stdin_read, stdout_write, stderr_write));
        let (sender, preamble) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout_read)
                .read_line(&mut line)
                .map(|_| line);
            let _ = sender.send(read);
        });
        let (sender, stderr) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr_read).read_to_string(&mut text);
            let _ = sender.send(text);
        });
        let preamble = preamble
            .recv_timeout(START)
            .map_err(|_| Error::Unexpected("no preamble from the tunnel".into()))??;
        lab.eventually("the daemon the tunnel started", START, || {
            Ok(lab.daemon_running().then_some(()))
        })?;
        // The tunnel reports a daemon it could not move out right after the
        // daemon answers, before it serves the session.
        std::thread::sleep(Duration::from_millis(500));
        drop(job);
        if child.wait_timeout(START)?.is_none() {
            return Err(Error::Unexpected("the tunnel outlived its job".into()));
        }
        drop(stdin_write);
        let stderr = stderr
            .recv_timeout(START)
            .map_err(|_| Error::Unexpected("the tunnel's stderr stayed open".into()))?;
        Ok(Ran { preamble, stderr })
    }

    /// The daemon a tunnel starts breaks away from a session job that
    /// allows it, as Win32-OpenSSH's does, and keeps running after the
    /// session ends.
    #[test]
    fn a_tunnel_daemon_outlives_the_ssh_session() -> Result<()> {
        let lab = Lab::without_config(BUDGET)?;
        assert!(!lab.daemon_running(), "a daemon ran before the tunnel");
        let ran = tunnel_session(&lab, true)?;
        assert_eq!(ran.preamble, devlanes_transport::TUNNEL_PREAMBLE);
        std::thread::sleep(Duration::from_secs(1));
        assert!(
            lab.daemon_running(),
            "the daemon ended with the session job"
        );
        assert!(!ran.stderr.contains("could not leave"), "{}", ran.stderr);
        Ok(())
    }

    /// In a session job that refuses breakaway the daemon stays inside: the
    /// tunnel says so on stderr, and the daemon ends with the session.
    #[test]
    fn a_tunnel_reports_a_daemon_its_session_job_keeps() -> Result<()> {
        let lab = Lab::without_config(BUDGET)?;
        let ran = tunnel_session(&lab, false)?;
        assert_eq!(ran.preamble, devlanes_transport::TUNNEL_PREAMBLE);
        assert!(
            ran.stderr
                .contains("could not leave this ssh session's job"),
            "{}",
            ran.stderr
        );
        lab.eventually("the daemon to end with the job", START, || {
            Ok((!lab.daemon_running()).then_some(()))
        })?;
        Ok(())
    }
}
