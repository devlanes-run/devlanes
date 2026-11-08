//! Runs one devlanes command as a child process of the server, the way a shell
//! would, and collects its output.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt},
    sync::mpsc::UnboundedSender,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// How long a cancelled command gets to stop what it started, such as the
/// command and units of `with`, before it is killed.
const GRACE: Duration = Duration::from_secs(5);

/// [`GRACE`] when the server stops: hosts kill the server soon after they
/// close its stdin (the TypeScript SDK sends SIGTERM 2 s later), and no
/// command may outlive it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// How long the pipes may stay open once the command ended: a process it left
/// in the background can hold them for as long as it runs.
const PIPE_WAIT: Duration = Duration::from_millis(500);

/// `CREATE_NEW_PROCESS_GROUP`: the command leads a process group of its own,
/// so a Ctrl-Break can reach it and its children without reaching the server.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// `CREATE_SUSPENDED`: the command waits until it is in its job.
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

/// What one command printed.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// The exit status; a child ended by a signal reads `128 + n`, as in a shell.
    pub exit_code: i32,
    /// What `--output json` would have printed: one document, an array for NDJSON,
    /// `Null` for nothing.
    pub data: Value,
    /// The call was cancelled and the child interrupted before it finished.
    pub cancelled: bool,
}

/// The commands of a server's calls, so the server can stop them before it exits.
#[derive(Debug, Clone, Default)]
pub struct Calls {
    tracker: TaskTracker,
    shutdown: CancellationToken,
}

impl Calls {
    /// Interrupts every command still running, as a cancelled call does but
    /// with [`SHUTDOWN_GRACE`], and waits until they ended.
    pub async fn stop(&self) {
        self.shutdown.cancel();
        self.tracker.close();
        let _ = tokio::time::timeout(SHUTDOWN_GRACE + PIPE_WAIT * 4, self.tracker.wait()).await;
    }

    /// How many commands are running.
    pub fn running(&self) -> usize {
        self.tracker.len()
    }
}

/// How to run the commands of one server.
#[derive(Debug, Clone)]
pub struct Runner {
    pub exe: PathBuf,
    /// The child's whole environment.
    pub env: BTreeMap<String, String>,
    pub calls: Calls,
    /// The login shell's environment, for a lane that has none from a shell.
    pub shell_env: crate::shell_env::Cache,
}

impl Runner {
    /// This runner with the variables of `shell` set over its environment.
    pub fn with_shell_env(&self, shell: &BTreeMap<String, String>) -> Self {
        Self {
            env: crate::shell_env::merge(&self.env, shell),
            ..self.clone()
        }
    }

    /// Runs `exe --output agent ARGS` in `cwd` until it exits or `cancel` fires,
    /// which interrupts it. With `from_lane` the child sends the daemon no env.
    /// Stdin is closed and stdout piped, so the child never touches the MCP stream.
    /// With `phases`, for a command that starts a lane, the events it streams
    /// stay out of [`Output::data`] and the progress message of each
    /// `lane.phase` goes to `phases` as the command writes it.
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[String],
        from_lane: bool,
        cancel: &CancellationToken,
        phases: Option<UnboundedSender<String>>,
    ) -> std::io::Result<Output> {
        let mut command = self.command(cwd);
        command.args(["--output", "agent"]).args(args);
        if from_lane {
            command.env(devlanes_proto::ENV_FROM_LANE_ENV, "1");
        }
        self.teed(command, cancel, phases).await
    }

    /// Runs `exe --output json ARGS` for the app-only tools; an error the
    /// command reports is `{"error": ...}` in [`Output::data`]. Without
    /// `spawn` the command never starts the daemon and fails with
    /// `daemon_not_running` when none runs.
    pub async fn json(
        &self,
        cwd: &Path,
        args: &[String],
        spawn: bool,
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        let mut command = self.command(cwd);
        command.args(["--output", "json"]).args(args);
        if !spawn {
            command.env(devlanes_proto::NO_SPAWN_ENV, "1");
        }
        self.teed(command, cancel, None).await
    }

    /// Runs `command` with [`devlanes_proto::JSON_OUT_ENV`] naming a pipe
    /// that a task reads while the command runs; its JSON becomes
    /// [`Output::data`], and `phases` gets what [`Runner::run`] says.
    async fn teed(
        &self,
        mut command: tokio::process::Command,
        cancel: &CancellationToken,
        phases: Option<UnboundedSender<String>>,
    ) -> std::io::Result<Output> {
        let tee = Tee::open(phases).await?;
        command.env(devlanes_proto::JSON_OUT_ENV, tee.path());
        let output = self.collect(command, cancel).await;
        let written = tee.finish().await;
        let mut output = output?;
        output.data = parse_json(&written);
        Ok(output)
    }

    /// A command for the devlanes executable with the server's environment.
    pub fn command(&self, cwd: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.exe);
        command.env_clear().envs(&self.env);
        // Values the server itself was started with belong to its parent's call.
        for name in devlanes_proto::CALL_ENV {
            command.env_remove(name);
        }
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// Spawns `command` in a process group of its own (and on Windows a job)
    /// and reads its output until it exits or `cancel` fires. The child is
    /// watched by a task of [`Calls`], so it is interrupted and waited for
    /// also when the call is dropped or the server stops.
    async fn collect(
        &self,
        command: tokio::process::Command,
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        #[cfg(unix)]
        let spawned = {
            let mut command = command;
            command.process_group(0);
            Spawned {
                child: command.spawn()?,
            }
        };
        #[cfg(windows)]
        let spawned = Spawned::start(command)?;
        let stop = cancel.child_token();
        // Dropping the call, as rmcp does when the client leaves, stops the child too.
        let _dropped = stop.clone().drop_guard();
        let shutdown = self.calls.shutdown.clone();
        let watched = self.calls.tracker.spawn(watch(spawned, async move {
            tokio::select! {
                () = stop.cancelled() => GRACE,
                () = shutdown.cancelled() => SHUTDOWN_GRACE,
            }
        }));
        watched.await.map_err(std::io::Error::other)?
    }
}

