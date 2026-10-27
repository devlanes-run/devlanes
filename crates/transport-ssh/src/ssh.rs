//! System OpenSSH as the pipe: one ControlMaster per box, one channel per
//! session, each a `ssh -S <control socket>` run through the command transport.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{BoxDuplex, Prefixed, Transport, TransportError};
use devlanes_transport_command::{CommandError, CommandOptions, CommandStream, CommandTransport};
use openssh::{Session, Stdio};
use tokio::sync::Mutex;

use crate::askpass::{AskpassConfig, AskpassServer};
use crate::probe::{self, INSTALL_DIR, RemoteProbe, SETUP_GUIDE};

/// macOS `sun_path` holds 104 bytes including the terminator, and ssh binds the
/// master at the control path plus `.` and 16 random characters before renaming it.
const MAX_CONTROL_PATH: usize = 104 - 1 - 17;

/// How long an idle master outlives its last session. Finite, so a master left
/// behind by a daemon that died exits on its own; long enough that a restarted
/// daemon reconnects through it without new prompts.
const CONTROL_PERSIST: &str = "ControlPersist=600";

/// Bytes of ssh's log kept for error messages.
const LOG_TAIL: usize = 4096;

/// Distinguishes askpass relays started by one process.
static NEXT_ASKPASS: AtomicU64 = AtomicU64::new(0);

/// Set once this process cleared masters left by older devlanes versions.
static LEGACY_SWEPT: AtomicBool = AtomicBool::new(false);

/// Settings for [`SshTransport`].
#[derive(Clone)]
pub struct SshOptions {
    /// Directory for control sockets, the master log and askpass files (made 0700).
    pub control_dir: PathBuf,
    /// Local `ssh` binary.
    pub ssh_program: PathBuf,
    /// Remote command name, looked up on PATH and then in `$HOME/.devlanes/bin`.
    pub remote_program: String,
    /// TCP connect limit passed as `ConnectTimeout`.
    pub connect_timeout: Duration,
    /// Limit for the whole master launch, including time spent answering prompts.
    pub auth_timeout: Duration,
    /// Extra `-o` options for the master, such as `Port=2222`.
    pub ssh_options: Vec<String>,
    /// Prompt relay; without it the master runs with `BatchMode=yes`.
    pub askpass: Option<AskpassConfig>,
    /// The box connected before, such as one a restarted daemon restores:
    /// failures are classified as after a first successful tunnel. Only
    /// permanent non-auth failures then stop the link; a box that is gone
    /// keeps being retried until the caller stops the link.
    pub connected_before: bool,
}

