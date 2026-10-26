//! Command [`devlanes_transport::Transport`]: runs a local command whose
//! standard input and output reach `devlanes tunnel --stdio` on a box, such as
//! `docker exec -i acme devlanes tunnel --stdio`. The SSH transport runs its
//! tunnels through it too.
//!
//! Each connect starts the command, reads its output up to the tunnel's
//! preamble line ([`devlanes_transport::read_preamble`]) and keeps what came
//! before as [`Opened::header`]. A failure carries the command's exit status
//! and the tail of its stderr. A program that cannot start, a protocol
//! mismatch, and before the first connection exit status 127 (a program the
//! command runs was not found) are permanent; the box daemon's stopped status
//! ([`devlanes_transport::EXIT_DAEMON_STOPPED`]) and other failures are
//! temporary. On Windows each command runs in a kill-on-close job.

#[cfg(unix)]
mod process;
#[cfg(windows)]
#[path = "process_windows.rs"]
mod process;

use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use async_trait::async_trait;
use devlanes_transport::{
    BoxDuplex, EXIT_DAEMON_STOPPED, Joined, PREAMBLE_LIMIT, PreambleError, Prefixed, Transport,
    TransportError, read_preamble,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// How long a command gets to write the tunnel preamble by default. Longer
/// than an SSH tunnel's wait, since the command may first start a container
/// or a codespace, or sign in.
pub const COMMAND_WAIT: Duration = Duration::from_secs(120);

/// The exit status shells and `docker exec` use when a program is not found.
pub const EXIT_NOT_FOUND: i32 = 127;

/// Bytes of the command's stderr kept while it runs; it is read on so the
/// command never blocks on a full pipe.
const STDERR_KEPT: usize = 64 * 1024;

/// Bytes of stderr an error message shows, from the end.
const STDERR_TAIL: usize = 4096;

/// How long stderr may stay open after the command exited, held by
/// something it started, before the exit is reported without the rest.
const STDERR_GRACE: Duration = Duration::from_secs(5);

/// Bytes of output before a failure an error message shows, from the end.
const SHOWN: usize = 4096;

/// Settings for [`CommandTransport`].
#[derive(Debug, Clone)]
pub struct CommandOptions {
    /// The box, as error messages name it.
    pub name: String,
    /// The process, as error messages name it when it fails, such as
    /// "the command for acme".
    pub what: String,
    /// How long the command gets to write the tunnel preamble.
    pub preamble_wait: Duration,
    /// A line the command prints once it is through its own setup, before the
    /// tunnel starts. Until it comes the wait is [`Ready::wait`]; after it,
    /// [`CommandOptions::preamble_wait`].
    pub ready: Option<Ready>,
    /// Variables set for the command, on top of this process's environment.
    pub env: Vec<(OsString, OsString)>,
    /// Variables of this process's environment the command does not get.
    pub env_remove: Vec<OsString>,
    /// The box connected before, such as one a restarted daemon restores:
    /// failures are classified as after a first successful connection.
    pub connected_before: bool,
    /// Ends every command started with these options when cancelled.
    pub stop: CancellationToken,
}

/// See [`CommandOptions::ready`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    /// The whole line, without its line break.
    pub line: String,
    /// How long the command gets to print it.
    pub wait: Duration,
}

impl CommandOptions {
    /// Defaults for the box `name`: messages call the process "the command
    /// for `name`", and the preamble wait is [`COMMAND_WAIT`].
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            what: format!("the command for {name}"),
            name,
            preamble_wait: COMMAND_WAIT,
            ready: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            connected_before: false,
            stop: CancellationToken::new(),
        }
    }
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// Its exit status, or why waiting for it failed.
    pub status: Result<ExitStatus, String>,
    /// The tail of its stderr.
    pub stderr: String,
}

impl Exit {
    /// The exit code, when the command exited with one.
    pub fn code(&self) -> Option<i32> {
        self.status.as_ref().ok().and_then(ExitStatus::code)
    }

    /// Whether the command exited successfully.
    pub fn success(&self) -> bool {
        self.status.as_ref().is_ok_and(ExitStatus::success)
    }
}

