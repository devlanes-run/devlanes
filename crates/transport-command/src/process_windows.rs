//! Windows: the command starts inside a kill-on-close job, so whatever it
//! starts (`docker.exe` talking to a helper, `wsl.exe` and its relay, a
//! `ProxyCommand`) ends with it, and with this process if it dies. It shares
//! this process's console when there is one (the daemon's is hidden), so no
//! window opens; without a console it gets a windowless one of its own.

use std::ffi::{OsStr, OsString};
use std::io::{self, PipeReader, PipeWriter};
use std::os::windows::io::{AsHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;

use devlanes_sys_windows::job::{Job, JobOptions};
use devlanes_sys_windows::spawn::{self, Child, CreationFlags, Environment, Spawn, Stdio, Stream};
use tokio::fs::File;

/// What the command's standard output is read through.
pub(crate) type Reader = File;
/// What the command's standard input is written through.
pub(crate) type Writer = File;
/// What the command's standard error is read through.
pub(crate) type ErrorReader = File;

/// A running command. Dropping it closes its job, which ends the command
/// and everything it started.
pub(crate) struct Process {
    _job: Job,
    child: Arc<Child>,
}

/// This side's ends of the command's standard streams.
pub(crate) struct Pipes {
    /// `None` when the command reads `NUL`.
    pub(crate) stdin: Option<Writer>,
    pub(crate) stdout: Reader,
    pub(crate) stderr: ErrorReader,
}

/// Starts `argv` with this process's environment minus `remove`, plus `env`
/// (names compared without case), with piped stdout and stderr, and piped
/// stdin when `stdin` is set. A program without a directory is looked up on
/// `PATH`, with `.exe` or `.com` added when it has no extension. Process
/// creation touches the disk, so it runs off the executor.
pub(crate) async fn start(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    remove: &[OsString],
    stdin: bool,
) -> io::Result<(Process, Pipes)> {
    let argv = argv.to_vec();
    let env = env.to_vec();
    let remove = remove.to_vec();
    tokio::task::spawn_blocking(move || start_blocking(&argv, &env, &remove, stdin))
        .await
        .map_err(io::Error::other)?
}

/// [`start`] on the calling thread.
fn start_blocking(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    remove: &[OsString],
    stdin: bool,
) -> io::Result<(Process, Pipes)> {
    let program = argv
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let program = locate(program)?;
    let line = command_line(argv)?;
    let environment = environment(env, remove)?;
    let job = Job::create(JobOptions {
        kill_on_close: true,
        die_on_unhandled_exception: true,
    })?;
    let (out_read, out_write) = spawn::pipe()?;
    let (err_read, err_write) = spawn::pipe()?;
    let stdin_pipe = if stdin { Some(spawn::pipe()?) } else { None };
    let child = spawn::spawn(&Spawn {
        program: &program,
        command_line: &line,
        environment: environment.as_ref(),
        cwd: None,
        stdio: Stdio::Handles {
            stdin: stdin_pipe
                .as_ref()
                .map_or(Stream::Null, |(read, _)| Stream::Handle(read.as_handle())),
            stdout: Stream::Handle(out_write.as_handle()),
            stderr: Stream::Handle(err_write.as_handle()),
        },
        inherit: &[],
        job: Some(&job),
        flags: CreationFlags {
            no_window: !devlanes_sys_windows::console::has_console(),
            ..CreationFlags::default()
        },
    })?;
    // The command holds its own ends now; this process keeps only the other
    // ones, so the command's exit reaches the readers as end of file.
    drop((out_write, err_write));
    let stdin = stdin_pipe.map(|(read, write)| {
        drop(read);
        writer(write)
    });
    Ok((
        Process {
            _job: job,
            child: Arc::new(child),
        },
        Pipes {
            stdin,
            stdout: reader(out_read),
            stderr: reader(err_read),
        },
    ))
}

impl Process {
    /// Waits for the command to exit, off the executor.
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || child.wait())
            .await
            .map_err(io::Error::other)?
            .map(ExitStatus::from_raw)
    }
}