impl SshOptions {
    /// Defaults with control files under `<devlanes_home>/run/ssh`.
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
    /// Local filesystem or process I/O failed.
    #[error("ssh I/O: {0}")]
    Io(#[from] io::Error),
    /// The control socket path would not fit in `sun_path`.
    #[error("ssh control path {0} is too long; use a shorter control directory")]
    ControlPathTooLong(PathBuf),
    /// The master did not start; `log` holds ssh's own diagnostics.
    #[error("ssh to {destination} failed ({status}): {log}")]
    Master {
        /// The ssh destination.
        destination: String,
        /// Exit status description.
        status: String,
        /// Tail of ssh's log.
        log: String,
        /// Whether retrying cannot help, such as rejected credentials on a
        /// first connection.
        permanent: bool,
        /// ssh rejected the credentials or the host key: permanent on a first
        /// connection, and a person has to sign in again after one.
        auth: bool,
    },
    /// The master did not finish authenticating in time.
    #[error("ssh to {0} did not authenticate in time")]
    AuthTimeout(String),
    /// A command over the master failed.
    #[error("ssh command: {0}")]
    Ssh(#[from] openssh::Error),
    /// The box answered the probe with unexpected output.
    #[error("unexpected probe output from the box: {0:?}")]
    Probe(String),
    /// The box's shell cannot run the devlanes found there by that path.
    #[error(
        "{shell} on {destination} cannot run devlanes from {program} because the path holds \
         `%`, `!` or `\"`. Install it in another folder"
    )]
    Unquotable {
        /// The ssh destination.
        destination: String,
        /// Where the probe found devlanes.
        program: String,
        /// The box's shell, as messages name it.
        shell: &'static str,
    },
    /// devlanes is neither on the box's `PATH` nor in `~/.devlanes/bin`.
    #[error(
        "devlanes is not installed on {destination}. Put it on the ssh PATH or in \
         ~/{INSTALL_DIR} as {SETUP_GUIDE} describes"
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
            Self::ControlPathTooLong(_) | Self::Unquotable { .. } => true,
            Self::Io(_) | Self::AuthTimeout(_) | Self::Ssh(_) | Self::Probe(_) => false,
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

/// A running ControlMaster and what the box reported over it.
struct Master {
    session: Arc<Session>,
    ctl: PathBuf,
    log: PathBuf,
    ssh_program: PathBuf,
    probe: Option<RemoteProbe>,
}

impl Drop for Master {
    fn drop(&mut self) {
        // Resumed sessions do not stop the master on drop, so ask it to exit.
        // Spawned without waiting; tokio reaps the child in the background.
        if tokio::runtime::Handle::try_current().is_ok() {
            let _ = tokio::process::Command::new(&self.ssh_program)
                .arg("-S")
                .arg(&self.ctl)
                .args(["-O", "exit", "--", "none"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        let log = self.log.clone();
        // Unlinking one file is cheap; doing it inline keeps cleanup working without a runtime.
        let _ = std::fs::remove_file(log);
    }
}

/// [`Transport`] that runs `devlanes tunnel --stdio` on a box over system OpenSSH.
///
/// The user's `~/.ssh/config`, agent, ProxyJump and certificates apply because
/// the real `ssh` binary does the work.
pub struct SshTransport {
    destination: String,
    options: SshOptions,
    master: Mutex<Option<Master>>,
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
            master: Mutex::new(None),
            options,
        }
    }

    /// The ssh destination.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Reports the box platform and devlanes location, starting a master if needed.
    pub async fn probe(&self) -> Result<RemoteProbe, SshError> {
        let mut master = self.master.lock().await;
        let master = self.ensure_master(&mut master).await?;
        self.ensure_probe(master).await.cloned()
    }

    /// Stops the master, ending every session over it.
    pub async fn close(&self) {
        self.master.lock().await.take();
    }

    /// Opens the tunnel, starting or reusing the master, and reads up to its
    /// preamble. The master is free for other connects meanwhile.
    async fn open(&self) -> Result<Prefixed<CommandStream>, SshError> {
        let tunnel = self.tunnel().await?;
        let opened = tunnel.open().await.map_err(|error| match error {
            // ssh itself did not start, as when the master's launch fails.
            CommandError::Spawn { source, .. } => SshError::Io(source),
            other => SshError::Tunnel(Box::new(other)),
        })?;
        self.connected.store(true, Ordering::Relaxed);
        Ok(opened.stream)
    }

    /// The tunnel command over a live master, starting one and probing the
    /// box first when needed.
    async fn tunnel(&self) -> Result<CommandTransport, SshError> {
        let mut guard = self.master.lock().await;
        let master = self.ensure_master(&mut guard).await?;
        let probe = self.ensure_probe(master).await?;
        let program = probe.program.clone().ok_or_else(|| self.not_installed())?;
        let command = probe
            .shell
            .tunnel_command(&program)
            .ok_or_else(|| SshError::Unquotable {
                destination: self.destination.clone(),
                shell: probe.shell.name(),
                program,
            })?;
        let mut options = CommandOptions::new(&self.destination);
        options.what = crate::TUNNEL.to_owned();
        options.preamble_wait = crate::PREAMBLE_WAIT;
        options.connected_before = self.connected.load(Ordering::Relaxed);
        Ok(CommandTransport::new(
            tunnel_argv(&self.options.ssh_program, &master.ctl, &command),
            options,
        ))
    }

    /// [`SshError::NotInstalled`] for this box: permanent until a tunnel once
    /// opened, like a host name that does not resolve.
    fn not_installed(&self) -> SshError {
        SshError::NotInstalled {
            destination: self.destination.clone(),
            permanent: !self.connected.load(Ordering::Relaxed),
        }
    }

    /// Returns a live master, replacing a dead one.
    async fn ensure_master<'a>(
        &self,
        slot: &'a mut Option<Master>,
    ) -> Result<&'a mut Master, SshError> {
        let alive = match slot.as_ref() {
            Some(master) => master.session.check().await.is_ok(),
            None => false,
        };
        if !alive {
            *slot = None;
            let master = match self.resume_master().await {
                Some(master) => master,
                None => self.launch_master().await?,
            };
            *slot = Some(master);
        }
        slot.as_mut()
            .ok_or_else(|| SshError::Io(io::Error::other("master slot empty after launch")))
    }