/// Command transport failures.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// The command line has no program.
    #[error("the command for {name} is empty; list the program and its arguments")]
    Empty {
        /// The box.
        name: String,
    },
    /// The program could not be started, for example because it is not installed.
    #[error("cannot run `{program}` for {name}: {source}")]
    Spawn {
        /// The box.
        name: String,
        /// The program, as given.
        program: String,
        /// Why starting it failed.
        #[source]
        source: io::Error,
    },
    /// The command exited with a failure, before or after the tunnel started.
    #[error("{}", exited(.name, .what, .exit, .output))]
    Exited {
        /// The box.
        name: String,
        /// The process, as messages name it.
        what: String,
        /// How it ended.
        exit: Exit,
        /// What it printed on stdout before the tunnel's preamble.
        output: Vec<u8>,
        /// Exit status [`EXIT_NOT_FOUND`] before the first connection.
        permanent: bool,
    },
    /// The box daemon is not running and the box's `remote.start_daemon` is off.
    #[error(
        "the devlanes daemon is not running on {name} and the box's user config turns off \
         starting it (`remote.start_daemon: false`); run `devlanes daemon start` there, or \
         `devlanes daemon install` to keep it running"
    )]
    DaemonStopped {
        /// The box.
        name: String,
    },
    /// The command did not print its [`Ready::line`] in time.
    #[error("{what} did not finish starting within {}s{}", .waited.as_secs(), shown(.output))]
    NotReady {
        /// The process, as messages name it.
        what: String,
        /// How long it had.
        waited: Duration,
        /// What it printed.
        output: Vec<u8>,
    },
    /// The command's output did not start with a preamble line of this protocol.
    #[error("{error}{}", detail(.stderr))]
    Preamble {
        /// What went wrong.
        error: PreambleError,
        /// See [`PreambleError::is_permanent`].
        permanent: bool,
        /// The tail of the command's stderr, when it had exited.
        stderr: String,
    },
}

impl CommandError {
    /// Reports whether reconnecting without user action is pointless.
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Empty { .. } | Self::Spawn { .. } => true,
            Self::Exited { permanent, .. } | Self::Preamble { permanent, .. } => *permanent,
            Self::DaemonStopped { .. } | Self::NotReady { .. } => false,
        }
    }

    /// What the command printed on stdout before it failed.
    pub fn output(&self) -> &[u8] {
        match self {
            Self::Exited { output, .. } | Self::NotReady { output, .. } => output,
            Self::Preamble { error, .. } => error.output(),
            Self::Empty { .. } | Self::Spawn { .. } | Self::DaemonStopped { .. } => &[],
        }
    }

    /// The error without the first `bytes` of [`CommandError::output`], for
    /// a caller that already read them, such as the probe before a
    /// [`Ready::line`], so the message shows only what came after.
    pub fn skip_output(self, bytes: usize) -> Self {
        let rest = |output: Vec<u8>| output.get(bytes..).unwrap_or_default().to_vec();
        match self {
            Self::Exited {
                name,
                what,
                exit,
                output,
                permanent,
            } => Self::Exited {
                name,
                what,
                exit,
                output: rest(output),
                permanent,
            },
            Self::NotReady {
                what,
                waited,
                output,
            } => Self::NotReady {
                what,
                waited,
                output: rest(output),
            },
            Self::Preamble {
                error,
                permanent,
                stderr,
            } => Self::Preamble {
                error: map_output(error, rest),
                permanent,
                stderr,
            },
            other => other,
        }
    }
}

impl From<CommandError> for TransportError {
    /// Permanent failures stop the link; everything else is retried.
    fn from(error: CommandError) -> Self {
        if error.is_permanent() {
            TransportError::permanent(error)
        } else {
            TransportError::transient(error)
        }
    }
}

/// The message of [`CommandError::Exited`].
fn exited(name: &str, what: &str, exit: &Exit, output: &[u8]) -> String {
    let detail = detail(&exit.stderr);
    let printed = shown(output);
    match &exit.status {
        Ok(status) if status.code() == Some(EXIT_NOT_FOUND) => format!(
            "{what} exited with {status}, so a program it runs was not found; check that \
             devlanes is installed on {name}{detail}{printed}"
        ),
        Ok(status) => format!("{what} exited with {status}{detail}{printed}"),
        Err(error) => format!("{what} failed: {error}{detail}{printed}"),
    }
}

/// `: <stderr>`, or nothing when it is empty.
fn detail(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

/// The output before a failure for an error message: its last [`SHOWN`]
/// bytes, or nothing.
fn shown(output: &[u8]) -> String {
    let start = output.len().saturating_sub(SHOWN);
    let text = String::from_utf8_lossy(&output[start..]);
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("; the box printed: {text}")
    }
}

