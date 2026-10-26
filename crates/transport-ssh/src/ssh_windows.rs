//! Windows: one `ssh.exe` per session.
//!
//! Win32-OpenSSH has no ControlMaster (it would need descriptor passing over
//! Unix sockets), so nothing outlives a session and each connect
//! authenticates again. To keep that to one authentication per connect, the
//! probe and the tunnel share the process: the box prints the probe and a
//! sentinel line, then execs `devlanes tunnel --stdio`, and everything after
//! the sentinel is the tunnel's: its preamble line, then mux traffic. A
//! standalone [`SshTransport::probe`] is a run of its own.
//!
//! Both run through the command transport, so every `ssh.exe` starts inside a
//! kill-on-close job: whatever it starts (a `ProxyCommand`) ends with the
//! session, and with the daemon if the daemon dies. The sentinel is the
//! session's ready line: until it comes, the wait is the authentication limit.
//!
//! The public types mirror the Unix module's.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{BoxDuplex, PreambleError, Prefixed, Transport, TransportError};
use devlanes_transport_command::{
    CommandError, CommandOptions, CommandStream, CommandTransport, Exit, Output, Ready,
};
use tokio_util::sync::CancellationToken;

use crate::askpass::{AskpassConfig, AskpassServer, SOCKET_ENV};
use crate::probe::{self, INSTALL_DIR, RemoteProbe, SETUP_GUIDE};
use crate::sh_quote;

/// Bytes of ssh's stderr kept for error messages.
const STDERR_TAIL: usize = 4096;

/// ssh's own exit status for connection and authentication failures.
const SSH_FAILED: i32 = 255;

/// Settings for [`SshTransport`].
#[derive(Clone)]
pub struct SshOptions {
    /// Kept for the Unix API, where control sockets live there; Windows
    /// writes nothing to disk.
    pub control_dir: PathBuf,
    /// Local `ssh` binary. The bare name `ssh` means the system OpenSSH client
    /// (`<System32>\OpenSSH\ssh.exe`), else the first `ssh.exe` on `PATH`.
    pub ssh_program: PathBuf,
    /// Remote command name, looked up on PATH and then in `$HOME/.devlanes/bin`.
    pub remote_program: String,
    /// TCP connect limit passed as `ConnectTimeout`.
    pub connect_timeout: Duration,
    /// Limit for one session's start, including time spent answering prompts.
    pub auth_timeout: Duration,
    /// Extra `-o` options for ssh, such as `Port=2222`.
    pub ssh_options: Vec<String>,
    /// Prompt relay; without it ssh runs with `BatchMode=yes`.
    pub askpass: Option<AskpassConfig>,
    /// The box connected before, such as one a restarted daemon restores:
    /// failures are classified as after a first successful tunnel. Only
    /// permanent non-auth failures then stop the link; a box that is gone
    /// keeps being retried until the caller stops the link.
    pub connected_before: bool,
}

impl SshOptions {
    /// Defaults, with `control_dir` under `<devlanes_home>/run/ssh` as on Unix.
    pub fn new(devlanes_home: impl AsRef<Path>) -> Self {
        Self {
            control_dir: devlanes_home.as_ref().join("run").join("ssh"),
            ssh_program: PathBuf::from("ssh"),
            remote_program: "devlanes".to_owned(),
            connect_timeout: Duration::from_secs(15),
            auth_timeout: Duration::from_secs(120),
            ssh_options: Vec::new(),
            askpass: None,
            connected_before: false,
        }
    }
}

