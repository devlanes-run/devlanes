//! Unix: the command is a tokio child process, killed when dropped.

use std::ffi::OsString;
use std::io;
use std::process::{ExitStatus, Stdio};

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// What the command's standard output is read through.
pub(crate) type Reader = ChildStdout;
/// What the command's standard input is written through.
pub(crate) type Writer = ChildStdin;
/// What the command's standard error is read through.
pub(crate) type ErrorReader = ChildStderr;

/// A running command. Dropping it kills the process.
pub(crate) struct Process(Child);

/// This side's ends of the command's standard streams.
pub(crate) struct Pipes {
    /// `None` when the command reads `/dev/null`.
    pub(crate) stdin: Option<Writer>,
    pub(crate) stdout: Reader,
    pub(crate) stderr: ErrorReader,
}

/// Starts `argv` with this process's environment minus `remove`, plus `env`,
/// with piped stdout and stderr, and piped stdin when `stdin` is set. The
/// program is looked up on `PATH` when it has no directory.
pub(crate) async fn start(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    remove: &[OsString],
    stdin: bool,
) -> io::Result<(Process, Pipes)> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in remove {
        command.env_remove(name);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn()?;
    let missing = || io::Error::other("the command's standard streams were not piped");
    let pipes = Pipes {
        stdin: child.stdin.take(),
        stdout: child.stdout.take().ok_or_else(missing)?,
        stderr: child.stderr.take().ok_or_else(missing)?,
    };
    Ok((Process(child), pipes))
}

impl Process {
    /// Waits for the command to exit.
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.0.wait().await
    }
}
