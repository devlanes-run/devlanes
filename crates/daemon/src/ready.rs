//! Readiness checks (plan 4.6). A check never restarts or stops anything; it only
//! moves a unit to `ready` or `failed`.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU16, Ordering},
    time::Duration,
};

use devlanes_proto::RpcError;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Instant, sleep, sleep_until, timeout},
};

use crate::units::{Logged, UnitHost, UnitState};

/// What "ready" means for one unit, after config evaluation.
#[derive(Clone)]
pub enum Readiness {
    /// Running is enough.
    Started,
    Tcp {
        port: u16,
    },
    Http {
        port: u16,
        path: String,
        status: Option<u16>,
        /// Host header to send; `None` sends `127.0.0.1:<port>`.
        host: Option<String>,
    },
    /// Regex over the lines of this run's log, as `devlanes logs` shows them.
    Log {
        pattern: String,
    },
    /// Shell command retried until it exits 0, in the unit's own environment.
    Cmd {
        command: String,
        cwd: std::path::PathBuf,
        /// The unit's assembled environment; the daemon's own is never inherited.
        env: BTreeMap<String, String>,
        /// Git for Windows' `sh.exe`, set with [`Readiness::with_shell`];
        /// without one the check fails with `shell_missing` at once.
        #[cfg(windows)]
        shell: Option<std::path::PathBuf>,
    },
}

#[cfg(windows)]
impl Readiness {
    /// Runs a `cmd` check through `shell`; other checks stay as they are.
    pub fn with_shell(self, shell: Option<std::path::PathBuf>) -> Self {
        match self {
            Self::Cmd {
                command, cwd, env, ..
            } => Self::Cmd {
                command,
                cwd,
                env,
                shell,
            },
            other => other,
        }
    }
}

impl std::fmt::Debug for Readiness {
    /// Omits the `cmd` environment, which can hold secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Started => f.write_str("Started"),
            Self::Tcp { port } => f.debug_struct("Tcp").field("port", port).finish(),
            Self::Http {
                port,
                path,
                status,
                host,
            } => f
                .debug_struct("Http")
                .field("port", port)
                .field("path", path)
                .field("status", status)
                .field("host", host)
                .finish(),
            Self::Log { pattern } => f.debug_struct("Log").field("pattern", pattern).finish(),
            Self::Cmd { command, cwd, .. } => f
                .debug_struct("Cmd")
                .field("command", command)
                .field("cwd", cwd)
                .finish_non_exhaustive(),
        }
    }
}

/// Polling interval for probes; short enough to feel instant, long enough to stay cheap.
const POLL: Duration = Duration::from_millis(150);
/// Upper bound for one `cmd` attempt, so a hanging probe cannot eat the whole timeout.
const CMD_ATTEMPT: Duration = Duration::from_secs(10);

/// How long after its spawn a unit's exit still counts as a failed start: a
/// unit is ready only once its check passed and it has run this long.
pub(crate) const EARLY_EXIT: Duration = Duration::from_millis(500);

/// Waits for readiness, process exit or the timeout, then records the result on the host.
/// `listening` reports the ports the unit's tree bound, for the tcp timeout diagnostic.
pub async fn run_check<F, Fut>(host: &UnitHost, check: Readiness, limit: Duration, listening: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Vec<u16>>,
{
    if matches!(check, Readiness::Started) {
        ready_after_window(host).await;
        return;
    }
    // A pattern that cannot compile never matches; waiting for the timeout would
    // only disguise a config mistake as a slow service.
    let log = match &check {
        Readiness::Log { pattern } => match regex::Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(error) => {
                host.set_state(UnitState::Failed(Some(RpcError::new(
                    "config_error",
                    json!({"path": "ready.log", "pattern": pattern, "message": error.to_string()}),
                    1,
                ))));
                return;
            }
        },
        _ => None,
    };
    // Without the shell no attempt can pass; failing now names the cause.
    #[cfg(windows)]
    if let Readiness::Cmd { shell: None, .. } = &check {
        host.set_state(UnitState::Failed(Some(windows::shell_missing(
            json!({"check": "cmd"}),
        ))));
        return;
    }
    let deadline = Instant::now() + limit;
    tracing::debug!(
        unit = %host.info().name,
        check = describe(&check),
        port = match &check {
            Readiness::Tcp { port } | Readiness::Http { port, .. } => Some(*port),
            _ => None,
        },
        timeout_ms = limit.as_millis() as u64,
        "readiness check"
    );
    let last = LastStatus::default();
    let probe = probe_until_ready(host, &check, log.as_ref(), deadline, &last);
    let exited = host.wait_exit();
    tokio::select! {
        result = probe => match result {
            Ok(()) => ready_after_window(host).await,
            Err(Missed::Failed(error)) => host.set_state(UnitState::Failed(Some(error))),
            Err(Missed::Deadline) => {
                let mut data = json!({"check": describe(&check), "timeout_ms": limit.as_millis()});
                if let Readiness::Tcp { port } | Readiness::Http { port, .. } = &check {
                    data["expected_port"] = json!(port);
                    data["listening"] = json!(listening().await);
                }
                if let Readiness::Http { port, path, status, host } = &check {
                    last.describe(&mut data, path, &host_header(*port, host.as_deref()), *status);
                }
                host.set_state(UnitState::Failed(Some(RpcError::new("ready_timeout", data, 1))));
            }
        },
        // The waiter task classifies the exit itself.
        _ = exited => {}
    }
}