/// SSH transport failures.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// Local process I/O failed.
    #[error("ssh I/O: {0}")]
    Io(#[from] io::Error),
    /// No OpenSSH client was found.
    #[error(
        "no ssh client found ({0}); add the Windows optional feature \"OpenSSH Client\" or put ssh.exe on PATH"
    )]
    ProgramMissing(PathBuf),
    /// ssh did not connect or authenticate; `log` holds its diagnostics.
    #[error("ssh to {destination} failed ({status}): {log}")]
    Master {
        /// The ssh destination.
        destination: String,
        /// Exit status description.
        status: String,
        /// Tail of ssh's stderr.
        log: String,
        /// Whether retrying cannot help, such as rejected credentials on a
        /// first connection.
        permanent: bool,
        /// ssh rejected the credentials or the host key: permanent on a first
        /// connection, and a person has to sign in again after one.
        auth: bool,
    },
    /// ssh did not finish authenticating in time.
    #[error("ssh to {0} did not authenticate in time")]
    AuthTimeout(String),
    /// The box answered the probe with unexpected output.
    #[error("unexpected probe output from the box: {0:?}")]
    Probe(String),
    /// devlanes is neither on the box's `PATH` nor in `~/.devlanes/bin`.
    #[error(
        "devlanes is not installed on {destination} (not on the ssh PATH or in ~/{INSTALL_DIR}); \
         install it there, see {SETUP_GUIDE}"
    )]
    NotInstalled {
        /// The ssh destination.
        destination: String,
        /// Set before the first tunnel opened; afterwards the binary may only
        /// be missing for a moment, such as during an upgrade on the box.
        permanent: bool,
    },
    /// The tunnel failed: its exit status and stderr, a stopped box daemon
    /// ([`CommandError::DaemonStopped`]) or output without a preamble line of
    /// this protocol ([`CommandError::Preamble`]).
    #[error(transparent)]
    Tunnel(Box<CommandError>),
}

impl SshError {
    /// Reports whether reconnecting without user action is pointless.
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Master { permanent, .. } | Self::NotInstalled { permanent, .. } => *permanent,
            Self::Tunnel(error) => error.is_permanent(),
            Self::ProgramMissing(_) => true,
            Self::Io(_) | Self::AuthTimeout(_) | Self::Probe(_) => false,
        }
    }

    /// Reports whether ssh rejected the credentials or the host key.
    pub fn is_auth_failure(&self) -> bool {
        matches!(self, Self::Master { auth: true, .. })
    }
}

impl From<SshError> for TransportError {
    /// Rejected credentials stop a first connection and wait for a sign-in
    /// after one.
    fn from(error: SshError) -> Self {
        match (error.is_permanent(), error.is_auth_failure()) {
            (true, true) => TransportError::auth_failed(error),
            (true, false) => TransportError::permanent(error),
            (false, true) => TransportError::needs_auth(error),
            (false, false) => TransportError::transient(error),
        }
    }
}

/// [`Transport`] that runs `devlanes tunnel --stdio` on a box over system OpenSSH.
///
/// The user's `%USERPROFILE%\.ssh\config`, agent, ProxyJump and certificates
/// apply because the real `ssh.exe` does the work.
pub struct SshTransport {
    destination: String,
    options: SshOptions,
    /// What the last session or probe found on the box.
    probe: Mutex<Option<RemoteProbe>>,
    /// Cancelled by [`SshTransport::close`] and on drop, ending every session.
    sessions: Mutex<CancellationToken>,
    /// Set after the first tunnel opened, or from the start with
    /// [`SshOptions::connected_before`]: from then on a failing DNS lookup is a
    /// network blip (sleep, Wi-Fi change), not a misspelt host, and rejected
    /// credentials need a person to sign in again.
    connected: AtomicBool,
}

impl SshTransport {
    /// Targets `destination`, any name `ssh` accepts.
    pub fn new(destination: impl Into<String>, options: SshOptions) -> Self {
        Self {
            destination: destination.into(),
            connected: AtomicBool::new(options.connected_before),
            options,
            probe: Mutex::new(None),
            sessions: Mutex::new(CancellationToken::new()),
        }
    }

