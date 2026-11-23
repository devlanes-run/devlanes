//! The native messaging host of the devlanes browser extension. Chrome starts
//! the devlanes binary with the extension's origin, and the host relays an
//! allowlist of requests to the daemon over stdin and stdout.

mod daemon;
pub mod frame;
pub mod message;
pub mod origin;
mod session;
pub mod view;

use std::{ffi::OsString, process::ExitCode};

pub use origin::is_origin;

/// Runs the host with `args`, the arguments after the program name. It
/// refuses an origin outside [`origin::ALLOWED_IDS`] with exit code 1, and
/// exits 0 when the extension closes the port. stdout carries frames only,
/// and diagnostics go to stderr.
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    if let Err(error) = origin::parse(args) {
        eprintln!("devlanes: {error}");
        return ExitCode::from(1);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("devlanes: the browser host couldn't start its runtime: {error}");
            return ExitCode::from(1);
        }
    };
    let result = runtime.block_on(session::serve(
        daemon::Local,
        tokio::io::stdin(),
        tokio::io::stdout(),
    ));
    // tokio reads stdin on a blocking thread that may still wait for input,
    // and a plain drop would wait for it.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devlanes: {error}");
            ExitCode::from(1)
        }
    }
}
