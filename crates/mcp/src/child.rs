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
use tokio::io::AsyncReadExt;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// How long a cancelled command gets to stop what it started, such as the
/// command and units of `with`, before it is killed.
const GRACE: Duration = Duration::from_secs(5);

/// How long the pipes may stay open once the command ended: a process it left
/// in the background can hold them for as long as it runs.
const PIPE_WAIT: Duration = Duration::from_millis(500);

/// `CREATE_NEW_PROCESS_GROUP`: the command leads a process group of its own,
/// so a Ctrl-Break can reach it and its children without reaching the server.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

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
    /// Interrupts every command still running, as a cancelled call does, and
    /// waits until they ended.
    pub async fn stop(&self) {
        self.shutdown.cancel();
        self.tracker.close();
        let _ = tokio::time::timeout(GRACE + PIPE_WAIT * 4, self.tracker.wait()).await;
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
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[String],
        from_lane: bool,
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        let mut command = self.command(cwd);
        command.args(["--output", "agent"]).args(args);
        if from_lane {
            command.env(devlanes_proto::ENV_FROM_LANE_ENV, "1");
        }
        self.teed(command, cancel).await
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
        self.teed(command, cancel).await
    }

    /// Runs `command` with [`devlanes_proto::JSON_OUT_ENV`] naming a temporary
    /// file, whose JSON becomes [`Output::data`].
    async fn teed(
        &self,
        mut command: tokio::process::Command,
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        let json = tokio::task::spawn_blocking(|| {
            tempfile::Builder::new()
                .prefix("devlanes-mcp-")
                .suffix(".json")
                .tempfile()
                .map(tempfile::NamedTempFile::into_temp_path)
        })
        .await
        .map_err(std::io::Error::other)??;
        command.env(devlanes_proto::JSON_OUT_ENV, &*json);
        let mut output = self.collect(command, cancel).await;
        if let Ok(output) = &mut output {
            let written = tokio::fs::read_to_string(&*json).await.unwrap_or_default();
            output.data = parse_json(&written);
        }
        // The temporary file goes on a blocking thread too.
        let _ = tokio::task::spawn_blocking(move || json.close()).await;
        output
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

    /// Spawns `command` in a process group of its own and reads its output
    /// until it exits or `cancel` fires. The child is watched by a task of
    /// [`Calls`], so it is interrupted and waited for also when the call is
    /// dropped or the server stops.
    async fn collect(
        &self,
        mut command: tokio::process::Command,
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
        let child = command.spawn()?;
        let stop = cancel.child_token();
        // Dropping the call, as rmcp does when the client leaves, stops the child too.
        let _dropped = stop.clone().drop_guard();
        let shutdown = self.calls.shutdown.clone();
        let watched = self.calls.tracker.spawn(watch(child, async move {
            tokio::select! {
                () = stop.cancelled() => {}
                () = shutdown.cancelled() => {}
            }
        }));
        watched.await.map_err(std::io::Error::other)?
    }
}

/// Reads `child`'s output until it exits, or until `stopped` resolves, which
/// interrupts it.
async fn watch(
    mut child: tokio::process::Child,
    stopped: impl Future<Output = ()>,
) -> std::io::Result<Output> {
    let stdout = Pipe::read(child.stdout.take());
    let stderr = Pipe::read(child.stderr.take());
    let (status, cancelled) = tokio::select! {
        status = child.wait() => (Some(status?), false),
        () = stopped => (interrupt(&mut child).await, true),
    };
    let (stdout, stderr) = tokio::join!(stdout.finish(), stderr.finish());
    Ok(Output {
        stdout,
        stderr,
        exit_code: status.map_or(130, exit_code),
        data: Value::Null,
        cancelled,
    })
}

/// Stops `child` the way Ctrl-C stops a command in a terminal, so `with` can
/// stop its command and what it started; kills it after [`GRACE`]. Then
/// processes it left in its group are killed, so none keeps running after the
/// call. Returns how it ended, if it could be waited for.
async fn interrupt(child: &mut tokio::process::Child) -> Option<ExitStatus> {
    let group = child.id();
    let waited = if group.is_some_and(send_interrupt) {
        tokio::time::timeout(GRACE, child.wait()).await.ok()
    } else {
        None
    };
    let status = match waited {
        Some(status) => status.ok(),
        None => {
            let _ = child.start_kill();
            child.wait().await.ok()
        }
    };
    #[cfg(unix)]
    if let Some(group) = group.and_then(|pid| i32::try_from(pid).ok()) {
        // The daemon a command spawned has left the group (setsid) by now.
        if let Some(group) = rustix::process::Pid::from_raw(group) {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }
    status
}

/// SIGINT to the command, which forwards it to the command `with` runs.
#[cfg(unix)]
fn send_interrupt(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::kill_process(pid, rustix::process::Signal::INT).is_ok())
}

/// Ctrl-Break to the command's process group: the command and what it runs in
/// the foreground, such as the command of `with`, share it. Ctrl-C cannot
/// target one group.
#[cfg(windows)]
fn send_interrupt(pid: u32) -> bool {
    devlanes_sys_windows::console::ctrl_break_group(pid).is_ok()
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