    /// The ssh destination.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Reports the box platform and devlanes location: what the last session
    /// found, else from a probe-only ssh run.
    pub async fn probe(&self) -> Result<RemoteProbe, SshError> {
        if let Some(probe) = self.remembered() {
            return Ok(probe);
        }
        let script = probe::probe_script(&self.options.remote_program);
        let askpass = self.askpass().await?;
        let argv = self.argv(&script, askpass.is_none()).await?;
        let options = self.command_options(askpass.as_ref());
        let output = devlanes_transport_command::run(argv, &options, self.options.auth_timeout)
            .await
            .map_err(|error| self.spawn_failed(error))?;
        drop(askpass);
        let output = output.ok_or_else(|| SshError::AuthTimeout(self.destination.clone()))?;
        if let Some(error) = self.ssh_failed(&output) {
            return Err(error);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let probe =
            probe::parse_probe(&stdout).ok_or_else(|| SshError::Probe(stdout.into_owned()))?;
        self.remember(Some(probe.clone()));
        Ok(probe)
    }

    /// Ends every session this transport started.
    pub async fn close(&self) {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        sessions.cancel();
        *sessions = CancellationToken::new();
    }

    /// Runs the session script (probe, sentinel, tunnel) and reads up to the
    /// tunnel's preamble. A box without devlanes ends the session after the
    /// probe with [`SshError::NotInstalled`].
    async fn open(&self) -> Result<Prefixed<CommandStream>, SshError> {
        let token = crate::random_token()?;
        let start = format!("devlanes-probe-{token}");
        let end = format!("devlanes-tunnel-ready-{token}");
        let script = probe::session_script(&self.options.remote_program, &start, &end);
        let askpass = self.askpass().await?;
        let argv = self.argv(&script, askpass.is_none()).await?;
        let mut options = self.command_options(askpass.as_ref());
        options.ready = Some(Ready {
            line: end.clone(),
            wait: self.options.auth_timeout,
        });
        let closed = options.stop.clone();
        let opened = CommandTransport::new(argv, options).open().await;
        // Prompts end with authentication; the box has answered by now.
        drop(askpass);
        let opened = match opened {
            Ok(opened) => opened,
            // A `close` while ssh authenticated ended this session too.
            Err(_) if closed.is_cancelled() => {
                return Err(SshError::Io(io::Error::other(
                    "the SSH transport was closed",
                )));
            }
            Err(error) => return Err(self.session_failed(error, &start, &end)),
        };
        let lines = lines(&opened.header);
        let probe = probe::parse_session_header(&lines, &start)
            .ok_or_else(|| SshError::Probe(lines.join("\n")))?;
        let missing = probe.program.is_none();
        self.remember(Some(probe));
        if missing {
            return Err(self.not_installed());
        }
        self.connected.store(true, Ordering::Relaxed);
        Ok(opened.stream)
    }

    /// The error for a session that failed before its preamble. After the
    /// sentinel the probe ran: a box without devlanes ends there, and
    /// anything else is the tunnel's failure. Before it, the failure is ssh's
    /// or the box shell's; see [`SshTransport::ended_early`].
    fn session_failed(&self, error: CommandError, start: &str, end: &str) -> SshError {
        let lines = lines(error.output());
        if let Some(at) = lines.iter().position(|line| line == end) {
            let Some(probe) = probe::parse_session_header(&lines[..at], start) else {
                return SshError::Probe(lines[..at].join("\n"));
            };
            let missing = probe.program.is_none();
            self.remember(Some(probe));
            return if missing {
                self.not_installed()
            } else {
                SshError::Tunnel(Box::new(error))
            };
        }
        match error {
            CommandError::Spawn { .. } | CommandError::Empty { .. } => self.spawn_failed(error),
            CommandError::NotReady { .. } => SshError::AuthTimeout(self.destination.clone()),
            CommandError::Exited { exit, output, .. } => {
                self.ended_early(&exit, !output.is_empty())
            }
            CommandError::Preamble {
                error: PreambleError::Ended { .. },
                stderr,
                ..
            } => SshError::Probe(format!(
                "the box ended the session before the probe finished: {stderr}"
            )),
            CommandError::Preamble {
                error: PreambleError::TooLong { .. },
                ..
            } => SshError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "the box printed too much before the probe finished",
            )),
            other => SshError::Tunnel(Box::new(other)),
        }
    }

    /// The error for a session whose output ended before the sentinel. Exit
    /// status 255 is ssh's own failure, classified from its stderr, unless the
    /// box `printed` something: ssh had authenticated then, and the 255 came
    /// from the box (its command exiting 255 or dying by a signal). Any other
    /// end came from the box too. ssh's stderr also carries the box's, so a
    /// shell startup file printing "Permission denied" or a missing `sh` (exit
    /// 127) must not read as rejected credentials, which would stop
    /// reconnecting for good; those are temporary errors carrying the stderr
    /// tail.
    fn ended_early(&self, exit: &Exit, printed: bool) -> SshError {
        let status = match &exit.status {
            Ok(status) if status.code() == Some(SSH_FAILED) && !printed => {
                return master_error(&self.destination, *status, &exit.stderr, self.first());
            }
            Ok(status) => status.to_string(),
            Err(error) => error.clone(),
        };
        SshError::Master {
            destination: self.destination.clone(),
            status,
            log: tail(exit.stderr.as_bytes()),
            permanent: false,
            auth: false,
        }
    }

    /// A command transport error from starting `ssh.exe`: a missing client
    /// names the optional feature.
    fn spawn_failed(&self, error: CommandError) -> SshError {
        match error {
            CommandError::Spawn {
                program, source, ..
            } if source.kind() == io::ErrorKind::NotFound => {
                SshError::ProgramMissing(PathBuf::from(program))
            }
            CommandError::Spawn { source, .. } => SshError::Io(source),
            other => SshError::Tunnel(Box::new(other)),
        }
    }

    /// A prompt relay for one ssh run, when prompts are relayed at all.
    async fn askpass(&self) -> Result<Option<AskpassServer>, SshError> {
        match &self.options.askpass {
            Some(config) => Ok(Some(AskpassServer::start(config, &self.destination).await?)),
            None => Ok(None),
        }
    }

    /// Options for one ssh run: messages name the box and the tunnel, the
    /// relay's environment replaces any inherited one, and [`SshTransport::close`]
    /// ends the run.
    fn command_options(&self, askpass: Option<&AskpassServer>) -> CommandOptions {
        let mut options = CommandOptions::new(&self.destination);
        options.what = crate::TUNNEL.to_owned();
        options.preamble_wait = crate::PREAMBLE_WAIT;
        options.connected_before = !self.first();
        options.env = askpass
            .map(AskpassServer::env)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| (OsString::from(name), value))
            .collect();
        options.env_remove = vec![OsString::from(SOCKET_ENV)];
        options.stop = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .child_token();
        options
    }

    /// The ssh run of `script` with `sh -c` on the box, ssh found as
    /// [`locate_program`] does.
    async fn argv(&self, script: &str, batch: bool) -> Result<Vec<String>, SshError> {
        let requested = self.options.ssh_program.clone();
        let program = tokio::task::spawn_blocking(move || locate_program(&requested))
            .await
            .map_err(io::Error::other)?
            .map_err(SshError::ProgramMissing)?;
        Ok(self.command_argv(&program, script, batch))
    }

    /// The whole command: `ssh -T` with keepalives, the connect limit, the
    /// extra options, batch mode without a relay, `--`, the destination and
    /// the remote command.
    fn command_argv(&self, program: &Path, script: &str, batch: bool) -> Vec<String> {
        let mut args: Vec<String> = vec![
            program.to_string_lossy().into_owned(),
            // No terminal on the box: the tunnel speaks binary over stdio.
            "-T".to_owned(),
            "-o".to_owned(),
            "ServerAliveInterval=15".to_owned(),
            "-o".to_owned(),
            "ServerAliveCountMax=3".to_owned(),
            "-o".to_owned(),
            format!(
                "ConnectTimeout={}",
                self.options.connect_timeout.as_secs().max(1)
            ),
        ];
        for option in &self.options.ssh_options {
            args.push("-o".to_owned());
            args.push(option.clone());
        }
        if batch {
            args.push("-o".to_owned());
            args.push("BatchMode=yes".to_owned());
        }
        // `--` keeps a destination starting with `-` from being parsed as an option.
        args.push("--".to_owned());
        args.push(self.destination.clone());
        // ssh joins its remaining arguments into one command for the login
        // shell, so the script travels as one quoted word.
        args.push(format!("sh -c {}", sh_quote(script)));
        args
    }

    /// ssh's own failure in a finished run, classified from its stderr. A run
    /// in which the box printed something had authenticated, so its 255 is the
    /// script's.
    fn ssh_failed(&self, output: &Output) -> Option<SshError> {
        (output.status.code() == Some(SSH_FAILED) && output.stdout.is_empty()).then(|| {
            master_error(
                &self.destination,
                output.status,
                &String::from_utf8_lossy(&output.stderr),
                self.first(),
            )
        })
    }

    /// Whether no tunnel has opened yet.
    fn first(&self) -> bool {
        !self.connected.load(Ordering::Relaxed)
    }

    /// [`SshError::NotInstalled`] for this box: permanent until a tunnel once
    /// opened, like a host name that does not resolve.
    fn not_installed(&self) -> SshError {
        SshError::NotInstalled {
            destination: self.destination.clone(),
            permanent: self.first(),
        }
    }

    /// The probe the last run found.
    fn remembered(&self) -> Option<RemoteProbe> {
        self.probe
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Records (or forgets) what the box reported.
    fn remember(&self, probe: Option<RemoteProbe>) {
        *self.probe.lock().unwrap_or_else(PoisonError::into_inner) = probe;
    }
}

