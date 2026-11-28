//! Starting `devlanes-gui view`, the viewer of one GUI service on this
//! machine, for `devlanes gui open` and the daemon's auto-open alike.
//!
//! The viewer is a client process, not a managed unit: it is no unit and no
//! member of a unit's run, it is not in `debug top`, and its input counts as
//! lane activity only through the GUI host, like any viewer's. It outlives
//! the command that started it, so it is started on its own (its own session
//! on Unix; no console, its own process group and outside the terminal's job
//! where that job allows it on Windows), with only null and its log file as
//! its standard streams: closing the terminal or a Ctrl-C there does not end
//! it, and no pipe of its starter stays open in it. Its log goes to a file of
//! the devlanes home.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use devlanes_core::ProcessIdentity;

/// What a viewer is started for.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The `devlanes-gui` executable.
    pub program: PathBuf,
    /// The GUI host's address on this machine.
    pub address: SocketAddr,
    /// The service as `lane:unit`.
    pub service: String,
    /// The directory of the one-viewer locks
    /// ([`devlanes_gui_client::instance`]).
    pub instance_dir: PathBuf,
    /// Started by the daemon's auto-open.
    pub auto: bool,
    /// The `devlanes` executable the viewer asks whether the service still
    /// runs while it cannot reconnect.
    pub devlanes: Option<PathBuf>,
    /// Where the viewer's log goes; new lines are appended.
    pub log: PathBuf,
    /// Variables to set for the viewer on top of the starter's environment.
    pub env: Vec<(String, String)>,
}

impl Launch {
    /// The arguments after the program.
    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "view".to_owned(),
            "--address".to_owned(),
            self.address.to_string(),
            "--service".to_owned(),
            self.service.clone(),
            "--instance-dir".to_owned(),
            self.instance_dir.to_string_lossy().into_owned(),
        ];
        if self.auto {
            args.push("--auto".to_owned());
        }
        if let Some(devlanes) = &self.devlanes {
            args.extend([
                "--devlanes".to_owned(),
                devlanes.to_string_lossy().into_owned(),
                "--unit".to_owned(),
                self.service.clone(),
            ]);
        }
        args
    }

    /// The viewer's environment: the starter's without the variables of one
    /// `devlanes mcp` command and the daemon's own names for the desktop,
    /// then [`Launch::env`].
    fn environment(&self) -> Vec<(String, String)> {
        let dropped = |name: &str| {
            devlanes_proto::CALL_ENV
                .iter()
                .any(|other| same_name(name, other))
                || devlanes_proto::VIEWER_DISPLAY_ENV
                    .iter()
                    .any(|(_, kept)| same_name(name, kept))
                || self.env.iter().any(|(set, _)| same_name(name, set))
        };
        let mut variables: Vec<(String, String)> = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            // Windows' per-drive variables (`=C:`) are no variables to pass on.
            .filter(|(name, _)| !name.starts_with('=') && !dropped(name))
            .collect();
        variables.extend(self.env.iter().cloned());
        variables
    }
}

/// Whether two variable names are one variable: by exact name on Unix.
#[cfg(unix)]
fn same_name(name: &str, other: &str) -> bool {
    name == other
}

/// Whether two variable names are one variable: Windows ignores case.
#[cfg(windows)]
fn same_name(name: &str, other: &str) -> bool {
    name.eq_ignore_ascii_case(other)
}

/// A started viewer, kept to end it and to reap it by its pid.
#[derive(Debug)]
pub struct Process {
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(windows)]
    child: devlanes_sys_windows::spawn::Child,
}

impl Process {
    /// The process id.
    pub fn pid(&self) -> u32 {
        #[cfg(unix)]
        return self.child.id();
        #[cfg(windows)]
        return self.child.pid();
    }

    /// The process identity, to end it with a check that the pid is still it.
    pub fn identity(&self) -> Option<ProcessIdentity> {
        #[cfg(unix)]
        return devlanes_sys::identity(self.child.id()).ok().flatten();
        #[cfg(windows)]
        return Some(ProcessIdentity {
            pid: self.child.pid(),
            sid: 0,
            start_time: self.child.start_time(),
        });
    }

