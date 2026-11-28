//! Starting `devlanes-gui view`, the viewer of one GUI service on this
//! machine, for `devlanes gui open` and the daemon's auto-open alike.
//!
//! The viewer is a client process, not a managed unit: it is no member of a
//! unit's run, its activity is not the lane's, and it outlives the command
//! that started it. It is started on its own (its own session on Unix; no
//! console, its own process group and outside the terminal's job where that
//! job allows it on Windows), so closing the terminal or a Ctrl-C there does
//! not end it. Its log goes to a file of the devlanes home.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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
    /// runs; `None` for an auto-opened viewer, which the daemon ends.
    pub devlanes: Option<PathBuf>,
    /// Where the viewer's log goes; the file is replaced.
    pub log: PathBuf,
    /// Variables to set for the viewer on top of the starter's environment.
    pub env: Vec<(String, String)>,
}

/// The directory of the viewer locks in the devlanes home `home`.
pub fn instance_dir(home: &Path) -> PathBuf {
    home.join("run").join("gui-viewers")
}

/// The log file of the viewer of `key` in the devlanes home `home`.
pub fn log_path(home: &Path, key: &str) -> PathBuf {
    home.join("logs").join(format!("gui-viewer-{key}.log"))
}

/// Starts the viewer. Blocking: it creates the log file and spawns.
pub fn spawn(launch: &Launch) -> std::io::Result<Child> {
    if let Some(parent) = launch.log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = std::fs::File::create(&launch.log)?;
    let mut command = Command::new(&launch.program);
    command
        .arg("view")
        .arg("--address")
        .arg(launch.address.to_string())
        .arg("--service")
        .arg(&launch.service)
        .arg("--instance-dir")
        .arg(&launch.instance_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .envs(launch.env.iter().map(|(name, value)| (name, value)));
    if launch.auto {
        command.arg("--auto");
    }
    if let Some(devlanes) = &launch.devlanes {
        command
            .arg("--devlanes")
            .arg(devlanes)
            .arg("--unit")
            .arg(&launch.service);
    }
    // The viewer's own names for the desktop are the daemon's to hand on.
    for (_, kept) in devlanes_proto::VIEWER_DISPLAY_ENV {
        command.env_remove(kept);
    }
    detach(command)
}

/// Spawns `command` in a session of its own.
#[cfg(unix)]
fn detach(mut command: Command) -> std::io::Result<Child> {
    devlanes_sys::new_session(&mut command);
    command.spawn()
}

/// Spawns `command` without a console, in a process group of its own and,
/// when the job allows it, outside this process's job, which some terminals
/// end when they close.
#[cfg(windows)]
fn detach(mut command: Command) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    command.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            command.creation_flags(flags);
            command.spawn()
        }
        spawned => spawned,
    }
}

/// The desktop variables for a viewer the daemon starts on Linux: the ones
/// the CLI handed to it under the viewer's names, else the daemon's own (a
/// daemon a desktop session's service manager started has them). `None`
/// when there is no display to open windows on.
#[cfg(target_os = "linux")]
pub fn display_env() -> Option<Vec<(String, String)>> {
    let handed: Vec<(String, String)> = devlanes_proto::VIEWER_DISPLAY_ENV
        .iter()
        .filter_map(|(name, kept)| {
            Some((
                (*name).to_owned(),
                std::env::var(kept).ok().filter(|v| !v.is_empty())?,
            ))
        })
        .collect();
    let has = |vars: &[(String, String)]| {
        vars.iter()
            .any(|(name, _)| name == "DISPLAY" || name == "WAYLAND_DISPLAY")
    };
    if has(&handed) {
        return Some(handed);
    }
    let own: Vec<(String, String)> = devlanes_proto::VIEWER_DISPLAY_ENV
        .iter()
        .filter_map(|(name, _)| {
            Some((
                (*name).to_owned(),
                std::env::var(name).ok().filter(|v| !v.is_empty())?,
            ))
        })
        .collect();
    has(&own).then_some(own)
}

/// macOS and Windows viewers open on the session's desktop.
#[cfg(not(target_os = "linux"))]
pub fn display_env() -> Option<Vec<(String, String)>> {
    Some(Vec::new())
}