impl Drop for SshTransport {
    /// Ends the sessions, as stopping the Unix master does.
    fn drop(&mut self) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancel();
    }
}

#[async_trait]
impl Transport for SshTransport {
    async fn connect(&self) -> Result<BoxDuplex, TransportError> {
        Ok(Box::new(self.open().await?))
    }
}

/// The lines of what the box printed, without their line breaks.
fn lines(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect()
}

/// Resolves a bare `ssh` to the system OpenSSH client
/// (`<System32>\OpenSSH\ssh.exe`, found through the API rather than
/// `%SystemRoot%`, which the environment controls), else the first match on
/// `PATH`. Git for Windows ships an MSYS `ssh` that also works, but it reads
/// its own `/etc/ssh` and does not use the Windows agent service, so the
/// system client goes first. Paths with a directory are used as given. `Err`
/// carries the name looked for when nothing matched: a bare name is never
/// started, since `CreateProcessW` would look for it in the working
/// directory. Blocking.
fn locate_program(program: &Path) -> Result<PathBuf, PathBuf> {
    if program
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Ok(program.to_path_buf());
    }
    let name = if program.extension().is_none() {
        program.with_extension("exe")
    } else {
        program.to_path_buf()
    };
    let system = devlanes_sys_windows::file::system_directory()
        .ok()
        .map(|system| system.join("OpenSSH"));
    let path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    system
        .into_iter()
        .chain(path.into_iter().filter(|dir| dir.is_absolute()))
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
        .ok_or(name)
}