    /// Runs the probe once per master, so the box's shell is learnt once too:
    /// the `sh` probe, then the Windows probe when that calls for it (see
    /// [`probe::needs_windows_probe`]). Without an answer from the Windows
    /// probe, as on a Unix box without PowerShell, the `sh` probe's stands,
    /// also when running the Windows probe failed; an error then carries the
    /// Windows probe's stderr too.
    async fn ensure_probe<'a>(&self, master: &'a mut Master) -> Result<&'a RemoteProbe, SshError> {
        if master.probe.is_none() {
            let output = master
                .session
                .command("sh")
                .arg("-c")
                .arg(probe::probe_script(&self.options.remote_program))
                .stdin(Stdio::null())
                .output()
                .await?;
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let mut found = probe::parse_probe(&stdout);
            let mut windows_stderr = None;
            if probe::needs_windows_probe(found.as_ref()) {
                let output = master
                    .session
                    .raw_command(probe::windows_probe_command(&self.options.remote_program))
                    .stdin(Stdio::null())
                    .output()
                    .await;
                if let Ok(output) = output {
                    match probe::parse_probe(&String::from_utf8_lossy(&output.stdout)) {
                        Some(windows) => found = Some(windows),
                        None => windows_stderr = Some(output.stderr),
                    }
                }
            }
            master.probe = Some(found.ok_or_else(|| {
                SshError::Probe(probe::probe_failure(&stdout, windows_stderr.as_deref()))
            })?);
        }
        master
            .probe
            .as_ref()
            .ok_or_else(|| SshError::Probe(String::new()))
    }

    /// The control socket and log of this destination's master.
    ///
    /// The name is stable for a destination and its options, so a restarted
    /// daemon finds a master that is still persisting and reuses it.
    fn control_paths(&self) -> (PathBuf, PathBuf) {
        let mut key = self.destination.clone();
        for option in &self.options.ssh_options {
            key.push('\0');
            key.push_str(option);
        }
        let name = format!("m-{:016x}", fnv1a(key.as_bytes()));
        (
            self.options.control_dir.join(&name),
            self.options.control_dir.join(format!("{name}.log")),
        )
    }

    /// A master left by an earlier daemon for this destination, if it still answers.
    async fn resume_master(&self) -> Option<Master> {
        let (ctl, log) = self.control_paths();
        if !tokio::fs::try_exists(&ctl).await.unwrap_or(false) {
            return None;
        }
        let session = Session::resume(
            ctl.clone().into_boxed_path(),
            Some(log.clone().into_boxed_path()),
        );
        if session.check().await.is_err() {
            // A dead socket blocks a new master at the same path.
            let _ = tokio::fs::remove_file(&ctl).await;
            return None;
        }
        Some(Master {
            session: Arc::new(session),
            ctl,
            log,
            ssh_program: self.options.ssh_program.clone(),
            probe: None,
        })
    }

    /// Starts `ssh -M -f -N` with askpass routing and resumes it as an openssh session.
    ///
    /// openssh's own builder forces `BatchMode=yes` and cannot set the master's
    /// environment, which would rule out askpass, so the master is launched here.
    async fn launch_master(&self) -> Result<Master, SshError> {
        let (ctl, log) = self.control_paths();
        if ctl.as_os_str().len() > MAX_CONTROL_PATH {
            return Err(SshError::ControlPathTooLong(ctl));
        }
        let dir = self.options.control_dir.clone();
        tokio::task::spawn_blocking(move || private_dir(&dir))
            .await
            .map_err(io::Error::other)??;
        if !LEGACY_SWEPT.swap(true, Ordering::Relaxed) {
            sweep_legacy_masters(&self.options.control_dir, &self.options.ssh_program).await;
        }
        let id = format!(
            "{}-{}",
            std::process::id(),
            NEXT_ASKPASS.fetch_add(1, Ordering::Relaxed)
        );
        let mut command = tokio::process::Command::new(&self.options.ssh_program);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .arg("-E")
            .arg(&log)
            .arg("-S")
            .arg(&ctl)
            .args(["-M", "-f", "-N"])
            .args(["-o", CONTROL_PERSIST])
            .args(["-o", "ServerAliveInterval=15"])
            .args(["-o", "ServerAliveCountMax=3"])
            .arg("-o")
            .arg(format!(
                "ConnectTimeout={}",
                self.options.connect_timeout.as_secs().max(1)
            ));
        for option in &self.options.ssh_options {
            command.arg("-o").arg(option);
        }
        let askpass = match &self.options.askpass {
            Some(config) => {
                let server = AskpassServer::start(
                    &self.options.control_dir,
                    &format!("a-{id}"),
                    config,
                    &self.destination,
                )
                .await?;
                for (key, value) in server.env() {
                    command.env(key, value);
                }
                Some(server)
            }
            None => {
                command.args(["-o", "BatchMode=yes"]);
                None
            }
        };
        // `--` keeps a destination starting with `-` from being parsed as an option.
        command.arg("--").arg(&self.destination);
        // With -f, ssh forks into the background once authenticated, so this
        // returns when the master is usable or authentication failed.
        let status = tokio::time::timeout(self.options.auth_timeout, command.status())
            .await
            .map_err(|_| SshError::AuthTimeout(self.destination.clone()))??;
        drop(askpass);
        if !status.success() {
            let log_path = log.clone();
            let text = tokio::task::spawn_blocking(move || {
                let text = std::fs::read_to_string(&log_path).unwrap_or_default();
                let _ = std::fs::remove_file(&log_path);
                text
            })
            .await
            .map_err(io::Error::other)?;
            let first = !self.connected.load(Ordering::Relaxed);
            return Err(master_error(&self.destination, status, &text, first));
        }
        let session = Session::resume(
            ctl.clone().into_boxed_path(),
            Some(log.clone().into_boxed_path()),
        );
        Ok(Master {
            session: Arc::new(session),
            ctl,
            log,
            ssh_program: self.options.ssh_program.clone(),
            probe: None,
        })
    }
}