/// Marks the unit ready once it has run [`EARLY_EXIT`] since its spawn, unless
/// it exits first; the waiter task classifies that exit.
async fn ready_after_window(host: &UnitHost) {
    tokio::select! {
        biased;
        _ = host.wait_exit() => {}
        () = sleep_until(host.spawned() + EARLY_EXIT) => host.set_state(UnitState::Ready),
    }
}

/// Short machine name for error data.
fn describe(check: &Readiness) -> &'static str {
    match check {
        Readiness::Started => "started",
        Readiness::Tcp { .. } => "tcp",
        Readiness::Http { .. } => "http",
        Readiness::Log { .. } => "log",
        Readiness::Cmd { .. } => "cmd",
    }
}

/// Why a probe ended without passing.
enum Missed {
    Deadline,
    /// The probe cannot work for this run; the unit fails with this error.
    Failed(RpcError),
}

/// Waits until `check` passes. `log` is the compiled pattern of a `log` check.
/// An http probe records each status it receives in `last`.
async fn probe_until_ready(
    host: &UnitHost,
    check: &Readiness,
    log: Option<&regex::Regex>,
    deadline: Instant,
    last: &LastStatus,
) -> Result<(), Missed> {
    let polled = match check {
        Readiness::Started => Ok(()),
        Readiness::Tcp { port } => poll(deadline, || async move { tcp_open(*port).await }).await,
        Readiness::Http {
            port,
            path,
            status,
            host,
        } => {
            let host = &host_header(*port, host.as_deref());
            poll(deadline, || async move {
                http_ok(*port, path, host, *status, last).await
            })
            .await
        }
        #[cfg(unix)]
        Readiness::Cmd { command, cwd, env } => {
            poll(deadline, || async move {
                cmd_succeeds(command, cwd, env, deadline).await
            })
            .await
        }
        #[cfg(windows)]
        Readiness::Cmd {
            command,
            cwd,
            env,
            shell,
        } => {
            // `run_check` fails a check without a shell before probing.
            let Some(shell) = shell else {
                return Err(Missed::Deadline);
            };
            poll(deadline, || async move {
                windows::cmd_succeeds(shell, command, cwd, env, deadline).await
            })
            .await
        }
        Readiness::Log { .. } => {
            // `run_check` passes the compiled pattern for a `log` check.
            let Some(regex) = log else {
                return Err(Missed::Deadline);
            };
            return log_line_matches(host, regex, deadline).await;
        }
    };
    polled.map_err(|()| Missed::Deadline)
}