/// Reads the command's output until it exits, or until `stopped` resolves
/// with the grace it gets, which interrupts it.
async fn watch(
    mut spawned: Spawned,
    stopped: impl Future<Output = Duration>,
) -> std::io::Result<Output> {
    let stdout = Pipe::read(spawned.child.stdout.take());
    let stderr = Pipe::read(spawned.child.stderr.take());
    let (status, cancelled) = tokio::select! {
        status = spawned.child.wait() => (Some(status?), false),
        grace = stopped => (spawned.interrupt(grace).await, true),
    };
    if !cancelled {
        spawned.release();
    }
    let (stdout, stderr) = tokio::join!(stdout.finish(), stderr.finish());
    Ok(Output {
        stdout,
        stderr,
        exit_code: status.map_or(130, exit_code),
        data: Value::Null,
        cancelled,
    })
}

/// A started command, and on Windows the job that holds it and what it starts.
/// On Unix its process group does that.
struct Spawned {
    child: tokio::process::Child,
    /// Ends every process in it when the server dies; `None` when Windows
    /// would not make one.
    #[cfg(windows)]
    job: Option<devlanes_sys_windows::job::Job>,
}

impl Spawned {
    /// Starts `command` in a new process group and in a job that ends it and
    /// everything it started when the server dies. The job allows breakaway
    /// so a daemon the command spawns leaves it; the cost is that Git for
    /// Windows' MSYS2 programs leave it too (see `devlanes_sys_windows::job`),
    /// and only the Ctrl-Break reaches those.
    #[cfg(windows)]
    fn start(mut command: tokio::process::Command) -> std::io::Result<Self> {
        use devlanes_sys_windows::job::{Job, JobOptions};
        let job = Job::create(JobOptions {
            kill_on_close: true,
            die_on_unhandled_exception: false,
        })
        .and_then(|job| job.allow_breakaway().map(|()| job))
        .inspect_err(|error| tracing::debug!(%error, "no job for the command"))
        .ok();
        // Suspended until it is in the job, so nothing it starts escapes.
        let suspended = if job.is_some() { CREATE_SUSPENDED } else { 0 };
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | suspended);
        let mut child = command.spawn()?;
        let (Some(job), Some(pid)) = (job, child.id()) else {
            return Ok(Self { child, job: None });
        };
        match job.adopt_suspended(pid) {
            Ok(true) => Ok(Self {
                child,
                job: Some(job),
            }),
            Ok(false) => {
                tracing::debug!("the command runs outside its job");
                Ok(Self { child, job: None })
            }
            Err(error) => {
                let _ = child.start_kill();
                Err(error)
            }
        }
    }

    /// Stops the command the way Ctrl-C stops one in a terminal, so `with`
    /// can stop its command and what it started; kills it after `grace`.
    /// What it left in its process group is killed too, so nothing keeps
    /// running after the call. Returns how it ended, if it could be waited for.
    #[cfg(unix)]
    async fn interrupt(&mut self, grace: Duration) -> Option<ExitStatus> {
        use rustix::process::{Pid, Signal, kill_process, kill_process_group};
        let pid = self
            .child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw);
        if let Some(pid) = pid {
            // The command forwards SIGINT to the command `with` runs.
            if kill_process(pid, Signal::INT).is_ok() {
                let until = tokio::time::Instant::now() + grace;
                while tokio::time::Instant::now() < until && !exited(pid) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            // Not reaped yet, so its PID, which is the group's ID, names no
            // other group. A daemon the command spawned is in a session of its own.
            let _ = kill_process_group(pid, Signal::KILL);
        }
        let _ = self.child.start_kill();
        self.child.wait().await.ok()
    }

    /// Stops the command with a Ctrl-Break to its process group, which the
    /// command of `with` shares (Ctrl-C cannot target one group), and
    /// terminates its job after `grace`, as Unix kills the group.
    #[cfg(windows)]
    async fn interrupt(&mut self, grace: Duration) -> Option<ExitStatus> {
        let sent = self
            .child
            .id()
            .is_some_and(|pid| devlanes_sys_windows::console::ctrl_break_group(pid).is_ok());
        let mut status = None;
        if sent {
            status = tokio::time::timeout(grace, self.child.wait())
                .await
                .ok()
                .and_then(Result::ok);
        }
        if let Some(job) = &self.job {
            let _ = job.terminate(1);
        }
        if status.is_none() {
            let _ = self.child.start_kill();
            status = self.child.wait().await.ok();
        }
        status
    }

    /// After a command that ended by itself, lets what it started in the
    /// background (a browser `url open` launched) outlive the call, as a
    /// shell would.
    fn release(self) {
        #[cfg(windows)]
        if let Some(job) = &self.job
            && let Err(error) = job.set_kill_on_close(false)
        {
            tracing::debug!(%error, "the command's background processes end with the call");
        }
    }
}

