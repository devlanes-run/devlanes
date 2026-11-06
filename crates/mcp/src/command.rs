//! The command line a tool call names: parsed with the CLI's own parser,
//! checked against the tool, and completed with the flags a call needs.

use std::time::Duration;

use devlanes_cli::{
    Cli, Command, ConfigCommand, DaemonCommand, LaneCommand, RemoteCommand, clap::Parser,
    clap::error::ErrorKind,
};

/// A command line ready to run.
pub struct Call {
    /// What clap parsed.
    pub cli: Cli,
    /// The subcommand path, such as `lane clean`; `status` for a bare `devlanes`.
    pub path: String,
    /// The arguments to run, without the program name.
    pub args: Vec<String>,
}

/// Why a command line does not run.
pub enum Rejected {
    /// `--help` or `--version`: the text clap prints, not an error.
    Help(String),
    /// Anything else, with the text to show.
    Error(String),
}

impl Call {
    /// Parses `args` as `devlanes ARGS`. `--output` and `--json` are dropped
    /// first, since every call runs with `--output agent`.
    pub fn parse(args: &[String], read_only_tool: bool) -> Result<Self, Rejected> {
        let args = without_output_flags(args);
        let cli = Cli::try_parse_from(std::iter::once("devlanes".to_owned()).chain(args.clone()))
            .map_err(|error| match error.kind() {
            ErrorKind::DisplayHelp
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            | ErrorKind::DisplayVersion => Rejected::Help(error.render().to_string()),
            _ => Rejected::Error(devlanes_cli::usage_error(
                &error.render().to_string(),
                &args,
            )),
        })?;
        let path = devlanes_cli::command_path(&args);
        let path = if path.is_empty() {
            "status".to_owned()
        } else {
            path.join(" ")
        };
        if read_only_tool && !cli.command.as_ref().is_none_or(Command::is_read_only) {
            return Err(Rejected::Error(format!(
                "devlanes {path} changes something, and devlanes_read only runs commands that \
                 change nothing; call the devlanes tool with the same args instead"
            )));
        }
        if let Some(reason) = cli.terminal_only() {
            return Err(Rejected::Error(format!(
                "devlanes {path} does not run here: {reason}"
            )));
        }
        Ok(Self { cli, path, args })
    }

    /// Whether the command blocks until units are ready (`start`, `restart`,
    /// `reload`, `wait`, `with`, `lane resume`).
    pub fn blocks(&self) -> bool {
        matches!(
            &self.cli.command,
            Some(
                Command::Start(_)
                    | Command::Restart(_)
                    | Command::Reload(_)
                    | Command::Wait { .. }
                    | Command::With(_)
                    | Command::Lane {
                        command: LaneCommand::Resume { .. }
                    }
            )
        )
    }

    /// Whether `lane.phase` events of this command are worth reporting as progress.
    pub fn starts(&self) -> bool {
        self.blocks()
            && !matches!(
                &self.cli.command,
                Some(Command::Wait { .. } | Command::With(_))
            )
    }

    /// Adds `--timeout` to a blocking command that has none, so it returns
    /// before the host gives up on the call. Returns the timeout in effect.
    pub fn limit(&mut self, default: Duration) -> Option<Duration> {
        if !self.blocks() {
            return self.cli.timeout;
        }
        if self.cli.timeout.is_none() {
            self.add(&["--timeout", &format!("{}s", default.as_secs())]);
            self.cli.timeout = Some(default);
        }
        self.cli.timeout
    }

    /// Adds `--lane REF` unless the command line names a lane already.
    pub fn default_lane(&mut self, lane: &str) {
        if self.cli.lane.is_none() {
            self.add(&["--lane", lane]);
            self.cli.lane = Some(lane.to_owned());
        }
    }

    /// Adds `--yes` (or the flag a refusal named) unless it is there.
    pub fn confirm(&mut self, flag: &str) {
        if flag == "--yes" {
            if self.cli.yes {
                return;
            }
            self.cli.yes = true;
        }
        let end = self.end();
        if !self.args[..end].iter().any(|arg| arg == flag) {
            self.add(&[flag]);
        }
    }

    /// Flags go before a `--`, after which they would reach the wrapped command.
    fn add(&mut self, flags: &[&str]) {
        let end = self.end();
        self.args
            .splice(end..end, flags.iter().map(|flag| (*flag).to_owned()));
    }

    /// The index of the first `--`, or the length.
    fn end(&self) -> usize {
        self.args
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or(self.args.len())
    }

    /// The question to ask the user before running this command, or `None` when
    /// it needs no consent or already has `--yes`.
    pub fn consent(&self) -> Option<String> {
        if self.cli.yes {
            return None;
        }
        let question = match self.cli.command.as_ref()? {
            Command::Lane {
                command:
                    LaneCommand::Clean {
                        lane,
                        dry_run: false,
                    },
            } => format!(
                "Clean lane {}? This stops it, runs its clean tasks and teardown, and deletes its \
                 data and logs.",
                lane.as_deref()
                    .or(self.cli.lane.as_deref())
                    .unwrap_or("of this worktree")
            ),
            Command::Lane {
                command: LaneCommand::Gc,
            } => "Clean every lane whose worktree or project file is gone, with its data?".into(),
            Command::Daemon { command } => match command {
                DaemonCommand::Pair => {
                    "Print a one-time code that lets another machine pair with this one?".into()
                }
                DaemonCommand::Stop => "Stop the daemon and every process it manages?".into(),
                DaemonCommand::Restart => {
                    "Restart the daemon? Running services stop and start again.".into()
                }
                DaemonCommand::ResetState => "Stop everything and empty the state database? \
                     Lanes get new numbers and ports."
                    .into(),
                DaemonCommand::Revoke { client } => {
                    format!("Disconnect {client} and delete its pairing?")
                }
                _ => return None,
            },
            Command::Remote { command } => match command {
                RemoteCommand::Connect { host, .. } => {
                    format!("Connect to {host} and mirror its lanes here?")
                }
                RemoteCommand::Disconnect { host } => match host {
                    Some(host) => format!("Disconnect from {host}?"),
                    None => "Disconnect from every box?".into(),
                },
                _ => return None,
            },
            Command::Config {
                command: ConfigCommand::Set { key, value },
            } if key.starts_with("remote.") => {
                format!("Set {key} to {value} in the user config?")
            }
            Command::Config {
                command: ConfigCommand::Unset { key },
            } if key.starts_with("remote.") => {
                format!("Remove {key} from the user config?")
            }
            _ => return None,
        };
        Some(question)
    }

