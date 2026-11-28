//! `devlanes-gui`: the second devlanes binary, which holds the GUI hosts and,
//! later, the viewer. It ships next to `devlanes`, which finds it in its own
//! directory, so `devlanes` itself never links the GUI libraries.

mod host;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Exit code for a failure after the arguments were read.
const FAILED: u8 = 1;

/// The command line.
#[derive(Debug, Parser)]
#[command(
    name = "devlanes-gui",
    about = "The devlanes GUI host; devlanes starts it, people do not",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print `devlanes-gui <version> build=<release|debug>`.
    Version,
    /// Run a GUI host: start the backend, listen on loopback, print the ready
    /// line and serve until stopped.
    Host(host::HostArgs),
}

/// Parses the command line and runs it. Logs go to standard error; only the
/// `version` output and the host's ready line use standard output.
fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        Command::Host(args) => match host::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devlanes-gui: {}", chain(&error));
                ExitCode::from(FAILED)
            }
        },
    }
}

/// What `version` prints. Installers and the updater read the first two
/// words; `build=` tells a release build from a debug one, as
/// `devlanes-hosts version` does.
fn version_line() -> String {
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    format!("devlanes-gui {} build={build}", devlanes_version::VERSION)
}

/// `error` and its sources, joined with `: `.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