/// Waits for a line of this run's log that `regex` matches: the lines the
/// screen model logged, as `devlanes logs` shows them. Reads from where the
/// last read stopped each time the log thread stores lines. Fails at once when
/// the log cannot be read or its writer stopped, since no later line can match.
async fn log_line_matches(
    host: &UnitHost,
    regex: &regex::Regex,
    deadline: Instant,
) -> Result<(), Missed> {
    // Watching before the first read: a line stored after this marks the watch
    // changed, and every line stored before it is in that read.
    let mut logged = host.watch_logged();
    let mut cursor = host.run_cursor();
    loop {
        let seen = *logged.borrow_and_update();
        loop {
            let query = devlanes_proto::LogQuery {
                cursor: cursor.clone(),
                max_bytes: Some(64 * 1024),
                ..devlanes_proto::LogQuery::default()
            };
            // No read error clears up while the run lives: the lock stays
            // poisoned, and a query or disk failure repeats on the same bytes.
            let page = host.read_log(query).await.map_err(|error| {
                let message = error.data["message"]
                    .as_str()
                    .map_or_else(|| error.code.clone(), str::to_owned);
                Missed::Failed(ready_log_failed(&message))
            })?;
            if page.items.iter().any(|line| regex.is_match(&line.text)) {
                return Ok(());
            }
            if let Some(line) = page.items.last() {
                cursor = Some(format!("{}#{}", line.unit, line.seq));
            }
            if !page.truncated || page.items.is_empty() {
                break;
            }
        }
        // Checked after the read: lines stored before the writer failed still count.
        if seen == Logged::Failed {
            return Err(Missed::Failed(ready_log_failed("the log writer stopped")));
        }
        match tokio::time::timeout_at(deadline, logged.changed()).await {
            Err(_) => return Err(Missed::Deadline),
            Ok(Ok(())) => {}
            // The writer closes the watch on `Through` only after finishing the
            // log, which follows the published exit, so `run_check` ends on that.
            Ok(Err(_)) => std::future::pending::<()>().await,
        }
    }
}

/// The failure of a `log` check whose log stopped being written or read.
fn ready_log_failed(message: &str) -> RpcError {
    RpcError::new(
        "ready_log_failed",
        json!({"check": "log", "message": message}),
        1,
    )
}

/// Runs one `cmd` attempt in the unit's environment. Each attempt is bounded by
/// [`CMD_ATTEMPT`] and the overall deadline; a timed-out probe is killed, not left
/// running alongside the next attempt.
#[cfg(unix)]
async fn cmd_succeeds(
    command: &str,
    cwd: &std::path::Path,
    env: &BTreeMap<String, String>,
    deadline: Instant,
) -> bool {
    let limit = deadline
        .saturating_duration_since(Instant::now())
        .min(CMD_ATTEMPT);
    let status = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    matches!(timeout(limit, status).await, Ok(Ok(status)) if status.success())
}

/// Waits for `infra.containers.<name>.ready` once the container is up. `ports`
/// maps its port names to published host ports in declaration order; `env` runs
/// a `cmd` probe (on Windows through `shell`, Git's `sh.exe`); `logs` reads the
/// container's last lines for a `log` probe. Fails with `config_error` for an
/// unknown port name or pattern, and with `container_not_ready` when the check
/// does not pass within its timeout.
pub(crate) async fn container_ready<F, Fut>(
    name: &str,
    ready: &devlanes_resolve::ContainerReady,
    ports: &[(String, u16)],
    env: &BTreeMap<String, String>,
    #[cfg(windows)] shell: Option<&std::path::Path>,
    logs: F,
) -> Result<(), RpcError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    use devlanes_resolve::ContainerCheck;
    let path = format!("infra.containers.{name}.ready");
    let host_port = |wanted: &Option<String>| {
        let found = match wanted {
            Some(wanted) => ports.iter().find(|(port, _)| port == wanted),
            None => ports.first(),
        };
        found.map(|(_, port)| *port).ok_or_else(|| {
            RpcError::new(
                "config_error",
                json!({
                    "path": path,
                    "message": match wanted {
                        Some(wanted) => format!("the container has no port named `{wanted}`"),
                        None => "a tcp or http check needs a container port".to_owned(),
                    },
                }),
                1,
            )
        })
    };
    let deadline = Instant::now() + ready.timeout;
    tracing::debug!(
        container = name,
        timeout_ms = ready.timeout.as_millis() as u64,
        "waiting until the container is ready"
    );
    let last = LastStatus::default();
    // The path and Host header an http check sent, for the timeout's error data.
    let mut http_request = None;
    let (kind, passed) = match &ready.check {
        ContainerCheck::Tcp { port } => {
            let port = host_port(port)?;
            ("tcp", poll(deadline, || tcp_open(port)).await)
        }
        ContainerCheck::Http {
            port,
            path,
            status,
            host,
        } => {
            let port = host_port(port)?;
            let host = host_header(port, host.as_deref());
            let probe = || http_ok(port, path, &host, *status, &last);
            let passed = poll(deadline, probe).await;
            http_request = Some((path, host));
            ("http", passed)
        }
        ContainerCheck::Log { pattern } => {
            let regex = regex::Regex::new(pattern).map_err(|error| {
                RpcError::new(
                    "config_error",
                    json!({"path": path, "pattern": pattern, "message": error.to_string()}),
                    1,
                )
            })?;
            let regex = &regex;
            let logs = &logs;
            let probe = || async move { logs().await.iter().any(|line| regex.is_match(line)) };
            ("log", poll(deadline, probe).await)
        }
        #[cfg(unix)]
        ContainerCheck::Cmd { shell, cwd } => {
            let probe = || cmd_succeeds(shell, cwd, env, deadline);
            ("cmd", poll(deadline, probe).await)
        }
        #[cfg(windows)]
        ContainerCheck::Cmd {
            shell: command,
            cwd,
        } => {
            let Some(shell) = shell else {
                return Err(windows::shell_missing(
                    json!({"container": name, "check": "cmd"}),
                ));
            };
            let probe = || windows::cmd_succeeds(shell, command, cwd, env, deadline);
            ("cmd", poll(deadline, probe).await)
        }
    };
    passed.map_err(|()| {
        let mut data = json!({
            "container": name,
            "check": kind,
            "timeout_ms": ready.timeout.as_millis(),
        });
        if let (ContainerCheck::Http { status, .. }, Some((path, host))) =
            (&ready.check, &http_request)
        {
            last.describe(&mut data, path, host, *status);
        }
        RpcError::new("container_not_ready", data, 1)
    })
}