    /// The `lane clean --dry-run` arguments for this `lane clean`, whose agent
    /// output shows the user what a clean removes.
    pub fn clean_plan(&self) -> Option<Vec<String>> {
        let Some(Command::Lane {
            command: LaneCommand::Clean { dry_run: false, .. },
        }) = &self.cli.command
        else {
            return None;
        };
        let mut plan = self.args.clone();
        plan.push("--dry-run".into());
        Some(plan)
    }
}

/// `args` without `--output MODE`, `--output=MODE` and `--json` before a `--`.
fn without_output_flags(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {
                kept.push(arg.clone());
                kept.extend(args.cloned());
                break;
            }
            "--json" => {}
            "--output" => {
                args.next();
            }
            _ if arg.starts_with("--output=") => {}
            _ => kept.push(arg.clone()),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `args` as owned strings.
    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    /// Parses or panics with the rejection.
    fn call(line: &[&str]) -> Call {
        match Call::parse(&args(line), false) {
            Ok(call) => call,
            Err(Rejected::Help(text) | Rejected::Error(text)) => panic!("{line:?}: {text}"),
        }
    }

    /// The output mode is the server's; flags after `--` belong to the command.
    #[test]
    fn output_flags_are_dropped() {
        assert_eq!(
            call(&["--json", "status", "--output", "plain"]).args,
            args(&["status"])
        );
        assert_eq!(
            call(&["exec", "--name", "x", "--", "tool", "--json"]).args,
            args(&["exec", "--name", "x", "--", "tool", "--json"])
        );
    }

    /// Blocking commands get the host's timeout before a `--`, unless they set one.
    #[test]
    fn blocking_commands_get_a_timeout() {
        let mut start = call(&["start", "api"]);
        assert_eq!(
            start.limit(Duration::from_secs(25)),
            Some(Duration::from_secs(25))
        );
        assert_eq!(start.args, args(&["start", "api", "--timeout", "25s"]));
        let mut with = call(&["with", "--", "pnpm", "test"]);
        with.limit(Duration::from_secs(50));
        assert_eq!(
            with.args,
            args(&["with", "--timeout", "50s", "--", "pnpm", "test"])
        );
        let mut own = call(&["wait", "api", "--timeout", "5s"]);
        assert_eq!(
            own.limit(Duration::from_secs(25)),
            Some(Duration::from_secs(5))
        );
        assert_eq!(own.args, args(&["wait", "api", "--timeout", "5s"]));
        let mut status = call(&["status"]);
        assert_eq!(status.limit(Duration::from_secs(25)), None);
        assert_eq!(status.args, args(&["status"]));
    }

    /// Terminal-only commands and write commands on the read tool are refused
    /// with what to do instead; `--help` is text, not an error.
    #[test]
    fn refusals() {
        let refused = |line: &[&str], read_only| match Call::parse(&args(line), read_only) {
            Err(Rejected::Error(text)) => text,
            _ => panic!("{line:?} was not refused"),
        };
        assert!(refused(&["ui"], false).contains("TUI"));
        assert!(refused(&["logs", "api", "-f"], false).contains("--timeout"));
        assert!(refused(&["env", "shell"], false).contains("env show"));
        assert!(refused(&["stop"], true).contains("devlanes tool"));
        assert!(refused(&["exec", "--name", "x", "--", "y"], true).contains("devlanes tool"));
        assert!(refused(&["nope"], false).contains("nope"));
        assert!(Call::parse(&args(&["status"]), true).is_ok());
        assert!(Call::parse(&args(&[]), true).is_ok_and(|call| call.path == "status"));
        assert!(matches!(
            Call::parse(&args(&["logs", "--help"]), true),
            Err(Rejected::Help(text)) if text.contains("--cursor")
        ));
    }

    /// The consent list asks; `--yes` from the agent means it asked already.
    #[test]
    fn consent_list() {
        for line in [
            &["lane", "clean", "2"][..],
            &["lane", "gc"],
            &["daemon", "pair"],
            &["daemon", "stop"],
            &["daemon", "restart"],
            &["daemon", "reset-state"],
            &["daemon", "revoke", "laptop"],
            &["remote", "connect", "box"],
            &["remote", "disconnect"],
            &["config", "set", "remote.require_pairing", "false"],
            &["config", "unset", "remote.allow"],
        ] {
            assert!(call(line).consent().is_some(), "{line:?}");
        }
        for line in [
            &["lane", "clean", "2", "--yes"][..],
            &["lane", "clean", "2", "--dry-run"],
            &["config", "set", "tui.theme", "dark"],
            &["stop"],
            &["daemon", "status"],
        ] {
            assert!(call(line).consent().is_none(), "{line:?}");
        }
        let mut clean = call(&["lane", "clean", "2"]);
        clean.confirm("--yes");
        clean.confirm("--yes");
        assert_eq!(clean.args, args(&["lane", "clean", "2", "--yes"]));
        assert!(clean.consent().is_none());
    }
}