/// `argv` as one command line, each argument quoted for the usual
/// `CommandLineToArgvW` rules.
pub(crate) fn command_line(argv: &[OsString]) -> io::Result<OsString> {
    let quoted = argv
        .iter()
        .map(|arg| {
            arg.to_str().map(spawn::quote_arg).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not valid Unicode", arg.to_string_lossy()),
                )
            })
        })
        .collect::<io::Result<Vec<String>>>()?;
    Ok(OsString::from(quoted.join(" ")))
}

/// The child's environment block, or `None` to inherit this process's when
/// nothing changes. The per-drive `=C:` entries cannot go into a block and
/// are left out.
fn environment(
    env: &[(OsString, OsString)],
    remove: &[OsString],
) -> io::Result<Option<Environment>> {
    if env.is_empty() && remove.is_empty() {
        return Ok(None);
    }
    let same = |a: &OsStr, b: &OsStr| match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => a == b,
    };
    let dropped = |name: &OsStr| {
        name.to_str().is_none_or(|name| name.starts_with('='))
            || remove.iter().any(|gone| same(name, gone))
            || env.iter().any(|(own, _)| same(name, own))
    };
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| !dropped(name))
        .collect();
    variables.extend(env.iter().cloned());
    Environment::new(variables)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Resolves a program without a directory on `PATH`, since `CreateProcessW`
/// does not search it for an application name; one with a directory is used
/// as given. `NotFound` names what was looked for. Blocking.
fn locate(program: &OsStr) -> io::Result<PathBuf> {
    let path = Path::new(program);
    if path
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Ok(path.to_path_buf());
    }
    let names: Vec<PathBuf> = if path.extension().is_some() {
        vec![path.to_path_buf()]
    } else {
        vec![path.with_extension("exe"), path.with_extension("com")]
    };
    let dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    dirs.iter()
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not on PATH", names[0].display()),
            )
        })
}

/// An async reader over the reading end of an anonymous pipe. Anonymous pipes
/// have no overlapped I/O, so tokio reads them on its blocking pool, as it
/// does a child's standard streams.
fn reader(pipe: PipeReader) -> File {
    File::from_std(std::fs::File::from(OwnedHandle::from(pipe)))
}

/// An async writer over the writing end of an anonymous pipe; see [`reader`].
fn writer(pipe: PipeWriter) -> File {
    File::from_std(std::fs::File::from(OwnedHandle::from(pipe)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each argument survives the command line whole, quotes and spaces included.
    #[test]
    fn command_lines_quote_each_argument() {
        let argv: Vec<OsString> = [
            r"C:\Windows\System32\OpenSSH\ssh.exe",
            "-o",
            r#"IdentityFile="C:\Users\a b\key""#,
            r#"sh -c 'printf "%s" x'"#,
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let line = command_line(&argv).expect("unicode");
        assert_eq!(
            line.to_string_lossy(),
            r#"C:\Windows\System32\OpenSSH\ssh.exe -o "IdentityFile=\"C:\Users\a b\key\"" "sh -c 'printf \"%s\" x'""#
        );
    }

    /// Bare names are searched on `PATH`; a name found nowhere is `NotFound`
    /// and never left for `CreateProcessW` to look up in the working directory.
    #[test]
    fn bare_names_are_searched_on_path() {
        let found = locate(OsStr::new("cmd")).expect("cmd.exe is on PATH");
        assert!(found.is_absolute(), "{}", found.display());
        let explicit = Path::new(r"C:\tools\acme.exe");
        assert_eq!(
            locate(explicit.as_os_str()).expect("kept"),
            explicit.to_path_buf()
        );
        let missing = locate(OsStr::new("devlanes-no-such-program")).expect_err("missing");
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        assert!(
            missing
                .to_string()
                .contains("devlanes-no-such-program.exe is not on PATH"),
            "{missing}"
        );
    }
}
