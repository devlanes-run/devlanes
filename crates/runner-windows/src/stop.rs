//! What one stop sends, chosen once from the unit's processes.

use devlanes_core::{StopSignal, WindowsStop};
use devlanes_sys_windows::{
    console::CtrlEvent,
    proc::{Held, ProcessEntry},
};

use crate::{runtime::Runtimes, spawn::is_console_host};

/// What one stop sends, chosen once from one read of the unit's processes.
#[derive(Debug)]
pub(crate) struct StopPlan {
    pub ask: Ask,
    /// The live processes, console host left out, read once for this stop;
    /// `WM_CLOSE` and the console event go to these.
    pub targets: Vec<ProcessEntry>,
}

/// How a stop asks the unit to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ask {
    /// SIGKILL: terminate the job.
    Terminate,
    /// Ctrl-C typed into the pseudo console.
    Key,
    /// One console event through the helper.
    Event(CtrlEvent),
}

impl StopPlan {
    /// The plan for `signal` to a unit whose processes are `processes`:
    ///
    /// | `stop.signal` | `pty: true` | `pty: false` | `pty: false`, an MSYS program runs |
    /// | --- | --- | --- | --- |
    /// | SIGINT, SIGTERM | Ctrl-C key | Ctrl-C event | Ctrl-Break event |
    /// | SIGHUP, SIGQUIT, SIGUSR1, SIGUSR2 | Ctrl-Break event | Ctrl-Break event | Ctrl-Break event |
    /// | SIGKILL | terminate | terminate | terminate |
    ///
    /// The key reaches the programs an MSYS shell starts as Ctrl-C, and a
    /// program that reads its console raw, such as `wrangler dev`, as the
    /// `0x03` its hotkey handler cleans up on. Without a terminal, MSYS starts
    /// native programs so that a Ctrl-C event does not reach them, so
    /// Ctrl-Break goes out instead. A target is an MSYS program when its
    /// image links the MSYS or Cygwin runtime (see [`Runtimes`]); a target
    /// whose image cannot be read counts as native. Images are read only for
    /// SIGINT and SIGTERM without a terminal. Blocking.
    pub(crate) fn new(
        signal: StopSignal,
        pty: bool,
        processes: Vec<ProcessEntry>,
        runtimes: &Runtimes,
    ) -> Self {
        let targets: Vec<ProcessEntry> = processes
            .into_iter()
            .filter(|p| !p.exited && !is_console_host(&p.name))
            .collect();
        let ask = match signal.windows_stop() {
            WindowsStop::Terminate => Ask::Terminate,
            WindowsStop::CtrlBreak => Ask::Event(CtrlEvent::CtrlBreak),
            WindowsStop::CtrlC if pty => Ask::Key,
            WindowsStop::CtrlC if targets.iter().any(|p| runs_msys(p, runtimes)) => {
                Ask::Event(CtrlEvent::CtrlBreak)
            }
            WindowsStop::CtrlC => Ask::Event(CtrlEvent::CtrlC),
        };
        tracing::debug!(?signal, pty, ?ask, targets = targets.len(), "stop planned");
        Self { ask, targets }
    }
}

/// Whether `process` runs a program that links the MSYS or Cygwin runtime;
/// `false` when it is gone or its image cannot be read. Blocking.
fn runs_msys(process: &ProcessEntry, runtimes: &Runtimes) -> bool {
    let image = match Held::open(process.pid, process.start_time) {
        Ok(Some(held)) => held.image_path(),
        Ok(None) => return false,
        Err(error) => Err(error),
    };
    match image.and_then(|image| runtimes.links_msys(&image)) {
        Ok(links) => links,
        Err(error) => {
            tracing::debug!(%error, pid = process.pid, name = %process.name, "image not read; counted as native");
            false
        }
    }
}