    /// The exit code once the process ended (reaping it on Unix), `None`
    /// while it runs. A Unix process ended by a signal reads as -1.
    pub fn exited(&mut self) -> std::io::Result<Option<i32>> {
        #[cfg(unix)]
        return Ok(self
            .child
            .try_wait()?
            .map(|status| status.code().unwrap_or(-1)));
        #[cfg(windows)]
        return Ok(self.child.exit_code()?.map(|code| code as i32));
    }
}

/// The directory of the viewer locks in the devlanes home `home`.
pub fn instance_dir(home: &Path) -> PathBuf {
    home.join("run").join("gui-viewers")
}

/// The log file of the viewer of `key` in the devlanes home `home`.
pub fn log_path(home: &Path, key: &str) -> PathBuf {
    home.join("logs").join(format!("gui-viewer-{key}.log"))
}

/// Starts the viewer. Blocking: it opens the log file and spawns.
pub fn spawn(launch: &Launch) -> std::io::Result<Process> {
    if let Some(parent) = launch.log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Appended: two starts racing for one service (the second one defers)
    // must not cut each other's lines.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&launch.log)?;
    detach(&launch.program, &launch.args(), &launch.environment(), log)
}

/// Spawns `program` with `args` and exactly `environment`, standard input
/// and output on null and standard error on `log`, in a session of its own.
/// Rust opens every descriptor close-on-exec, so only those three reach it.
#[cfg(unix)]
fn detach(
    program: &Path,
    args: &[String],
    environment: &[(String, String)],
    log: std::fs::File,
) -> std::io::Result<Process> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    devlanes_sys::new_session(&mut command);
    Ok(Process {
        child: command.spawn()?,
    })
}

/// Spawns `program` with `args` and exactly `environment`, null as standard
/// input and output and `log` as standard error, and no other handle: a
/// `std::process` spawn would pass every inheritable handle of this process,
/// such as the caller's output pipe, which would then stay open as long as
/// the viewer runs. No console, a process group of its own, and outside this
/// process's job when the job allows breakaway.
#[cfg(windows)]
fn detach(
    program: &Path,
    args: &[String],
    environment: &[(String, String)],
    log: std::fs::File,
) -> std::io::Result<Process> {
    use std::ffi::OsString;
    use std::os::windows::io::AsHandle;

    use devlanes_sys_windows::spawn::{self, CreationFlags, Environment, Spawn, Stdio, Stream};

    let environment = Environment::new(environment.iter().cloned())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let program_text = program.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the viewer's path is not UTF-8",
        )
    })?;
    let mut command_line = OsString::from(spawn::quote_arg(program_text));
    for arg in args {
        command_line.push(" ");
        command_line.push(spawn::quote_arg(arg));
    }
    let spec = |breakaway_from_job: bool| Spawn {
        program,
        command_line: &command_line,
        environment: Some(&environment),
        cwd: None,
        stdio: Stdio::Handles {
            stdin: Stream::Null,
            stdout: Stream::Null,
            stderr: Stream::Handle(log.as_handle()),
        },
        inherit: &[],
        job: None,
        flags: CreationFlags {
            detached_process: true,
            new_process_group: true,
            breakaway_from_job,
            ..CreationFlags::default()
        },
    };
    let child = match spawn::spawn(&spec(true)) {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            spawn::spawn(&spec(false))?
        }
        spawned => spawned?,
    };
    Ok(Process { child })
}

