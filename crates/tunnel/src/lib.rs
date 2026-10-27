//! Box side of remote operation: `devlanes tunnel --stdio`.
//!
//! Remote is mirroring only. The tunnel writes the transport's preamble line,
//! then [`services`] answers a client daemon's mux streams from this machine's
//! own daemon, reached over its socket like any other client:
//!
//! - the registry is the daemon's `remote.registry` feed: a snapshot, then
//!   each change; it carries ports, hostnames and states, never environments
//!   or secrets, the client gets it without hidden units, and a feed that
//!   ends or breaks ends the session;
//! - the client's mirror reports go to the daemon with `remote.report`, which
//!   keeps the latest one with the client's `status` entry;
//! - data streams dial loopback ports of running, non-hidden services,
//!   containers and `infra.external` ports, nothing else;
//! - syncs stream `sync.status` entries of non-hidden services; the client
//!   never chooses a path;
//! - with `remote.require_pairing` (the default), a client is served only
//!   after the daemon accepted its remote token (`remote.pairing.verify`) or
//!   traded its one-time code for one (`remote.pairing.claim`), and its feed
//!   ends when `daemon revoke` deletes its pairing.

mod pairing;
mod registry;
mod sync;
#[cfg(test)]
mod test_daemon;

use std::sync::Arc;

use devlanes_client::{Client, Paths};
use devlanes_proto::{Context, Method, remote::RemoteClient};
use devlanes_transport::{
    BoxInfo, BoxServices, EXIT_DAEMON_STOPPED, LoopbackDialer, NoPairing, PairingGate, ServeError,
    ServeOptions, TUNNEL_PREAMBLE,
};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// Login-session variables a daemon started by the tunnel must not keep: the
/// daemon outlives the SSH session, and an agent socket would let its processes
/// use the connecting user's keys.
const SESSION_ENV: [&str; 5] = [
    "SSH_AUTH_SOCK",
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "SSH_ORIGINAL_COMMAND",
];

/// Why the tunnel stopped.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    /// The box daemon could not be reached or started.
    #[error("box daemon: {0}")]
    Daemon(#[from] devlanes_client::Error),
    /// No box daemon runs and the box's user config turns off starting one
    /// (`remote.start_daemon: false`).
    #[error("box daemon is not running")]
    DaemonStopped,
    /// The box daemon's `daemon.status` did not have the expected shape.
    #[error("box daemon sent an unexpected daemon.status: {0}")]
    Status(#[from] serde_json::Error),
    /// Writing the preamble to stdout failed.
    #[error("writing to the client: {0}")]
    Stdout(#[source] std::io::Error),
    /// The session with the client failed.
    #[error(transparent)]
    Serve(#[from] ServeError),
}

impl TunnelError {
    /// The process exit status for this failure: [`EXIT_DAEMON_STOPPED`] for a
    /// stopped daemon, which the client's transport reports as temporary, else 1.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::DaemonStopped => u8::try_from(EXIT_DAEMON_STOPPED).unwrap_or(1),
            Self::Daemon(_) | Self::Status(_) | Self::Stdout(_) | Self::Serve(_) => 1,
        }
    }
}

/// Box services backed by the daemon behind `paths`.
///
/// `client` (with the id and name from each client's handshake) is listed by
/// the box's `status` while that client's registry feed is open. With
/// `require_pairing` (the box's `remote.require_pairing`), the daemon checks
/// each client's token or code first, and ends the feed of a client whose
/// pairing is revoked; without it every client is served, as SSH let it in.
pub fn services(paths: Paths, client: RemoteClient, require_pairing: bool) -> BoxServices {
    let pairing: Arc<dyn PairingGate> = if require_pairing {
        Arc::new(pairing::DaemonPairing::new(
            paths.clone(),
            client.ssh.clone(),
        ))
    } else {
        Arc::new(NoPairing)
    };
    BoxServices {
        registry: Arc::new(registry::DaemonRegistry::new(
            paths.clone(),
            client,
            require_pairing,
        )),
        dialer: Arc::new(LoopbackDialer),
        sync: Arc::new(sync::DaemonSync::new(paths)),
        pairing,
    }
}

/// What the handshake label adds when the box daemon this tunnel started
/// stays inside the ssh session's job, which ends it with the session
/// (Windows, when the job refuses breakaway). The client warns about it.
pub const SESSION_BOUND: &str = "(daemon stops with the session)";

/// What the tunnel tells the client on stderr in that case.
const SESSION_BOUND_HINT: &str = "the box daemon started here could not leave this ssh session's \
     job, so it stops when the session ends; run `devlanes daemon install` on the box, or start \
     the daemon outside ssh";

/// Version label sent to clients in the handshake: `devlanes <version>`, plus
/// `(linger off)` on Linux boxes whose user services stop at logout.
pub fn server_label() -> String {
    let label = format!("devlanes {}", devlanes_version::VERSION);
    if linger_off() {
        format!("{label} (linger off)")
    } else {
        label
    }
}

/// False: Windows has no systemd to stop the user's processes at logout.
#[cfg(windows)]
fn linger_off() -> bool {
    false
}

/// Whether the daemon that `daemon`'s connection started ends with this
/// process's job: it could not break away, and the job kills its processes
/// when it closes, as an ssh session's job does. Win32-OpenSSH's job allows
/// breakaway, so this happens with other ssh servers, or with a job inside
/// the session's that refuses it. Only the innermost job's kill-on-close
/// limit is read (what `job_of_self` reports), which is the session's own
/// job under sshd.
#[cfg(windows)]
fn bound_to_session(daemon: &Client) -> bool {
    daemon.spawned_in_job
        && devlanes_sys_windows::job::job_of_self().is_ok_and(|job| job.kill_on_close)
}

/// False: a Unix daemon detaches from the ssh session with `setsid`.
#[cfg(unix)]
fn bound_to_session(_daemon: &Client) -> bool {
    false
}

/// True on Linux when systemd stops the user's processes at logout, which would
/// stop a daemon the tunnel started once the SSH session ends.
#[cfg(unix)]
fn linger_off() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let user = rustix::process::getuid().as_raw().to_string();
    std::process::Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger", "--value"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"no")
}