/// The last HTTP status an http probe received, so a timeout can say what the
/// service answered instead of only that it never passed. 0 means no answer yet.
#[derive(Default)]
struct LastStatus(AtomicU16);

impl LastStatus {
    /// Remembers `code` as the latest answer.
    fn record(&self, code: u16) {
        self.0.store(code, Ordering::Relaxed);
    }

    /// The latest answer, if any request got a status line.
    fn get(&self) -> Option<u16> {
        Some(self.0.load(Ordering::Relaxed)).filter(|code| *code != 0)
    }

    /// Adds the request the probe sent (`http_path`, `http_host`), `expected_status`
    /// (null when unset) and, once anything answered, `last_status` to timeout data.
    fn describe(
        &self,
        data: &mut serde_json::Value,
        path: &str,
        host: &str,
        expected: Option<u16>,
    ) {
        data["http_path"] = json!(path);
        data["http_host"] = json!(host);
        data["expected_status"] = json!(expected);
        if let Some(code) = self.get() {
            data["last_status"] = json!(code);
        }
    }
}

/// Retries a probe until it succeeds or the deadline passes.
async fn poll<F, Fut>(deadline: Instant, mut probe: F) -> Result<(), ()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = Instant::now();
    let mut attempts: u32 = 0;
    loop {
        attempts += 1;
        if probe().await {
            tracing::debug!(
                attempts,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "probe passed"
            );
            return Ok(());
        }
        tracing::trace!(attempts, "probe not passing yet");
        if Instant::now() + POLL >= deadline {
            tracing::debug!(
                attempts,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "probe timed out"
            );
            return Err(());
        }
        sleep(POLL).await;
    }
}

/// Reaches the port the way the proxy does, since some dev servers bind only one family.
async fn connect(port: u16) -> Option<TcpStream> {
    devlanes_loopback::connect_loopback(port, Duration::from_millis(500))
        .await
        .ok()
}

/// The port accepts a connection.
pub async fn tcp_open(port: u16) -> bool {
    connect(port).await.is_some()
}

/// The Host header an http probe sends: the configured `host`, else `127.0.0.1:<port>`.
fn host_header(port: u16, host: Option<&str>) -> String {
    host.map_or_else(|| format!("127.0.0.1:{port}"), str::to_owned)
}