#[async_trait]
impl Transport for SshTransport {
    async fn connect(&self) -> Result<BoxDuplex, TransportError> {
        Ok(Box::new(self.open().await?))
    }
}

/// Creates `dir` with mode 0700 so sockets and askpass files stay private.
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Classifies a failed master launch from ssh's log.
///
/// A name that does not resolve is permanent only before the first successful
/// connection (a typo); afterwards it is a network change that will pass.
/// Rejected credentials and host key failures are permanent before the first
/// connection and need a person to sign in again after it.
fn master_error(destination: &str, status: ExitStatus, log: &str, first: bool) -> SshError {
    // These need new credentials or known_hosts changes, not time.
    const AUTH: [&str; 3] = [
        "Permission denied",
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

/// 64-bit FNV-1a: stable across builds, unlike the std hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Asks masters named by older devlanes versions (`m-<pid>-<n>`, persisting
/// forever) to exit, and removes their logs. Stable `m-<hash>` masters are left
/// for reuse; they expire after [`CONTROL_PERSIST`].
async fn sweep_legacy_masters(dir: &Path, ssh_program: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("m-") else {
            continue;
        };
        let legacy = rest
            .trim_end_matches(".log")
            .split_once('-')
            .is_some_and(|(pid, n)| {
                pid.bytes().all(|b| b.is_ascii_digit()) && n.bytes().all(|b| b.is_ascii_digit())
            });
        if !legacy {
            continue;
        }
        if name.ends_with(".log") {
            let _ = tokio::fs::remove_file(entry.path()).await;
            continue;
        }
        let _ = tokio::process::Command::new(ssh_program)
            .arg("-S")
            .arg(entry.path())
            .args(["-O", "exit", "--", "none"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await;
        let _ = tokio::fs::remove_file(entry.path()).await;
    }
}

/// Returns the last [`LOG_TAIL`] bytes as lossy UTF-8.
fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(LOG_TAIL);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

/// `ssh` running `command`, the box shell's `<program> tunnel --stdio`
/// ([`probe::BoxShell::tunnel_command`]), over the master at `ctl`.
///
/// The master holds the connection and its authentication, so this client
/// reads no ssh config (`-F none`): a `Host *` or `Match all` block would
/// otherwise still apply to it, and its `RemoteCommand`, `RequestTTY` or
/// `ControlMaster` would break the tunnel. Batch mode, no terminal, and the
/// destination `none` on the discard port, so a dead master makes this fail
/// at once instead of opening a connection of its own. ssh hands the box's
/// shell the words after the destination as one command, so `command` goes
/// as one word, already quoted for that shell.
fn tunnel_argv(ssh_program: &Path, ctl: &Path, command: &str) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![ssh_program.into(), "-S".into(), ctl.into()];
    argv.extend(
        [
            "-F",
            "none",
            "-o",
            "BatchMode=yes",
            "-T",
            "-p",
            "9",
            "--",
            "none",
            command,
        ]
        .map(OsString::from),
    );
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes an executable fake `ssh` that records its environment and fails like ssh.
    fn fake_ssh(dir: &Path, log_line: &str) -> io::Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-ssh");
        let script = format!(
            "#!/bin/sh\n\
             while [ $# -gt 0 ]; do case \"$1\" in -E) log=\"$2\"; shift;; esac; shift; done\n\
             printf '%s\\n' \"askpass=$SSH_ASKPASS require=$SSH_ASKPASS_REQUIRE\" > \"{}/env\"\n\
             printf '%s\\n' {} > \"$log\"\n\
             exit 255\n",
            dir.display(),
            crate::sh_quote(log_line),
        );
        std::fs::write(&path, script)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }

    struct Never;

    #[async_trait]
    impl crate::PromptRelay for Never {
        async fn prompt(&self, _prompt: crate::AskpassPrompt) -> Option<String> {
            None
        }
    }

    #[tokio::test]
    async fn rejected_credentials_are_permanent_and_askpass_env_is_set()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let dir = tempfile::tempdir()?;
            let mut options = SshOptions::new(dir.path());
            options.ssh_program = fake_ssh(dir.path(), "user@box: Permission denied (publickey).")?;
            options.askpass = Some(AskpassConfig {
                exe: PathBuf::from("/usr/bin/devlanes"),
                relay: Arc::new(Never),
            });
            let transport = SshTransport::new("box", options);
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(error.is_permanent());
            assert!(error.to_string().contains("Permission denied"));
            assert!(
                error.into_needs_auth().is_needs_auth(),
                "a caller whose prompt went unanswered waits for a sign-in"
            );
            let env = std::fs::read_to_string(dir.path().join("env"))?;
            assert!(env.contains("require=force"), "{env}");
            assert!(env.contains(".askpass"), "{env}");
            Ok(())
        })
        .await?
    }

    /// Rejected credentials or a changed host key on a box that connected
    /// before, in this process or as `connected_before` says, wait for a
    /// person to sign in instead of stopping the link.
    #[tokio::test]
    async fn auth_failures_after_a_connection_need_sign_in()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(10), async {
            for log in [
                "user@box: Permission denied (publickey).",
                "Host key verification failed.",
            ] {
                let dir = tempfile::tempdir()?;
                let mut options = SshOptions::new(dir.path());
                options.ssh_program = fake_ssh(dir.path(), log)?;
                let transport = SshTransport::new("box", options.clone());
                transport.connected.store(true, Ordering::Relaxed);
                let Err(error) = transport.connect().await else {
                    panic!("fake ssh unexpectedly connected");
                };
                assert!(error.is_needs_auth(), "{log}: {error}");
                assert!(!error.is_permanent(), "{log}: {error}");

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

    #[tokio::test]
    async fn network_failures_are_transient() -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let dir = tempfile::tempdir()?;
            let mut options = SshOptions::new(dir.path());
            options.ssh_program = fake_ssh(
                dir.path(),
                "connect to host box port 22: Connection refused",
            )?;
            let transport = SshTransport::new("box", options);
            let Err(error) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(!error.is_permanent());
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn dns_failures_are_permanent_only_before_the_first_connection()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let dir = tempfile::tempdir()?;
            let mut options = SshOptions::new(dir.path());
            options.ssh_program = fake_ssh(
                dir.path(),
                "ssh: Could not resolve hostname box: nodename nor servname provided",
            )?;
            let transport = SshTransport::new("box", options);
            let Err(first) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(first.is_permanent(), "a typo should stop the link");
            // After a tunnel once opened, the same failure is a network change.
            transport.connected.store(true, Ordering::Relaxed);
            let Err(later) = transport.connect().await else {
                panic!("fake ssh unexpectedly connected");
            };
            assert!(
                !later.is_permanent(),
                "a reconnect after DNS loss must keep retrying"
            );
            Ok(())
        })
        .await?
    }

    /// A box without devlanes stops the link only before the first tunnel;
    /// later the binary may be missing for a moment during an upgrade.
    #[test]
    fn a_missing_binary_is_permanent_only_before_the_first_connection() {
        let transport = SshTransport::new("box", SshOptions::new("/tmp/h"));
        assert!(transport.not_installed().is_permanent());
        transport.connected.store(true, Ordering::Relaxed);
        let later = transport.not_installed();
        assert!(!later.is_permanent());
        assert!(!TransportError::from(later).is_permanent());
    }

    /// Tunnels run over the master only, with the command quoted for the
    /// box's shell as one word: the one a Unix box always ran, or a Windows
    /// box's cmd.exe or PowerShell form.
    #[test]
    fn tunnels_go_over_the_master() {
        let argv = |command: &str| -> Vec<String> {
            tunnel_argv(
                Path::new("ssh"),
                Path::new("/tmp/h/run/ssh/m-0123456789abcdef"),
                command,
            )
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
        };
        let unix = probe::BoxShell::Sh
            .tunnel_command("/home/acme/my tools/devlanes")
            .expect("quotable");
        assert_eq!(
            argv(&unix),
            [
                "ssh",
                "-S",
                "/tmp/h/run/ssh/m-0123456789abcdef",
                "-F",
                "none",
                "-o",
                "BatchMode=yes",
                "-T",
                "-p",
                "9",
                "--",
                "none",
                "'/home/acme/my tools/devlanes' tunnel --stdio",
            ]
        );
        let windows = probe::BoxShell::Cmd
            .tunnel_command(r"C:\Users\acme\.devlanes\bin\devlanes.exe")
            .expect("quotable");
        assert_eq!(
            argv(&windows).last().map(String::as_str),
            Some(r#""C:\Users\acme\.devlanes\bin\devlanes.exe" tunnel --stdio"#)
        );
    }

    /// A program cmd.exe would expand stops the link with what to change,
    /// instead of a tunnel that runs something else.
    #[test]
    fn an_unquotable_program_is_permanent() {
        let error = SshError::Unquotable {
            destination: "acme".to_owned(),
            program: r"C:\100%\devlanes.exe".to_owned(),
            shell: probe::BoxShell::Cmd.name(),
        };
        assert!(error.is_permanent());
        assert!(error.to_string().contains("cmd.exe"), "{error}");
    }

    #[test]
    fn control_paths_are_stable_and_short() {
        let options = SshOptions::new("/tmp/h");
        let one = SshTransport::new("box", options.clone()).control_paths();
        let again = SshTransport::new("box", options.clone()).control_paths();
        let other = SshTransport::new("other", options).control_paths();
        assert_eq!(one, again, "a restarted daemon finds the same master");
        assert_ne!(one.0, other.0);
        let name = one.0.file_name().map(|n| n.to_string_lossy().into_owned());
        assert_eq!(name.map(|n| n.len()), Some(18), "m- plus 16 hex digits");
    }

    #[tokio::test]
    async fn legacy_masters_are_swept_and_stable_ones_kept()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let exits = dir.path().join("exits");
        let fake = dir.path().join("fake-ssh");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$2\" >> '{}'\nexit 0\n", exits.display()),
        )?;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))?;
        let control = dir.path().join("ssh");
        std::fs::create_dir(&control)?;
        for name in ["m-123-0", "m-123-0.log", "m-0123456789abcdef", "a-1-0.sock"] {
            std::fs::write(control.join(name), b"")?;
        }
        sweep_legacy_masters(&control, &fake).await;
        assert!(!control.join("m-123-0").exists());
        assert!(!control.join("m-123-0.log").exists());
        assert!(
            control.join("m-0123456789abcdef").exists(),
            "reusable master kept"
        );
        let asked = std::fs::read_to_string(&exits)?;
        assert!(
            asked.contains("m-123-0"),
            "legacy master asked to exit: {asked}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn long_control_paths_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let deep = dir.path().join("x".repeat(80));
        let transport = SshTransport::new("box", SshOptions::new(deep));
        let result = transport.launch_master().await;
        assert!(matches!(result, Err(SshError::ControlPathTooLong(_))));
        Ok(())
    }
}