/// The error a finished command reports, `None` for a successful exit.
/// `permanent` is only ever set later, by [`CommandTransport::classify`].
fn exit_error(name: &str, what: &str, exit: &Exit) -> Option<CommandError> {
    if exit.success() {
        return None;
    }
    if exit.code() == Some(EXIT_DAEMON_STOPPED) {
        return Some(CommandError::DaemonStopped {
            name: name.to_owned(),
        });
    }
    Some(CommandError::Exited {
        name: name.to_owned(),
        what: what.to_owned(),
        exit: exit.clone(),
        output: Vec::new(),
        permanent: false,
    })
}

/// A connected command: the stream starts right after the tunnel preamble.
#[derive(Debug)]
pub struct Opened {
    /// The command's stdout and stdin; bytes read past the preamble come first.
    pub stream: Prefixed<CommandStream>,
    /// What the command printed before the preamble, such as a login banner,
    /// [`Ready::line`] included.
    pub header: Vec<u8>,
}

/// What a command run to its end printed and how it ended, from [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Its exit status.
    pub status: ExitStatus,
    /// The last 64 KiB of its stdout.
    pub stdout: Vec<u8>,
    /// The last 64 KiB of its stderr.
    pub stderr: Vec<u8>,
}

/// [`Transport`] that runs one command per connection and speaks the mux
/// over its standard input and output.
///
/// The command inherits this process's environment and gets no terminal, so
/// a prompt it shows has nobody to answer it.
pub struct CommandTransport {
    argv: Vec<OsString>,
    options: CommandOptions,
    /// Set after the first preamble, or from the start with
    /// [`CommandOptions::connected_before`].
    connected: AtomicBool,
}

