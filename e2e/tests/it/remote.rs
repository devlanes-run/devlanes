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

/// A one-time code from `daemon pair` on `boxed`, as its plain output prints
/// it for scripts: the code alone.
fn pairing_code(boxed: &Lab) -> Result<String> {
    let mut pair = boxed.command(boxed.repo(), &["daemon", "pair"])?;
    pair.env("DEVLANES_OUTPUT", "plain");
    let output = boxed.run(pair, Duration::from_secs(60))?;
    output.expect_code(0)?;
    let code = output.stdout.trim().to_owned();
    if code.len() != 9 || code.as_bytes().get(4) != Some(&b'-') {
        return Err(Error::Unexpected(format!("not a pairing code: {code:?}")));
    }
    Ok(code)
}

/// A box in `remote.boxes` connects through its command once paired with a
/// code from `daemon pair`: the client mirrors the box lane's ports, `remote
/// ls` shows the command transport, and the box lists the client as paired
/// and connected. `daemon revoke` on the box ends the tunnel and leaves the
/// client waiting in `needs_pairing`; a new code connects it again, and
/// `disconnect` ends the tunnel.
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

    let code = pairing_code(&boxed)?;
    client.json(
        client.repo(),
        &["remote", "connect", "acme", "--background", "--code", &code],
    )?;
    let mirrored_api = |host: &Value| {
        host["state"] == "connected"
            && host["ports"]
                .as_array()
                .is_some_and(|ports| ports.iter().any(|entry| entry["port"] == api))
    };
    let mirrored = client.eventually("acme's api port in remote ls", CONNECT, || {
        Ok(host(&client, "acme")?.filter(mirrored_api))
    })?;
    assert_eq!(mirrored["transport"], "command", "{mirrored}");
    assert_eq!(mirrored["lanes"], 1, "{mirrored}");

    let status = boxed.json(boxed.repo(), &["status", "--json"])?;
    let clients = status["clients"].as_array().cloned().unwrap_or_default();
    assert_eq!(clients.len(), 1, "the box lists one client: {status}");
    let id = clients[0]["id"].as_str().unwrap_or_default().to_owned();
    let paired = boxed.json(boxed.repo(), &["daemon", "clients", "--json"])?;
    let entry = paired["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == id.as_str()))
        .cloned()
        .unwrap_or_default();
    assert_eq!(entry["paired"], true, "{paired}");

    let revoked = boxed.json(boxed.repo(), &["daemon", "revoke", &id, "--json"])?;
    assert_eq!(revoked["ended"], 1, "{revoked}");
    let waiting = client.eventually("acme to need pairing", CONNECT, || {
        Ok(host(&client, "acme")?.filter(|host| host["state"] == "needs_pairing"))
    })?;
    assert!(
        waiting["ports"]
            .as_array()
            .is_some_and(|ports| ports.iter().any(|entry| entry["port"] == api)),
        "the mirrored port stays: {waiting}"
    );
    let code = pairing_code(&boxed)?;
    client.json(
        client.repo(),
        &["remote", "connect", "acme", "--background", "--code", &code],
    )?;
    client.eventually("acme to connect again", CONNECT, || {
        Ok(host(&client, "acme")?.filter(mirrored_api))
    })?;

    client.json(client.repo(), &["remote", "disconnect", "acme"])?;
    boxed.eventually("the box's tunnel to end", CONNECT, || {
        let status = boxed.json(boxed.repo(), &["status", "--json"])?;
        Ok(status.get("clients").is_none().then_some(()))
    })?;
    Ok(())
}