/// Minimal HTTP/1.1 probe; avoids a client dependency for one status line. Sends
/// `host` as the Host header. Passes on exactly `expected`, or on any 2xx status when
/// `expected` is unset. Records any status it receives in `last`.
async fn http_ok(
    port: u16,
    path: &str,
    host: &str,
    expected: Option<u16>,
    last: &LastStatus,
) -> bool {
    let Some(mut stream) = connect(port).await else {
        return false;
    };
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: devlanes-ready\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut head = [0_u8; 64];
    let Ok(Ok(read)) = timeout(Duration::from_secs(2), stream.read(&mut head)).await else {
        return false;
    };
    let status = std::str::from_utf8(&head[..read])
        .ok()
        .and_then(|text| text.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    let Some(code) = status else {
        return false;
    };
    last.record(code);
    match expected {
        Some(want) => code == want,
        // Redirects are not followed, so a 3xx (often a login page) or a 404 from a
        // half-started router must not count as healthy.
        None => (200..300).contains(&code),
    }
}

/// `cmd` probes on Windows: Git's `sh.exe` in a job that ends the probe's
/// whole tree when the attempt is over or abandoned, and when the daemon dies.
#[cfg(windows)]
mod windows {
    use std::{collections::BTreeMap, io, path::Path, sync::Arc};

    use devlanes_proto::RpcError;
    use devlanes_sys_windows::{
        job::{Job, JobOptions},
        spawn::{self, Child, CreationFlags, Environment, Spawn, Stdio, Stream},
    };
    use serde_json::{Value, json};
    use tokio::time::{Instant, timeout};

    use super::CMD_ATTEMPT;

    /// The failure of a `cmd` check without Git's `sh.exe`: `spawn_failed`
    /// with reason `shell_missing`, plus the check's own `data`.
    pub(super) fn shell_missing(mut data: Value) -> RpcError {
        data["program"] = json!("/bin/sh");
        data["reason"] = json!("shell_missing");
        data["message"] = json!(
            "`ready.cmd` needs Git for Windows' sh.exe, and no `git` on PATH has one; install Git for Windows and restart the daemon"
        );
        RpcError::new("spawn_failed", data, 1)
    }

    /// Runs one `cmd` attempt with `shell`, like the Unix probe: the unit's
    /// environment only (plus the shell's variables and Git's tools at the
    /// end of `PATH`), no input or output, bounded by [`CMD_ATTEMPT`] and the
    /// deadline. Whatever is left of the attempt's process tree is ended when
    /// it returns or is dropped.
    pub(super) async fn cmd_succeeds(
        shell: &Path,
        command: &str,
        cwd: &Path,
        env: &BTreeMap<String, String>,
        deadline: Instant,
    ) -> bool {
        let limit = deadline
            .saturating_duration_since(Instant::now())
            .min(CMD_ATTEMPT);
        let mut env = env.clone();
        devlanes_shell_windows::shell_env(&mut env, shell);
        // The per-drive `=C:` entries cannot go into an environment block.
        env.retain(|name, _| !name.starts_with('='));
        let line = devlanes_shell_windows::shell_command_line(shell, command);
        let (shell, cwd) = (shell.to_path_buf(), cwd.to_path_buf());
        // Process creation touches the disk, so it stays off the executor.
        let started = tokio::task::spawn_blocking(move || start(&shell, &line, &cwd, &env)).await;
        let Ok(Ok((job, child))) = started else {
            return false;
        };
        let child = Arc::new(child);
        let waiter = tokio::task::spawn_blocking({
            let child = child.clone();
            move || child.wait()
        });
        let passed = matches!(timeout(limit, waiter).await, Ok(Ok(Ok(0))));
        // Closing the job's only handle ends every process still in it. A
        // dropped attempt drops `job` too, which also ends the blocking wait.
        drop(job);
        passed
    }

    /// Starts the shell in a new kill-on-close job, its standard streams on
    /// `NUL`. Blocking.
    fn start(
        shell: &Path,
        line: &std::ffi::OsStr,
        cwd: &Path,
        env: &BTreeMap<String, String>,
    ) -> io::Result<(Job, Child)> {
        let environment = Environment::new(env)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let job = Job::create(JobOptions {
            kill_on_close: true,
            die_on_unhandled_exception: true,
        })?;
        // Default flags: the probe shares the daemon's hidden console, so no
        // window opens and no console host is created per attempt.
        let child = spawn::spawn(&Spawn {
            program: shell,
            command_line: line,
            environment: Some(&environment),
            cwd: Some(cwd),
            stdio: Stdio::Handles {
                stdin: Stream::Null,
                stdout: Stream::Null,
                stderr: Stream::Null,
            },
            inherit: &[],
            job: Some(&job),
            flags: CreationFlags::default(),
        })?;
        Ok((job, child))
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[tokio::test]
    async fn tcp_and_http_probes_see_a_local_listener() -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = [0_u8; 512];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await;
            }
        });
        let last = LastStatus::default();
        assert!(tcp_open(port).await);
        assert!(http_ok(port, "/", "127.0.0.1", None, &last).await);
        assert!(http_ok(port, "/", "127.0.0.1", Some(204), &last).await);
        assert!(!http_ok(port, "/", "127.0.0.1", Some(200), &last).await);
        Ok(())
    }

    /// Serves HTTP on a local port, answering each request with the status its
    /// path names, such as `/404`. `/health` answers 200 only with
    /// `Host: api.example.invalid` and 404 otherwise, like an app that routes by host.
    async fn status_server() -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = [0_u8; 512];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or("/500");
                let code = if path == "/health" {
                    let own = request.contains("\r\nHost: api.example.invalid\r\n");
                    if own { "200" } else { "404" }.to_owned()
                } else {
                    path.strip_prefix('/').unwrap_or("500").to_owned()
                };
                let response = format!("HTTP/1.1 {code} Status\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok(port)
    }

    /// Without `status` only 2xx passes; with it, only that exact code does.
    #[tokio::test]
    async fn http_probe_needs_2xx_unless_a_status_is_set() -> Result<(), Box<dyn std::error::Error>>
    {
        let port = status_server().await?;
        let last = LastStatus::default();
        assert!(http_ok(port, "/200", "127.0.0.1", None, &last).await);
        assert!(http_ok(port, "/299", "127.0.0.1", None, &last).await);
        assert!(!http_ok(port, "/302", "127.0.0.1", None, &last).await);
        assert!(!http_ok(port, "/404", "127.0.0.1", None, &last).await);
        assert!(!http_ok(port, "/500", "127.0.0.1", None, &last).await);
        assert!(http_ok(port, "/404", "127.0.0.1", Some(404), &last).await);
        assert!(http_ok(port, "/302", "127.0.0.1", Some(302), &last).await);
        assert!(!http_ok(port, "/200", "127.0.0.1", Some(204), &last).await);
        Ok(())
    }

    /// The probe sends the configured Host header, and `127.0.0.1:<port>` without one.
    #[tokio::test]
    async fn http_probe_sends_the_host_header() -> Result<(), Box<dyn std::error::Error>> {
        let port = status_server().await?;
        let last = LastStatus::default();
        let default = host_header(port, None);
        assert_eq!(default, format!("127.0.0.1:{port}"));
        assert!(!http_ok(port, "/health", &default, None, &last).await);
        assert_eq!(last.get(), Some(404));
        let own = host_header(port, Some("api.example.invalid"));
        assert!(http_ok(port, "/health", &own, None, &last).await);
        Ok(())
    }

    /// A probe remembers the last status it received, and a closed port leaves it
    /// untouched.
    #[tokio::test]
    async fn http_probe_records_the_last_status() -> Result<(), Box<dyn std::error::Error>> {
        let port = status_server().await?;
        let last = LastStatus::default();
        assert_eq!(last.get(), None);
        assert!(!http_ok(port, "/302", "127.0.0.1", None, &last).await);
        assert_eq!(last.get(), Some(302));
        let closed = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let closed_port = closed.local_addr()?.port();
        drop(closed);
        assert!(!http_ok(closed_port, "/", "127.0.0.1", None, &last).await);
        assert_eq!(last.get(), Some(302));
        Ok(())
    }

    /// A container http check that times out says what it requested, what the
    /// service last answered and whether a status was configured.
    #[tokio::test]
    async fn container_http_timeout_reports_the_last_status()
    -> Result<(), Box<dyn std::error::Error>> {
        let port = status_server().await?;
        let ready = devlanes_resolve::ContainerReady {
            check: devlanes_resolve::ContainerCheck::Http {
                port: None,
                path: "/302".into(),
                status: None,
                host: None,
            },
            timeout: Duration::from_millis(400),
        };
        let ports = [("http".to_owned(), port)];
        let env = BTreeMap::new();
        #[cfg(unix)]
        let check = container_ready("web", &ready, &ports, &env, || async { Vec::new() });
        #[cfg(windows)]
        let check = container_ready("web", &ready, &ports, &env, None, || async { Vec::new() });
        let Err(error) = timeout(Duration::from_secs(10), check).await? else {
            return Err("a 302 must not pass a check without `status`".into());
        };
        assert_eq!(error.code, "container_not_ready");
        assert_eq!(error.data["check"], "http");
        assert_eq!(error.data["last_status"], 302);
        assert!(error.data["expected_status"].is_null());
        assert_eq!(error.data["http_path"], "/302");
        assert_eq!(error.data["http_host"], format!("127.0.0.1:{port}"));
        Ok(())
    }

    /// `cmd` probes see the unit's environment only, and a hanging attempt is cut
    /// at the deadline instead of blocking the check.
    #[cfg(unix)]
    #[tokio::test]
    async fn cmd_probe_uses_unit_env_and_is_bounded() {
        let cwd = std::env::temp_dir();
        let env = BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("DL_READY".to_owned(), "yes".to_owned()),
        ]);
        let soon = Instant::now() + Duration::from_secs(5);
        assert!(cmd_succeeds("test \"$DL_READY\" = yes", &cwd, &env, soon).await);
        // The daemon's own HOME is not inherited.
        assert!(!cmd_succeeds("test -n \"$HOME\"", &cwd, &env, soon).await);
        let started = Instant::now();
        let deadline = started + Duration::from_millis(300);
        assert!(!cmd_succeeds("sleep 30", &cwd, &env, deadline).await);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// The environment Windows programs need, `PATH` holding only System32,
    /// plus `extra`.
    #[cfg(windows)]
    fn windows_env(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let system = devlanes_sys_windows::file::system_directory().expect("System32");
        let mut env: BTreeMap<String, String> = ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"]
            .iter()
            .filter_map(|name| Some(((*name).to_owned(), std::env::var(name).ok()?)))
            .collect();
        env.insert("PATH".into(), system.to_string_lossy().into_owned());
        env.extend(
            extra
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
        env
    }

    /// `cmd` probes run through Git's `sh.exe` with the unit's environment
    /// only, find Git's tools through the directory appended to `PATH`, and
    /// pass only on exit 0.
    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_probe_runs_through_git_sh() {
        let shell = devlanes_shell_windows::windows_shell()
            .await
            .expect("Git for Windows' sh.exe");
        let cwd = std::env::temp_dir();
        let env = windows_env(&[("DL_READY", "yes")]);
        // An attempt is cut off after `CMD_ATTEMPT`, and a loaded runner has
        // stalled sh.exe for longer than that. A paused clock does not advance
        // while a blocking task runs, so the attempt's limit cannot pass while
        // it waits for sh.exe and only the exit code decides here;
        // `cmd_probe_timeout_ends_the_tree` covers the cut-off. A hang is
        // caught by a real-time guard per probe instead. Paused only after the
        // lookup above, which bounds `git --exec-path` with a real-time timeout.
        tokio::time::pause();
        let probe = |command: &'static str| {
            let (shell, cwd, env) = (shell.clone(), cwd.clone(), env.clone());
            async move {
                let deadline = Instant::now() + CMD_ATTEMPT;
                // Dropping `done` ends the guard; a panic drops the attempt,
                // whose job then ends sh.exe.
                let (done, cancelled) = std::sync::mpsc::channel::<()>();
                let guard = tokio::task::spawn_blocking(move || {
                    cancelled.recv_timeout(Duration::from_secs(60))
                });
                let passed = tokio::select! {
                    passed = windows::cmd_succeeds(&shell, command, &cwd, &env, deadline) => passed,
                    _ = guard => panic!("{command} did not exit within 60 s of real time"),
                };
                drop(done);
                passed
            }
        };
        assert!(probe("test \"$DL_READY\" = yes").await);
        // The daemon's own variables are not inherited.
        assert!(probe("test -z \"$USERPROFILE\"").await);
        assert!(probe("sleep 0 && exit 0").await);
        assert!(!probe("sleep 0 && exit 3").await);
    }

    /// An attempt cut at the deadline ends its whole process tree, including
    /// a program the shell started in the background.
    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_probe_timeout_ends_the_tree() {
        let shell = devlanes_shell_windows::windows_shell()
            .await
            .expect("Git for Windows' sh.exe");
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("listener");
        let port = listener.local_addr().expect("address").port().to_string();
        let env = windows_env(&[("DL_PORT", &port)]);
        // The background cat echoes a connection to the test until the test
        // closes its end, so only the job can end cat before that, and the
        // connection's end is cat's end. A cat the job missed exits when the
        // test drops the connection.
        let script = "cat <> /dev/tcp/127.0.0.1/$DL_PORT >&0 & wait";
        // Tokio does not advance a paused clock while a `spawn_blocking` task
        // (the attempt's wait for sh.exe) runs, so the deadline passes only
        // when the test advances it, however long sh.exe takes to start cat.
        // Paused only after the shell lookup, which has its own real-time
        // timeout.
        tokio::time::pause();
        let mut attempt = tokio::spawn(async move {
            let deadline = Instant::now() + CMD_ATTEMPT;
            let cwd = std::env::temp_dir();
            windows::cmd_succeeds(&shell, script, &cwd, &env, deadline).await
        });
        // Real time bounds the test, since the paused clock would not wait.
        // Dropping `done` ends the guard; a panic drops the attempt, whose job
        // then ends the tree.
        let (done, cancelled) = std::sync::mpsc::channel::<()>();
        let mut guard =
            tokio::task::spawn_blocking(move || cancelled.recv_timeout(Duration::from_secs(60)));
        let (mut background, _) = tokio::select! {
            accepted = listener.accept() => accepted.expect("accept"),
            _ = &mut guard => panic!("the background cat did not connect within 60 s"),
        };
        // The shell's forked child connects before it execs cat, so only an
        // echo shows that cat runs and holds the connection. A connection lost
        // on the way fails here instead of passing the final read.
        let mut byte = [0_u8; 1];
        background.write_all(b"x").await.expect("write");
        tokio::select! {
            echo = background.read_exact(&mut byte) => {
                echo.expect("the background cat closed its connection before the cut-off");
            }
            _ = &mut guard => panic!("the background cat did not echo within 60 s"),
        };
        // The attempt may not have set its deadline yet when cat echoes; each
        // advance passes whatever deadline it has set.
        let passed = loop {
            tokio::select! {
                finished = &mut attempt => break finished.expect("attempt"),
                _ = &mut guard => panic!("the attempt did not end at its deadline"),
                () = tokio::time::advance(CMD_ATTEMPT) => {}
            }
        };
        assert!(!passed);
        // With nothing left to echo, the read returns only once cat is gone.
        let ended = tokio::select! {
            _ = background.read(&mut byte) => true,
            _ = &mut guard => false,
        };
        drop(done);
        assert!(ended, "the background cat outlived its probe");
    }

    /// A `cmd` check without Git's `sh.exe` fails at once with
    /// `shell_missing`, for units and containers alike.
    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_probe_without_shell_fails_with_shell_missing() {
        let ready = devlanes_resolve::ContainerReady {
            check: devlanes_resolve::ContainerCheck::Cmd {
                shell: "exit 0".into(),
                cwd: std::env::temp_dir(),
            },
            timeout: Duration::from_secs(60),
        };
        let env = BTreeMap::new();
        let check = container_ready("db", &ready, &[], &env, None, || async { Vec::new() });
        let error = timeout(Duration::from_secs(5), check)
            .await
            .expect("fails at once")
            .expect_err("no shell");
        assert_eq!(error.code, "spawn_failed");
        assert_eq!(error.data["reason"], "shell_missing");
        assert_eq!(error.data["container"], "db");
        let check = Readiness::Cmd {
            command: "exit 0".into(),
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            shell: Some("sh.exe".into()),
        }
        .with_shell(None);
        assert!(matches!(check, Readiness::Cmd { shell: None, .. }));
    }

    #[tokio::test]
    async fn closed_port_is_not_ready() -> Result<(), Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        assert!(!tcp_open(port).await);
        Ok(())
    }
}