/// Serves one client over this process's stdin and stdout until it leaves.
///
/// Writes [`TUNNEL_PREAMBLE`] first, before anything that could take time, so
/// the client can tell this tunnel's output from what the box's login shell
/// printed. Starts the box daemon when it is not running and `start_daemon`
/// (the box's `remote.start_daemon`) allows it; otherwise a stopped daemon
/// fails with [`TunnelError::DaemonStopped`]. `require_pairing` (the box's
/// `remote.require_pairing`) serves only paired clients ([`services`]).
/// Nothing but the preamble and mux frames may reach stdout, so diagnostics
/// belong on stderr, which ssh hands back to the client. A daemon started
/// here that stays inside the ssh session's job is reported there and in the
/// label ([`SESSION_BOUND`]).
///
/// On Windows stdin and stdout are the pipes sshd passes to a command without
/// a terminal, which std reads and writes byte for byte; only console handles
/// get text conversion.
pub async fn run_stdio(
    paths: Paths,
    start_daemon: bool,
    require_pairing: bool,
) -> Result<(), TunnelError> {
    write_preamble(&mut tokio::io::stdout()).await?;
    let mut daemon = daemon_client(&paths, start_daemon).await?;
    let bound = bound_to_session(&daemon);
    if bound {
        eprintln!("devlanes tunnel: {SESSION_BOUND_HINT}");
    }
    let box_info = status_box_info(&mut daemon).await?;
    drop(daemon);
    let client = RemoteClient {
        pid: std::process::id(),
        ssh: std::env::var("SSH_CONNECTION")
            .ok()
            .filter(|value| !value.is_empty()),
        since_ms: now_ms(),
        ..RemoteClient::default()
    };
    // `loginctl` blocks briefly; it runs once, before serving starts.
    let mut label = tokio::task::spawn_blocking(server_label)
        .await
        .unwrap_or_else(|_| format!("devlanes {}", devlanes_version::VERSION));
    if bound {
        label = format!("{label} {SESSION_BOUND}");
    }
    let mut options = ServeOptions::new(label);
    options.box_info = box_info;
    devlanes_transport::serve(
        devlanes_transport::stdio(),
        services(paths, client, require_pairing),
        options,
    )
    .await?;
    Ok(())
}