impl CommandTransport {
    /// Runs `argv` (program first) for each connection.
    pub fn new<I, S>(argv: I, options: CommandOptions) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            connected: AtomicBool::new(options.connected_before),
            options,
        }
    }

    /// The command, program first.
    pub fn argv(&self) -> &[OsString] {
        &self.argv
    }

    /// Starts the command and reads its output up to the tunnel preamble.
    pub async fn open(&self) -> Result<Opened, CommandError> {
        let first = !self.connected.load(Ordering::Relaxed);
        let (mut stream, exit) = self.spawn().await?;
        let mut header = Vec::new();
        if let Some(ready) = &self.options.ready {
            match tokio::time::timeout(ready.wait, read_ready(&mut stream, &ready.line)).await {
                Err(_) => {
                    return Err(CommandError::NotReady {
                        what: self.options.what.clone(),
                        waited: ready.wait,
                        output: stream.read_so_far(),
                    });
                }
                Ok(Err(error)) => return Err(self.classify(error, first, &exit)),
                Ok(Ok(lines)) => header = lines,
            }
        }
        match read_preamble(stream, self.options.preamble_wait).await {
            Ok(found) => {
                self.connected.store(true, Ordering::Relaxed);
                header.extend(found.header);
                Ok(Opened {
                    stream: found.stream,
                    header,
                })
            }
            Err(error) => Err(self.classify(with_header(error, header), first, &exit)),
        }
    }

    /// Starts the command with piped stdio and the task that waits for it.
    /// The second value receives its exit once it ends.
    async fn spawn(&self) -> Result<(CommandStream, Arc<Mutex<Option<Exit>>>), CommandError> {
        let Some(program) = self.argv.first().filter(|program| !program.is_empty()) else {
            return Err(CommandError::Empty {
                name: self.options.name.clone(),
            });
        };
        let spawn_error = |source| CommandError::Spawn {
            name: self.options.name.clone(),
            program: program.to_string_lossy().into_owned(),
            source,
        };
        let (process, pipes) = process::start(
            &self.argv,
            &self.options.env,
            &self.options.env_remove,
            true,
        )
        .await
        .map_err(spawn_error)?;
        let stdin = pipes
            .stdin
            .ok_or_else(|| spawn_error(io::Error::other("stdin was not piped")))?;
        let (exit_tx, exit_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let kept = Arc::new(Mutex::new(None));
        let shared = kept.clone();
        let stop = self.options.stop.clone();
        let stderr = pipes.stderr;
        tokio::spawn(async move {
            let mut process = process;
            {
                let waiting = async {
                    let exit = finish(&mut process, stderr).await;
                    *lock(&shared) = Some(exit.clone());
                    let _ = exit_tx.send(exit);
                };
                // Dropping the stream drops `stop_tx`. Leaving this task drops
                // `process`, which kills the command (on Windows its whole tree).
                tokio::select! {
                    () = waiting => {}
                    _ = stop_rx => {}
                    () = stop.cancelled() => {}
                }
            }
            drop(process);
        });
        let stream = CommandStream {
            io: Joined::new(pipes.stdout, stdin),
            unread: Vec::new(),
            seen: Vec::new(),
            name: self.options.name.clone(),
            what: self.options.what.clone(),
            exit: Some(exit_rx),
            finished: None,
            _stop: stop_tx,
        };
        Ok((stream, kept))
    }

    /// The error for a command whose output ended, failed or misbehaved
    /// before the preamble. `first` is set until the first connection, and
    /// `exit` holds the exit once the command ended.
    fn classify(
        &self,
        error: PreambleError,
        first: bool,
        exit: &Mutex<Option<Exit>>,
    ) -> CommandError {
        let error = match error {
            PreambleError::Io { source, output }
                if source
                    .get_ref()
                    .is_some_and(|inner| inner.is::<CommandError>()) =>
            {
                match source
                    .into_inner()
                    .map(|inner| inner.downcast::<CommandError>())
                {
                    Some(Ok(command)) => return with_output(*command, output, first),
                    Some(Err(inner)) => PreambleError::Io {
                        source: io::Error::other(inner),
                        output,
                    },
                    None => PreambleError::Ended { output },
                }
            }
            other => other,
        };
        let stderr = match &error {
            PreambleError::Ended { .. } => lock(exit)
                .as_ref()
                .map(|exit| exit.stderr.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        CommandError::Preamble {
            permanent: error.is_permanent(first),
            error,
            stderr,
        }
    }
}

#[async_trait]
impl Transport for CommandTransport {
    async fn connect(&self) -> Result<BoxDuplex, TransportError> {
        Ok(Box::new(self.open().await?.stream))
    }
}

/// A failure the stream reported, with what the command printed before it.
/// Exit status [`EXIT_NOT_FOUND`] is permanent only before the first
/// connection: afterwards the program may be missing for a moment, such as
/// during an upgrade on the box.
fn with_output(error: CommandError, output: Vec<u8>, first: bool) -> CommandError {
    match error {
        CommandError::Exited {
            name, what, exit, ..
        } => CommandError::Exited {
            permanent: first && exit.code() == Some(EXIT_NOT_FOUND),
            name,
            what,
            exit,
            output,
        },
        other => other,
    }
}

/// `error` with `header`, read before the preamble wait began, in front of
/// its output.
fn with_header(error: PreambleError, header: Vec<u8>) -> PreambleError {
    if header.is_empty() {
        return error;
    }
    map_output(error, |output| [header, output].concat())
}

/// `error` with `change` applied to its output.
fn map_output(error: PreambleError, change: impl FnOnce(Vec<u8>) -> Vec<u8>) -> PreambleError {
    match error {
        PreambleError::Ended { output } => PreambleError::Ended {
            output: change(output),
        },
        PreambleError::TooLong { output } => PreambleError::TooLong {
            output: change(output),
        },
        PreambleError::Silent { waited, output } => PreambleError::Silent {
            waited,
            output: change(output),
        },
        PreambleError::Protocol { found, output } => PreambleError::Protocol {
            found,
            output: change(output),
        },
        PreambleError::Io { source, output } => PreambleError::Io {
            source,
            output: change(output),
        },
    }
}

/// Reads `stream` up to and including the line `line`, returning what it
/// read; bytes after the line stay in the stream. Fails like
/// [`read_preamble`] when the output ends, fails or grows past
/// [`PREAMBLE_LIMIT`] first.
async fn read_ready(stream: &mut CommandStream, line: &str) -> Result<Vec<u8>, PreambleError> {
    let mut line_start = 0;
    let mut chunk = [0u8; 8192];
    loop {
        while let Some(end) = stream.seen[line_start..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let line_end = line_start + end;
            let text = &stream.seen[line_start..line_end];
            if text.strip_suffix(b"\r").unwrap_or(text) == line.as_bytes() {
                stream.unread = stream.seen.split_off(line_end + 1);
                return Ok(std::mem::take(&mut stream.seen));
            }
            line_start = line_end + 1;
        }
        if stream.seen.len() > PREAMBLE_LIMIT {
            return Err(PreambleError::TooLong {
                output: stream.read_so_far(),
            });
        }
        match stream.read(&mut chunk).await {
            Ok(0) => {
                return Err(PreambleError::Ended {
                    output: stream.read_so_far(),
                });
            }
            Ok(read) => stream.seen.extend_from_slice(&chunk[..read]),
            Err(source) => {
                return Err(PreambleError::Io {
                    source,
                    output: stream.read_so_far(),
                });
            }
        }
    }
}

/// Waits for `process` to exit and collects the tail of `stderr`, giving
/// stderr [`STDERR_GRACE`] to close after the exit.
async fn finish(process: &mut process::Process, stderr: process::ErrorReader) -> Exit {
    let mut collecting = tokio::spawn(collect_tail(stderr));
    let status = process.wait().await;
    let stderr = match tokio::time::timeout(STDERR_GRACE, &mut collecting).await {
        Ok(Ok(bytes)) => bytes,
        _ => {
            collecting.abort();
            Vec::new()
        }
    };
    Exit {
        status: status.map_err(|error| error.to_string()),
        stderr: tail(&stderr),
    }
}

/// Runs `argv` to its end with stdin from the null device, waiting at most
/// `limit`; `Ok(None)` when it did not finish in time, after ending it. For
/// short commands that share a transport's setup, such as an SSH probe.
pub async fn run<I, S>(
    argv: I,
    options: &CommandOptions,
    limit: Duration,
) -> Result<Option<Output>, CommandError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let Some(program) = argv.first().filter(|program| !program.is_empty()) else {
        return Err(CommandError::Empty {
            name: options.name.clone(),
        });
    };
    let spawn_error = |source| CommandError::Spawn {
        name: options.name.clone(),
        program: program.to_string_lossy().into_owned(),
        source,
    };
    let (mut process, pipes) = process::start(&argv, &options.env, &options.env_remove, false)
        .await
        .map_err(spawn_error)?;
    let outcome = {
        let finished = async {
            let (stdout, stderr) =
                tokio::join!(collect_tail(pipes.stdout), collect_tail(pipes.stderr));
            let status = process.wait().await?;
            Ok::<_, io::Error>(Output {
                status,
                stdout,
                stderr,
            })
        };
        tokio::select! {
            outcome = tokio::time::timeout(limit, finished) => outcome.ok(),
            () = options.stop.cancelled() => None,
        }
    };
    // Anything the command left running ends here (its job, on Windows).
    drop(process);
    match outcome {
        Some(Ok(output)) => Ok(Some(output)),
        Some(Err(source)) => Err(spawn_error(source)),
        None => Ok(None),
    }
}

/// Reads `reader` to its end, keeping the last [`STDERR_KEPT`] bytes.
async fn collect_tail(mut reader: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return kept,
            Ok(read) => {
                kept.extend_from_slice(&buffer[..read]);
                if kept.len() > STDERR_KEPT {
                    kept.drain(..kept.len() - STDERR_KEPT);
                }
            }
        }
    }
}