/// ssh's wording for rejected credentials, as in `user@box: Permission denied
/// (publickey).`. The log is ssh's stderr, which also carries the box's,
/// where a bare "Permission denied" names a file.
const DENIED: &str = "Permission denied (";

/// Classifies a failed connection from ssh's stderr.
///
/// A name that does not resolve is permanent only before the first successful
/// connection (a typo); afterwards it is a network change that will pass.
/// Rejected credentials and host key failures are permanent before the first
/// connection and need a person to sign in again after it.
fn master_error(destination: &str, status: ExitStatus, log: &str, first: bool) -> SshError {
    // These need new credentials or known_hosts changes, not time.
    const AUTH: [&str; 3] = [
        DENIED,
        "Host key verification failed",
        "REMOTE HOST IDENTIFICATION HAS CHANGED",
    ];
    let log = log.trim();
    let auth = AUTH.iter().any(|needle| log.contains(needle));
    SshError::Master {
        destination: destination.to_owned(),
        status: status.to_string(),
        permanent: first && (auth || log.contains("Could not resolve hostname")),
        auth,
        log: tail(log.as_bytes()),
    }
}

/// Returns the last [`STDERR_TAIL`] bytes as lossy UTF-8.
fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_TAIL);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use devlanes_testing::fake_tool::{Behaviour, install};
    use std::sync::Arc;

    use super::*;
    use crate::askpass::{AskpassPrompt, PromptRelay};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct Never;

    #[async_trait]
    impl PromptRelay for Never {
        async fn prompt(&self, _prompt: AskpassPrompt) -> Option<String> {
            None
        }
    }

    /// A transport whose ssh is a fake that prints `stderr` and exits with `code`.
    fn fake(
        dir: &Path,
        stderr: &str,
        code: u8,
    ) -> Result<SshTransport, Box<dyn std::error::Error>> {
        let ssh = install(
            dir,
            "ssh",
            &Behaviour::new()
                .record(dir.join("args"))
                .stderr(stderr)
                .exit(code),
        )?;
        let mut options = SshOptions::new(dir);
        options.ssh_program = ssh;
        Ok(SshTransport::new("box", options))
    }

    /// The arguments the fake recorded on its first run.
    fn recorded(dir: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(dir.join("args"))?;
        let first = text.lines().next().ok_or("the fake recorded nothing")?;
        Ok(serde_json::from_str(first)?)
    }

    /// Rejected credentials stop reconnecting. The run carries the askpass
    /// environment and the session script after `--` and the destination.
    #[tokio::test]
    async fn rejected_credentials_are_permanent_and_askpass_env_is_set() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let mut transport = fake(
                dir.path(),
                "require={env:SSH_ASKPASS_REQUIRE} relay={env:DEVLANES_ASKPASS_SOCKET} \
                 askpass={env:SSH_ASKPASS}\nuser@box: Permission denied (publickey).\n",
                255,
            )?;
            transport.options.askpass = Some(AskpassConfig {
                exe: PathBuf::from(r"C:\acme\devlanes.exe"),
                relay: Arc::new(Never),
            });
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(error.is_permanent(), "{error}");
            let message = error.to_string();
            assert!(
                error.into_needs_auth().is_needs_auth(),
                "a caller whose prompt went unanswered waits for a sign-in"
            );
            assert!(message.contains("Permission denied"), "{message}");
            assert!(message.contains("require=force"), "{message}");
            assert!(
                message.contains(r"relay=\\.\pipe\devlanes-askpass-"),
                "{message}"
            );
            assert!(
                message.contains(r"askpass=C:\acme\devlanes.exe"),
                "{message}"
            );
            let args = recorded(dir.path())?;
            let at = args
                .iter()
                .position(|arg| arg == "--")
                .ok_or("no -- before the destination")?;
            assert_eq!(args[at + 1], "box");
            assert!(args[at + 2].starts_with("sh -c 'printf "), "{args:?}");
            assert_eq!(args.len(), at + 3, "{args:?}");
            assert!(args.contains(&"-T".to_owned()), "{args:?}");
            assert!(!args.contains(&"BatchMode=yes".to_owned()), "{args:?}");
            Ok(())
        })
        .await?
    }

    /// Network failures keep the link retrying; without askpass ssh runs in
    /// batch mode and gets no relay.
    #[tokio::test]
    async fn network_failures_are_transient() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let transport = fake(
                dir.path(),
                "relay=[{env:DEVLANES_ASKPASS_SOCKET}]\n\
                 ssh: connect to host box port 22: Connection refused\n",
                255,
            )?;
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(!error.is_permanent(), "{error}");
            assert!(error.to_string().contains("relay=[]"), "{error}");
            assert!(recorded(dir.path())?.contains(&"BatchMode=yes".to_owned()));
            Ok(())
        })
        .await?
    }

    /// A session that ends with a status other than ssh's own 255 failed on the
    /// box, even when the box's stderr (which ssh passes on) says "Permission
    /// denied": the link keeps retrying and the error carries that stderr.
    #[tokio::test]
    async fn box_failures_before_the_tunnel_are_transient() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            for code in [1, 127] {
                let run = dir.path().join(code.to_string());
                std::fs::create_dir(&run)?;
                let transport = fake(
                    &run,
                    "bash: /home/acme/.bashrc: Permission denied\nsh: devlanes: not found\n",
                    code,
                )?;
                let Err(error) = transport.connect().await else {
                    panic!("fake ssh unexpectedly connected");
                };
                assert!(!error.is_permanent(), "exit {code}: {error}");
                let message = error.to_string();
                assert!(message.contains(".bashrc: Permission denied"), "{message}");
                assert!(message.contains(&code.to_string()), "{message}");
            }
            Ok(())
        })
        .await?
    }

    /// Exit 255 is ssh's own only while the box printed nothing, and even then
    /// a bare "Permission denied" from the box's shell is not ssh rejecting
    /// credentials: both keep the link retrying.
    #[tokio::test]
    async fn box_failures_with_status_255_are_transient() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let quiet = dir.path().join("quiet");
            std::fs::create_dir(&quiet)?;
            let transport = fake(&quiet, "bash: /home/acme/.bashrc: Permission denied\n", 255)?;
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(!error.is_permanent(), "{error}");

            let printed = dir.path().join("printed");
            std::fs::create_dir(&printed)?;
            let ssh = install(
                &printed,
                "ssh",
                &Behaviour::new()
                    .stdout("devlanes-probe-x\n")
                    .stderr("user@box: Permission denied (publickey).\n")
                    .exit(255),
            )?;
            let mut options = SshOptions::new(&printed);
            options.ssh_program = ssh;
            let transport = SshTransport::new("box", options);
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(!error.is_permanent(), "{error}");
            Ok(())
        })
        .await?
    }

    /// Rejected credentials or a changed host key on a box that connected
    /// before, in this process or as `connected_before` says, wait for a
    /// person to sign in instead of stopping the link.
    #[tokio::test]
    async fn auth_failures_after_a_connection_need_sign_in() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            for (index, log) in [
                "user@box: Permission denied (publickey).\n",
                "Host key verification failed.\n",
            ]
            .into_iter()
            .enumerate()
            {
                let run = dir.path().join(index.to_string());
                std::fs::create_dir(&run)?;
                let transport = fake(&run, log, 255)?;
                transport.connected.store(true, Ordering::Relaxed);
                let Err(error) = transport.connect().await else {
                    panic!("fake ssh unexpectedly connected");
                };
                assert!(error.is_needs_auth(), "{log}: {error}");
                assert!(!error.is_permanent(), "{log}: {error}");

                let mut options = transport.options.clone();
                options.connected_before = true;
                let restored = SshTransport::new("box", options);
                let Err(error) = restored.connect().await else {
                    panic!("fake ssh unexpectedly connected");
                };
                assert!(error.is_needs_auth(), "{log}: {error}");
            }
            Ok(())
        })
        .await?
    }

    /// A misspelt host stops the link only before the first connection.
    #[tokio::test]
    async fn dns_failures_are_permanent_only_before_the_first_connection() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let transport = fake(
                dir.path(),
                "ssh: Could not resolve hostname box: No such host is known.\n",
                255,
            )?;
            let Err(first) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(first.is_permanent(), "a typo should stop the link");
            transport.connected.store(true, Ordering::Relaxed);
            let Err(later) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(!later.is_permanent(), "a reconnect after DNS loss retries");
            Ok(())
        })
        .await?
    }

    /// A box without devlanes stops the link only before the first tunnel;
    /// later the binary may be missing for a moment during an upgrade.
    #[test]
    fn a_missing_binary_is_permanent_only_before_the_first_connection() {
        let transport = SshTransport::new("box", SshOptions::new(r"C:\h"));
        assert!(transport.not_installed().is_permanent());
        transport.connected.store(true, Ordering::Relaxed);
        let later = transport.not_installed();
        assert!(!later.is_permanent());
        assert!(!TransportError::from(later).is_permanent());
    }

    /// A box whose probe finds no devlanes ends the session after the
    /// sentinel, which is a missing binary, not a failed tunnel.
    #[test]
    fn a_box_without_devlanes_ends_after_the_probe() -> TestResult {
        let transport = SshTransport::new("box", SshOptions::new(r"C:\h"));
        let start = "devlanes-probe-1";
        let end = "devlanes-tunnel-ready-1";
        let error = CommandError::Preamble {
            error: PreambleError::Ended {
                output: format!(
                    "motd\n{start}\ndevlanes-probe:os=Linux\ndevlanes-probe:arch=x86_64\n\
                         devlanes-probe:program=\n{end}\n"
                )
                .into_bytes(),
            },
            permanent: false,
            stderr: String::new(),
        };
        let error = transport.session_failed(error, start, end);
        assert!(
            matches!(
                error,
                SshError::NotInstalled {
                    permanent: true,
                    ..
                }
            ),
            "{error:?}"
        );
        let probe = transport.remembered().ok_or("no probe remembered")?;
        assert_eq!(probe.platform.os, "linux");
        // With devlanes found, the same end is the tunnel's own failure.
        let error = CommandError::Preamble {
            error: PreambleError::Ended {
                output: format!(
                    "{start}\ndevlanes-probe:os=Linux\ndevlanes-probe:arch=x86_64\n\
                         devlanes-probe:program=/usr/bin/devlanes\n{end}\n"
                )
                .into_bytes(),
            },
            permanent: false,
            stderr: String::new(),
        };
        let error = transport.session_failed(error, start, end);
        assert!(matches!(error, SshError::Tunnel(_)), "{error:?}");
        Ok(())
    }

    /// An ssh that never prints the sentinel fails the session once the
    /// authentication limit passes, and is ended with its job.
    #[tokio::test]
    async fn a_silent_ssh_times_out() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let ssh = install(
                dir.path(),
                "ssh",
                &Behaviour::new().sleep(Duration::from_secs(60)),
            )?;
            let mut options = SshOptions::new(dir.path());
            options.ssh_program = ssh;
            options.auth_timeout = Duration::from_millis(500);
            let transport = SshTransport::new("box", options);
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(!error.is_permanent(), "{error}");
            assert!(error.to_string().contains("in time"), "{error}");
            Ok(())
        })
        .await?
    }

    /// A probe run reads the box's answer; the next probe reuses it.
    #[tokio::test]
    async fn probes_parse_and_are_remembered() -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let ssh = install(
                dir.path(),
                "ssh",
                &Behaviour::new().record(dir.path().join("args")).stdout(
                    "motd\ndevlanes-probe:os=Linux\ndevlanes-probe:arch=x86_64\n\
                         devlanes-probe:program=/usr/local/bin/devlanes\n",
                ),
            )?;
            let mut options = SshOptions::new(dir.path());
            options.ssh_program = ssh;
            let transport = SshTransport::new("box", options);
            let probe = transport.probe().await?;
            assert_eq!(probe.platform.os, "linux");
            assert_eq!(probe.program.as_deref(), Some("/usr/local/bin/devlanes"));
            assert_eq!(transport.probe().await?, probe);
            let runs = std::fs::read_to_string(dir.path().join("args"))?;
            assert_eq!(runs.lines().count(), 1, "{runs}");
            Ok(())
        })
        .await?
    }

    /// A missing client is a permanent error that names the optional feature.
    #[tokio::test]
    async fn a_missing_client_is_reported() -> TestResult {
        let dir = tempfile::tempdir()?;
        let mut options = SshOptions::new(dir.path());
        options.ssh_program = dir.path().join("no-such-ssh.exe");
        let transport = SshTransport::new("box", options);
        let Err(error) = transport.connect().await else {
            panic!("a missing ssh connected");
        };
        assert!(error.is_permanent());
        assert!(error.to_string().contains("OpenSSH Client"), "{error}");
        Ok(())
    }

    /// Bare names prefer the system client; paths are kept; a name found
    /// nowhere is never left for `CreateProcessW` to look up.
    #[test]
    fn bare_names_find_the_system_client() {
        let explicit = Path::new(r"C:\tools\ssh.exe");
        assert_eq!(locate_program(explicit).as_deref(), Ok(explicit));
        let system = devlanes_sys_windows::file::system_directory()
            .ok()
            .map(|system| system.join(r"OpenSSH\ssh.exe"));
        if let Some(system) = system.filter(|path| path.is_file()) {
            assert_eq!(locate_program(Path::new("ssh")), Ok(system));
        }
        assert_eq!(
            locate_program(Path::new("devlanes-no-such-ssh")),
            Err(PathBuf::from("devlanes-no-such-ssh.exe"))
        );
    }

    /// The command keeps the remote command as one argument whatever quotes it
    /// holds; the command transport quotes each argument for ssh's parser.
    #[test]
    fn commands_keep_the_remote_command_whole() {
        let mut options = SshOptions::new(r"C:\h");
        options.ssh_options = vec![r#"IdentityFile="C:\Users\a b\key""#.to_owned()];
        let transport = SshTransport::new("acme@box", options);
        let script = r#"printf '%s\n' "$p" 'x'"#;
        let argv = transport.command_argv(
            Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe"),
            script,
            true,
        );
        assert_eq!(
            argv[..4],
            [
                r"C:\Windows\System32\OpenSSH\ssh.exe",
                "-T",
                "-o",
                "ServerAliveInterval=15"
            ]
        );
        let at = argv
            .iter()
            .position(|arg| arg == r#"IdentityFile="C:\Users\a b\key""#)
            .expect("the identity option");
        assert_eq!(argv[at - 1], "-o");
        assert_eq!(
            argv[argv.len() - 5..],
            [
                "-o",
                "BatchMode=yes",
                "--",
                "acme@box",
                r#"sh -c 'printf '\''%s\n'\'' "$p" '\''x'\'''"#
            ]
        );
    }
}