/// The `daemon.status` fields a box reports to clients; a daemon from before
/// they existed leaves them out.
#[derive(Debug, Deserialize)]
struct StatusFields {
    #[serde(default)]
    https_port: Option<u16>,
    #[serde(default)]
    ports_range: Option<String>,
}

/// Writes [`TUNNEL_PREAMBLE`] to `out` and flushes it.
async fn write_preamble(out: &mut (impl AsyncWrite + Unpin)) -> Result<(), TunnelError> {
    out.write_all(TUNNEL_PREAMBLE.as_bytes())
        .await
        .map_err(TunnelError::Stdout)?;
    out.flush().await.map_err(TunnelError::Stdout)
}

/// What the running box daemon behind `paths` tells clients about this box,
/// for [`ServeOptions::box_info`]: its proxy's HTTPS port and `ports.range`.
pub async fn box_info(paths: &Paths) -> Result<BoxInfo, TunnelError> {
    status_box_info(&mut Client::connect(paths).await?).await
}

/// [`box_info`] over an open connection to the box daemon.
async fn status_box_info(daemon: &mut Client) -> Result<BoxInfo, TunnelError> {
    let status = daemon
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await?;
    let fields: StatusFields = serde_json::from_value(status)?;
    Ok(BoxInfo {
        https_port: fields.https_port,
        ports_range: fields.ports_range,
    })
}

/// A connection to the box daemon, starting it first only when `start_daemon`.
async fn daemon_client(paths: &Paths, start_daemon: bool) -> Result<Client, TunnelError> {
    if start_daemon {
        return Ok(Client::connect_or_spawn_without(paths, &SESSION_ENV).await?);
    }
    match Client::connect(paths).await {
        Ok(client) => Ok(client),
        Err(error) if error.is_not_running() => Err(TunnelError::DaemonStopped),
        Err(error) => Err(error.into()),
    }
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Paths of a home in `dir` where no daemon runs.
    fn idle_home(dir: &std::path::Path) -> Paths {
        let socket = dir.join("run").join("d.sock");
        // Windows pipes have their own namespace.
        #[cfg(windows)]
        let socket = std::path::PathBuf::from(format!(
            r"\\.\pipe\devlanes-tunnel-test-{}",
            socket.to_string_lossy().replace(['\\', '/', ':'], "-")
        ));
        Paths {
            run: dir.join("run"),
            socket,
            db: dir.join("state.db"),
            logs: dir.join("logs"),
            home: dir.to_path_buf(),
        }
    }

    /// With `remote.start_daemon: false` a stopped daemon is not started: the
    /// tunnel fails with the status the client's transport retries on.
    #[tokio::test]
    async fn a_stopped_daemon_is_reported_when_starting_is_off() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = idle_home(dir.path());
        let result = tokio::time::timeout(Duration::from_secs(30), daemon_client(&paths, false))
            .await
            .expect("timeout");
        let Err(error) = result else {
            panic!("connected to a daemon that does not run");
        };
        assert!(matches!(error, TunnelError::DaemonStopped), "{error}");
        assert_eq!(error.to_string(), "box daemon is not running");
        assert_eq!(i32::from(error.exit_code()), EXIT_DAEMON_STOPPED);
        assert!(!paths.run.exists(), "nothing was prepared for a daemon");
    }

    /// The tunnel's output starts with the preamble line, so the client can
    /// find it after whatever a login shell printed.
    #[tokio::test]
    async fn output_starts_with_the_preamble() -> Result<(), TunnelError> {
        let mut out = Vec::new();
        write_preamble(&mut out).await?;
        assert_eq!(out, TUNNEL_PREAMBLE.as_bytes());
        Ok(())
    }

    /// A box daemon from before `https_port` and `ports_range` existed leaves
    /// the box info empty instead of failing the tunnel.
    #[tokio::test]
    async fn box_info_defaults_without_the_new_status_fields() -> test_daemon::TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let paths = test_daemon::home(dir.path())?;
            let daemon = test_daemon::answer_one(&paths, |_| devlanes_proto::Outcome::Ok {
                data: json!({"version": "0.3.0", "pid": 42}),
            })
            .await?;
            let info = box_info(&paths).await?;
            assert_eq!(info, BoxInfo::default());
            let request = daemon.await??;
            assert_eq!(request.method, Method::DaemonStatus);
            Ok(())
        })
        .await?
    }
}
