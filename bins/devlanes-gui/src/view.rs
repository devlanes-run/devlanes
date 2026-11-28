//! `devlanes-gui view`: the native viewer of one GUI service.
//!
//! `devlanes gui open` and the daemon's auto-open start it with the
//! service's address on this machine. With `--instance-dir` it is the one
//! viewer of the service here: a second one asks the first for its windows
//! and ends.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use devlanes_gui_client::instance::{self, Claim, Info, StartedBy};
use devlanes_gui_codec::{DecoderBackend, RawBackend};
use devlanes_gui_viewer::{Options, UnitCheck, ViewerError};

/// The arguments of `devlanes-gui view`.
#[derive(Debug, clap::Args)]
pub struct ViewArgs {
    /// The GUI host's address on this machine: the service's devlanes_gui
    /// port, or its mirror for a box's service, such as 127.0.0.1:26012.
    #[arg(long, value_name = "ADDRESS")]
    address: SocketAddr,
    /// The service, such as acme@2:desktop.
    #[arg(long, value_name = "LABEL")]
    service: String,
    /// The directory of the viewer locks, so a service has one viewer here.
    #[arg(long, value_name = "DIR")]
    instance_dir: Option<PathBuf>,
    /// Started by the daemon's auto-open: windows open without taking focus,
    /// and the daemon ends the viewer once the service stopped.
    #[arg(long)]
    auto: bool,
    /// The `devlanes` executable to ask whether the service still runs while
    /// the viewer cannot reconnect.
    #[arg(long, value_name = "PATH", requires = "unit")]
    devlanes: Option<PathBuf>,
    /// The unit to ask about, as lane:unit.
    #[arg(long, value_name = "UNIT", requires = "devlanes")]
    unit: Option<String>,
    /// The box the service's lane was found on, for a box's lane: while that
    /// box is not connected, neither the check nor the daemon counts the
    /// service as gone.
    #[arg(long = "box", value_name = "NAME")]
    host_box: Option<String>,
}

/// Why `view` did not run.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    /// The viewer failed.
    #[error(transparent)]
    Viewer(#[from] ViewerError),
    /// The lock directory could not be used.
    #[error("cannot use the viewer lock in {}", .0.display())]
    Instance(PathBuf, #[source] std::io::Error),
}

impl ViewError {
    /// Whether the machine has no display, which the exit code tells apart.
    pub fn no_display(&self) -> bool {
        matches!(self, Self::Viewer(ViewerError::NoDisplay))
    }
}

/// The decoders this machine has, best first: H.264 where the OS or OpenH264
/// has it, then `raw`. Looked for on a thread of its own, since Media
/// Foundation's probe uses the multithreaded COM apartment, which the winit
/// thread this runs on must never join.
fn decoders() -> Vec<Arc<dyn DecoderBackend>> {
    let found = std::thread::spawn(crate::codecs::find)
        .join()
        .map(|codecs| {
            for problem in &codecs.problems {
                tracing::warn!(codec = %problem.codec, reason = problem.reason.word(), detail = %problem.detail, "a video decoder is missing");
            }
            codecs.decoders
        });
    // A probe that panicked leaves `raw`, which every host has.
    found.unwrap_or_else(|_| vec![Arc::new(RawBackend)])
}

/// Runs the viewer on the main thread until the service is gone.
pub fn run(args: ViewArgs) -> Result<(), ViewError> {
    crate::host::init_logging();
    // Before the lock, so a machine without a display neither takes it nor
    // asks a running viewer for anything.
    if !devlanes_gui_client::display::here() {
        return Err(ViewerError::NoDisplay.into());
    }
    let instance = match &args.instance_dir {
        None => None,
        Some(dir) => {
            let key = instance::key(&args.service, args.address.port());
            let pid = std::process::id();
            let info = Info {
                pid,
                start_time: devlanes_sys::start_time(pid).ok().flatten().unwrap_or(0),
                started_by: if args.auto {
                    StartedBy::Daemon
                } else {
                    StartedBy::User
                },
                service: args.service.clone(),
                host_box: args.host_box.clone(),
            };
            let failed = |error| ViewError::Instance(dir.clone(), error);
            match instance::claim(dir, &key, &info).map_err(failed)? {
                Claim::Claimed(instance) => Some(instance),
                Claim::Held => {
                    if !args.auto {
                        instance::ask_to_show(dir, &key).map_err(failed)?;
                    }
                    tracing::info!(service = %args.service, "a viewer of this service runs already");
                    return Ok(());
                }
            }
        }
    };
    let host_box = args.host_box;
    let check = args
        .devlanes
        .zip(args.unit)
        .map(|(devlanes, unit)| UnitCheck {
            devlanes,
            unit,
            host_box,
        });
    devlanes_gui_viewer::run(Options {
        address: args.address,
        service: args.service,
        decoders: decoders(),
        focus: !args.auto,
        check,
        instance,
    })?;
    Ok(())
}