/// The desktop variables for a viewer the daemon starts on Linux and other
/// Unix desktops: the ones the CLI handed to it under the viewer's names,
/// else the daemon's own (a daemon a desktop session's service manager
/// started has them). `None` when there is no display to open windows on.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn display_env() -> Option<Vec<(String, String)>> {
    let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let handed: Vec<(String, String)> = devlanes_proto::VIEWER_DISPLAY_ENV
        .iter()
        .filter_map(|(name, kept)| Some(((*name).to_owned(), read(kept)?)))
        .collect();
    let own: Vec<(String, String)> = devlanes_proto::VIEWER_DISPLAY_ENV
        .iter()
        .filter_map(|(name, _)| Some(((*name).to_owned(), read(name)?)))
        .collect();
    let usable = |vars: &[(String, String)]| {
        devlanes_gui_client::display::has_display(|name| {
            vars.iter()
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone().into())
        })
    };
    [handed, own].into_iter().find(|vars| usable(vars))
}

/// macOS and Windows viewers open on the session's desktop.
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn display_env() -> Option<Vec<(String, String)>> {
    Some(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set in the helper process the test below starts.
    const HELPER_ENV: &str = "DEVLANES_TEST_VIEWER_SPAWN";

    /// A program that runs for a while and ignores the viewer's arguments.
    #[cfg(windows)]
    fn sleeper() -> (PathBuf, Vec<String>) {
        let system = devlanes_sys_windows::file::system_directory().expect("System32");
        (
            system.join("WindowsPowerShell\\v1.0\\powershell.exe"),
            ["-NoProfile", "-Command", "Start-Sleep 30"]
                .map(str::to_owned)
                .to_vec(),
        )
    }

    /// A program that runs for a while.
    #[cfg(unix)]
    fn sleeper() -> (PathBuf, Vec<String>) {
        (PathBuf::from("sleep"), vec!["30".to_owned()])
    }

    /// The helper side of the test below: started with its standard output
    /// on a pipe, it starts a long-running program the way a viewer is
    /// started, writes that program's pid where the test asked, and exits.
    #[test]
    fn detach_helper() {
        let Some(pid_file) = std::env::var_os(HELPER_ENV) else {
            return;
        };
        let dir = tempfile::tempdir().expect("dir");
        let log = std::fs::File::create(dir.path().join("log")).expect("log");
        let (program, args) = sleeper();
        let environment: Vec<(String, String)> = std::env::vars()
            .filter(|(name, _)| !name.starts_with('=') && name != HELPER_ENV)
            .collect();
        let process = detach(&program, &args, &environment, log).expect("spawned");
        std::fs::write(pid_file, process.pid().to_string()).expect("pid");
        println!("started");
    }

    /// The caller of `gui open` reads its output to the end: a viewer started
    /// while the caller's pipe is this process's output must not hold that
    /// pipe, or `$(devlanes gui open)` and the TUI's `w` would wait for the
    /// viewer to end. The helper's pipe closes as the helper exits, while the
    /// program it started keeps running.
    #[test]
    fn a_started_viewer_holds_no_pipe_of_its_starter() {
        if std::env::var_os(HELPER_ENV).is_some() {
            return;
        }
        let dir = tempfile::tempdir().expect("dir");
        let pid_file = dir.path().join("pid");
        let mut helper = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "gui::viewer::tests::detach_helper",
                "--nocapture",
            ])
            .env(HELPER_ENV, &pid_file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("helper");
        let mut stdout = helper.stdout.take().expect("stdout");
        let (done, read) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
            let _ = done.send(text);
        });
        let text = read.recv_timeout(std::time::Duration::from_secs(20));
        let _ = helper.wait();
        // The started program is the test's to end.
        let identity = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|pid| pid.trim().parse().ok())
            .and_then(|pid| devlanes_sys::identity(pid).ok().flatten());
        if let Some(identity) = identity {
            #[cfg(unix)]
            let _ = devlanes_sys::signal_verified(&identity, devlanes_sys::Signal::KILL);
            #[cfg(windows)]
            let _ = devlanes_sys::terminate_verified(&identity, 1);
        }
        let text = text.expect("end of file while the started program still runs");
        assert!(text.contains("started"), "{text}");
        assert!(identity.is_some(), "the started program was running");
    }
}