/// The last [`STDERR_TAIL`] bytes as lossy UTF-8, trimmed.
fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_TAIL);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

/// Locks `mutex`, ignoring poisoning: the guarded value is replaced whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The command's standard output and input as one duplex.
///
/// At end of file it waits for the command's exit status, so a command that
/// fails surfaces as an error carrying its stderr ([`CommandError::Exited`])
/// instead of a bare disconnect, and a stopped box daemon as
/// [`CommandError::DaemonStopped`]. Dropping it ends the command.
pub struct CommandStream {
    io: Joined<process::Reader, process::Writer>,
    /// Bytes read past [`Ready::line`], served before `io`.
    unread: Vec<u8>,
    /// Bytes [`read_ready`] has read so far.
    seen: Vec<u8>,
    name: String,
    what: String,
    exit: Option<oneshot::Receiver<Exit>>,
    /// Set at end of file: how the command ended, `None` for a clean end.
    finished: Option<Option<Exit>>,
    _stop: oneshot::Sender<()>,
}

impl std::fmt::Debug for CommandStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandStream")
            .field("name", &self.name)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl CommandStream {
    /// What [`read_ready`] read before a failure.
    fn read_so_far(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.seen)
    }

    /// Resolves end of file into success or the command's failure.
    fn poll_eof(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.finished.is_none() {
            let exit = match self.exit.as_mut() {
                Some(exit) => ready!(Pin::new(exit).poll(cx)).ok(),
                None => None,
            };
            self.exit = None;
            self.finished = Some(exit);
        }
        match self
            .finished
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|exit| exit_error(&self.name, &self.what, exit))
        {
            Some(error) => {
                Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionAborted, error)))
            }
            None => Poll::Ready(Ok(())),
        }
    }
}

impl AsyncRead for CommandStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.unread.is_empty() {
            let n = this.unread.len().min(buf.remaining());
            buf.put_slice(&this.unread[..n]);
            this.unread.drain(..n);
            return Poll::Ready(Ok(()));
        }
        if this.finished.is_some() {
            return this.poll_eof(cx);
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut this.io).poll_read(cx, buf))?;
        if buf.filled().len() == before && buf.remaining() > 0 {
            return this.poll_eof(cx);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for CommandStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests;