/// Whether the child `pid` has exited, without reaping it.
#[cfg(unix)]
fn exited(pid: rustix::process::Pid) -> bool {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG;
    !matches!(waitid(WaitId::Pid(pid), options), Ok(None))
}

/// A pipe read to its end in a task, whose bytes so far stay available when a
/// process the command left behind keeps the pipe open.
struct Pipe {
    bytes: Arc<Mutex<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Pipe {
    /// Starts reading `pipe`.
    fn read(pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let task = pipe.map(|mut pipe| {
            let bytes = bytes.clone();
            tokio::spawn(async move {
                let mut chunk = vec![0; 8192];
                while let Ok(read @ 1..) = pipe.read(&mut chunk).await {
                    bytes
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend_from_slice(&chunk[..read]);
                }
            })
        });
        Self { bytes, task }
    }

    /// What the pipe delivered by its end, or by [`PIPE_WAIT`] from now, as text.
    async fn finish(mut self) -> String {
        if let Some(task) = self.task.take() {
            let abort = task.abort_handle();
            if tokio::time::timeout(PIPE_WAIT, task).await.is_err() {
                abort.abort();
            }
        }
        let bytes = self.bytes.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Drop for Pipe {
    /// Stops reading when the output is not wanted, such as after waiting
    /// for the command failed.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The pipe a command writes its JSON copy to ([`devlanes_proto::JSON_OUT_ENV`]),
/// read by a task while the command runs. The CLI opens it once, without
/// letting the processes it starts inherit it, so once the command exited no
/// writer is left and the reader ends at the pipe's end, without a timeout.
struct Tee {
    /// A FIFO in a directory of its own.
    #[cfg(unix)]
    dir: Option<tempfile::TempDir>,
    #[cfg(unix)]
    path: PathBuf,
    /// The server's own write end, held until the command ended: the reader
    /// sees no end before the command opened the FIFO, or between its writes.
    #[cfg(unix)]
    writer: Option<tokio::net::unix::pipe::Sender>,
    /// A named pipe of one instance that only this machine can open.
    #[cfg(windows)]
    name: String,
    reader: Option<tokio::task::JoinHandle<String>>,
}

/// What opening a named pipe whose one instance a client took answers:
/// `ERROR_FILE_NOT_FOUND` once that client closed its end, `ERROR_PIPE_BUSY`
/// while it holds it.
#[cfg(windows)]
const PIPE_TAKEN: [i32; 2] = [2, 231];

impl Tee {
    /// Creates the FIFO and opens its read end, then a write end of the
    /// server's own; both non-blocking, reader first, since a FIFO takes a
    /// non-blocking writer only once it has a reader.
    #[cfg(unix)]
    async fn open(phases: Option<UnboundedSender<String>>) -> std::io::Result<Self> {
        let (dir, path) = tokio::task::spawn_blocking(|| {
            use std::os::unix::fs::PermissionsExt as _;
            let dir = tempfile::Builder::new()
                .prefix("devlanes-mcp-")
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()?;
            let path = dir.path().join("json");
            devlanes_sys::make_fifo(&path)?;
            Ok::<_, std::io::Error>((dir, path))
        })
        .await
        .map_err(std::io::Error::other)??;
        let options = tokio::net::unix::pipe::OpenOptions::new();
        let receiver = options.open_receiver(&path)?;
        let writer = options.open_sender(&path)?;
        Ok(Self {
            dir: Some(dir),
            path,
            writer: Some(writer),
            reader: Some(tokio::spawn(read(receiver, phases))),
        })
    }

    /// Creates the named pipe and asks Windows to connect it before the
    /// command starts: a command that writes and exits before the reader task
    /// runs still finds a connected reader, and its lines are kept.
    #[cfg(windows)]
    async fn open(phases: Option<UnboundedSender<String>>) -> std::io::Result<Self> {
        use std::{
            sync::atomic::{AtomicU64, Ordering},
            task::Poll,
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            r"\\.\pipe\devlanes-mcp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let server = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true)
            .access_outbound(false)
            .create(&name)?;
        let mut connected = Box::pin(async move { server.connect().await.map(|()| server) });
        // The first poll issues the connect; the spawn does not wait for a client.
        let early = std::future::poll_fn(|cx| Poll::Ready(connected.as_mut().poll(cx))).await;
        let reader = tokio::spawn(async move {
            let server = match early {
                Poll::Ready(server) => server,
                Poll::Pending => connected.await,
            };
            match server {
                Ok(server) => read(server, phases).await,
                Err(error) => {
                    tracing::debug!(%error, "the command's JSON pipe did not connect");
                    String::new()
                }
            }
        });
        Ok(Self {
            name,
            reader: Some(reader),
        })
    }

    /// The path to give the command.
    fn path(&self) -> &std::ffi::OsStr {
        #[cfg(unix)]
        {
            self.path.as_os_str()
        }
        #[cfg(windows)]
        {
            std::ffi::OsStr::new(&self.name)
        }
    }

    /// Once the command ended: what it wrote, read to the pipe's end.
    async fn finish(mut self) -> String {
        #[cfg(unix)]
        drop(self.writer.take());
        #[cfg(windows)]
        self.end_unconnected();
        let written = match self.reader.take() {
            Some(reader) => reader.await.unwrap_or_default(),
            None => String::new(),
        };
        #[cfg(unix)]
        if let Some(dir) = self.dir.take() {
            let _ = tokio::task::spawn_blocking(move || dir.close()).await;
        }
        written
    }

    /// Ends the reader of a pipe the command never opened by connecting and
    /// closing a client end in its place. A pipe that is taken had the
    /// command's end, which closed when the command exited.
    #[cfg(windows)]
    fn end_unconnected(&mut self) {
        let client = tokio::net::windows::named_pipe::ClientOptions::new()
            .read(false)
            .write(true)
            .open(&self.name);
        match client {
            Ok(client) => drop(client),
            Err(error)
                if error
                    .raw_os_error()
                    .is_some_and(|code| PIPE_TAKEN.contains(&code)) => {}
            Err(error) => {
                tracing::debug!(%error, "the command's JSON pipe could not be closed");
                if let Some(reader) = self.reader.take() {
                    reader.abort();
                }
            }
        }
    }
}

impl Drop for Tee {
    /// Stops reading when the call is dropped before the command ended.
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
    }
}

/// Reads the JSON lines of `pipe` to its end and returns them as text. With
/// `phases`, lines that are daemon events stay out of the text, and each
/// `lane.phase` among them sends its progress message.
async fn read(
    pipe: impl tokio::io::AsyncRead + Unpin,
    phases: Option<UnboundedSender<String>>,
) -> String {
    let mut lines = tokio::io::BufReader::new(pipe).split(b'\n');
    let mut written = String::new();
    while let Ok(Some(line)) = lines.next_segment().await {
        let line = String::from_utf8_lossy(&line);
        if let Some(phases) = &phases
            && let Ok(event) = serde_json::from_str::<devlanes_proto::Event>(&line)
        {
            if let Some(message) = crate::progress::message(&event) {
                let _ = phases.send(message);
            }
            continue;
        }
        written.push_str(&line);
        written.push('\n');
    }
    written
}

/// The code a shell would report for `status`.
fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

/// One JSON document as it is, NDJSON lines as an array, `Null` for no output.
/// Lines that are not JSON are skipped.
pub fn parse_json(text: &str) -> Value {
    let mut documents: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    match documents.len() {
        0 => Value::Null,
        1 => documents.remove(0),
        _ => Value::Array(documents),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A document stays as it is; a stream becomes an array.
    #[test]
    fn json_documents_and_streams() {
        assert_eq!(parse_json(""), Value::Null);
        assert_eq!(parse_json("{\"a\":1}\n"), json!({"a": 1}));
        assert_eq!(
            parse_json("{\"a\":1}\n{\"b\":2}\n"),
            json!([{"a": 1}, {"b": 2}])
        );
    }
}
