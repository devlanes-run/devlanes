//! Unix: the command is a tokio child process leading a process group of its
//! own. Dropping it kills the group, so what the command started and left in
//! it (the `ssh` that `gh codespace ssh` runs, a `docker` helper) ends with it,
//! as a kill-on-close job does on Windows. A program that moves itself to
//! another group or session leaves it, as a devlanes daemon does.

use std::ffi::OsString;
use std::io;
use std::process::{ExitStatus, Stdio};

use rustix::process::{Pid, Signal};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// What the command's standard output is read through.
pub(crate) type Reader = ChildStdout;
/// What the command's standard input is written through.
pub(crate) type Writer = ChildStdin;
/// What the command's standard error is read through.
pub(crate) type ErrorReader = ChildStderr;

/// A running command. Dropping it kills its process group and the process.
pub(crate) struct Process {
    child: Child,
    /// The group the command leads, whose id is the command's pid.
    group: Option<Pid>,
}

/// This side's ends of the command's standard streams.
pub(crate) struct Pipes {
    /// `None` when the command reads `/dev/null`.
    pub(crate) stdin: Option<Writer>,
    pub(crate) stdout: Reader,
    pub(crate) stderr: ErrorReader,
}

/// Starts `argv` with this process's environment minus `remove`, plus `env`,
/// with piped stdout and stderr, and piped stdin when `stdin` is set, as the
/// leader of a new process group. The program is looked up on `PATH` when it
/// has no directory.
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
        .process_group(0)
        .kill_on_drop(true);
    for name in remove {
        command.env_remove(name);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn()?;
    let group = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(Pid::from_raw);
    let missing = || io::Error::other("the command's standard streams were not piped");
    let pipes = Pipes {
        stdin: child.stdin.take(),
        stdout: child.stdout.take().ok_or_else(missing)?,
        stderr: child.stderr.take().ok_or_else(missing)?,
    };
    Ok((Process { child, group }, pipes))
}

impl Process {
    /// Waits for the command to exit.
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }
}

impl Drop for Process {
    /// Kills what is left of the command's group; tokio then kills and reaps
    /// the command itself. The group id stays taken while any member lives,
    /// so it can only name another group once all of them are gone, and then
    /// only one that a new process created with the reused id since.
    fn drop(&mut self) {
        if let Some(group) = self.group {
            let _ = rustix::process::kill_process_group(group, Signal::KILL);
        }
    }
}