/// Ways to start the box's tunnel, by name: directly, and on Windows the way
/// a Windows box's sshd runs the tunnel command (quoted as the SSH transport
/// quotes it) with each default shell installed here: cmd.exe, Windows
/// PowerShell, PowerShell 7 and Git Bash, emulated by fakes in `dir`.
#[cfg_attr(
    unix,
    expect(unused_variables, reason = "Windows puts its fakes there")
)]
fn tunnel_commands(dir: &std::path::Path) -> Result<Vec<(String, Vec<OsString>)>> {
    let devlanes = devlanes()?;
    let direct: Vec<OsString> = vec![devlanes.clone().into(), "tunnel".into(), "--stdio".into()];
    #[cfg_attr(unix, expect(unused_mut, reason = "Windows adds its shells"))]
    let mut commands = vec![("direct".to_owned(), direct)];
    #[cfg(windows)]
    {
        use devlanes_testing::fake_tool::{Behaviour, SshdShell, install};
        use devlanes_transport_ssh::BoxShell;

        for shell in SshdShell::ALL {
            let Some(program) = shell.program() else {
                eprintln!("skipping {shell:?}: not installed here");
                continue;
            };
            let quoting = match shell {
                SshdShell::Cmd => BoxShell::Cmd,
                SshdShell::WindowsPowerShell | SshdShell::Pwsh => BoxShell::PowerShell,
                SshdShell::GitBash => BoxShell::Sh,
            };
            let command = quoting
                .tunnel_command(&devlanes.to_string_lossy())
                .ok_or_else(|| Error::Setup(format!("{shell:?} cannot run {devlanes:?}")))?;
            let sshd = install(
                dir,
                &format!("sshd-{}", shell.kind()),
                &Behaviour::new().run_with(
                    shell.kind(),
                    Some(&program),
                    Vec::<(String, String)>::new(),
                ),
            )?;
            commands.push((shell.kind().to_owned(), vec![sshd.into(), command.into()]));
        }
    }
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
/// on Windows, as sshd starts it through each default shell: the preamble
/// comes first, the handshake and registry work, and a data stream to the
/// box's api service carries every byte value both ways unchanged. The box
/// does not require pairing here: the sessions are the test's own, and
/// `a_command_box_mirrors_its_lane` pairs through the real binaries.
#[test]
fn a_tunnel_carries_every_byte() -> Result<()> {
    let boxed = Lab::new("two-services", BUDGET)?;
    let config = boxed.home().join("config.yaml");
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str("remote:\n  require_pairing: false\n");
    std::fs::write(&config, text)?;
    let started = boxed.json(boxed.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let lanes = boxed.lanes(boxed.repo())?;
    let api = need(
        lane(&lanes, "fixture-two@0")
            .and_then(|lane| unit(lane, "api"))
            .and_then(port),
        "the box's api port",
    )?;
    // Only the lab's own environment, as its commands get it, so the tunnel
    // reaches the lab's daemon: an inherited `XDG_RUNTIME_DIR` would move the
    // socket it looks for on Linux.
    let env: Vec<(OsString, OsString)> = boxed
        .command(boxed.repo(), &[])?
        .get_envs()
        .filter_map(|(name, value)| Some((name.to_os_string(), value?.to_os_string())))
        .collect();
    let inherited: Vec<OsString> = std::env::vars_os().map(|(name, _)| name).collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let payload = binary_payload();
    let fakes = tempfile::tempdir()?;
    for (name, argv) in tunnel_commands(fakes.path())? {
        let mut options = CommandOptions::new(format!("box via {name}"));
        options.env = env.clone();
        // Removed before `env` is set, so this leaves exactly `env`.
        options.env_remove = inherited.clone();
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

    /// A tunnel that ran in a session job and ended: its first stdout line,
    /// its stderr, the daemon it started, and the job, still open.
    struct Ran {
        preamble: String,
        stderr: String,
        daemon: u32,
        job: Job,
    }

    /// Runs `lab`'s `devlanes tunnel --stdio` inside a kill-on-close job
    /// (allowing breakaway when `breakaway` is set), as sshd runs a session's
    /// command. After its preamble the client goes away (stdin closes), so
    /// the tunnel starts the daemon, says what it has to say about it, finds
    /// no client and exits; the job stays open for the caller to close.
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
        drop(stdin_write);
        if child.wait_timeout(START)?.is_none() {
            return Err(Error::Unexpected(
                "the tunnel kept running without a client".into(),
            ));
        }
        let stderr = stderr
            .recv_timeout(START)
            .map_err(|_| Error::Unexpected("the tunnel's stderr stayed open".into()))?;
        // The tunnel waited for the daemon to be ready before it read stdin.
        let daemon = lab
            .daemon_pid()?
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| Error::Unexpected(format!("no daemon after the tunnel: {stderr}")))?;
        Ok(Ran {
            preamble,
            stderr,
            daemon,
            job,
        })
    }

    /// The daemon a tunnel starts breaks away from a session job that
    /// allows it, as Win32-OpenSSH's does: it is not in the job, and keeps
    /// running once the job is closed as the session's end closes it.
    #[test]
    fn a_tunnel_daemon_outlives_the_ssh_session() -> Result<()> {
        let lab = Lab::without_config(BUDGET)?;
        assert!(!lab.daemon_running(), "a daemon ran before the tunnel");
        let ran = tunnel_session(&lab, true)?;
        assert_eq!(ran.preamble, devlanes_transport::TUNNEL_PREAMBLE);
        assert!(
            !ran.stderr.contains("stops when this SSH session ends"),
            "{}",
            ran.stderr
        );
        let members = ran.job.members()?;
        assert!(!members.contains(&ran.daemon), "{members:?}");
        drop(ran.job);
        assert!(
            lab.daemon_running(),
            "the daemon ended with the session job"
        );
        Ok(())
    }

    /// In a session job that refuses breakaway the daemon stays inside: the
    /// tunnel says so on stderr, and the daemon ends when the job is closed.
    #[test]
    fn a_tunnel_reports_a_daemon_its_session_job_keeps() -> Result<()> {
        let lab = Lab::without_config(BUDGET)?;
        let ran = tunnel_session(&lab, false)?;
        assert_eq!(ran.preamble, devlanes_transport::TUNNEL_PREAMBLE);
        assert!(
            ran.stderr.contains("stops when this SSH session ends"),
            "{}",
            ran.stderr
        );
        let members = ran.job.members()?;
        assert!(members.contains(&ran.daemon), "{members:?}");
        drop(ran.job);
        lab.eventually("the daemon to end with the job", START, || {
            Ok((!lab.daemon_running()).then_some(()))
        })?;
        Ok(())
    }
}
