//! What agents read beyond clap's help: the server instructions, the
//! `devlanes` tool and the descriptions the curated tools override. A test in
//! `tools` turns every tool call and args array quoted here into a command
//! line and parses it.

/// The server `instructions`. Hosts may keep only the start, so the rules that
/// change what an agent does come in the first 512 characters.
pub const INSTRUCTIONS: &str = "\
devlanes runs a dev environment per Git worktree (a lane) with its own ports, URLs and env. \
Common commands have tools of their own; the devlanes tool runs the others.
- Run dev servers, watchers, workers and tunnels with exec: `exec {\"name\": \"web\", \"command\": \
[\"pnpm\", \"dev\"]}`. devlanes owns the process and gives it the lane's ports and env. Never use \
&, nohup or a background shell for them.
- start, restart and wait block until ready or failed: no sleep or curl polling.

More:
- Read a unit with `logs {\"units\": [\"web\"], \"tail\": 100}`, the lane with status, a failure \
with `debug_why {\"unit\": \"web\"}`, CPU and memory with debug_top.
- Run builds, tests, git and other commands that exit in your own shell, or with \
`with {\"command\": [\"pnpm\", \"test\"]}` when they need the lane running.
- Stop what you started with `stop {\"mine\": true}`. Leave the lane running when you hand work \
to the user for review.
- lane_clean, daemon_pair and other commands that delete data or reach other machines ask the \
user first.
- cwd picks the worktree; without it the call uses the project the agent runs in.";

/// The server `instructions` under `--read-only`, where only the tools that
/// change nothing exist.
pub const READ_ONLY_INSTRUCTIONS: &str = "\
devlanes runs a dev environment per Git worktree (a lane) with its own ports, URLs and env. This \
server only reads: ask the user to start, stop or change anything. Common commands have tools of \
their own; the devlanes tool runs the others that change nothing.
- wait blocks until ready or failed: no sleep or curl polling.

More:
- Read a unit with `logs {\"units\": [\"web\"], \"tail\": 100}`, the lane with status, a failure \
with `debug_why {\"unit\": \"web\"}`, CPU and memory with debug_top.
- cwd picks the worktree; without it the call uses the project the agent runs in.";

/// Description of the `devlanes` tool, which runs any command line.
pub const DEVLANES: &str = "\
Run a devlanes command that has no tool of its own, such as debug config, infra status, remote \
ls, config get or lane new, or any command with --help. args is the command line after \
\"devlanes\", such as [\"infra\", \"status\"] or [\"debug\", \"config\"]. Use the other tools first: \
start, exec, logs, stop and the rest take typed arguments. Returns the command's agent output, \
with its JSON in structuredContent.";

/// Description of the `devlanes` tool under `--read-only`, where it only runs
/// commands that change nothing.
pub const DEVLANES_READ_ONLY: &str = "\
Run a devlanes command that changes nothing and has no tool of its own, such as debug config, \
infra status, remote ls, config get, run --list or lane clean --dry-run, or any command with \
--help. args is the command line after \"devlanes\", such as [\"infra\", \"status\"] or \
[\"lane\", \"clean\", \"2\", \"--dry-run\"]. Use the other tools first.";

/// Description of the `args` argument of the `devlanes` tool.
pub const ARGS: &str = "The devlanes command line without \"devlanes\", one element per argument.";

/// Description of every tool's `cwd` argument.
pub const CWD: &str = "Worktree directory to run in, which picks its lane; relative to the \
server's directory.";

/// The descriptions of global options whose CLI help does not fit a tool
/// call, by argument id.
pub fn global_help(id: &str) -> Option<&'static str> {
    Some(match id {
        "timeout" => {
            "How long to wait, such as 30s or 5m [default for commands that wait: a little under \
             the host's tool call limit]"
        }
        "env" => "Variables for the processes the command starts, as KEY=VALUE",
        "yes" => {
            "Skip the server's question to the user; set it only when the user already agreed in \
             the chat"
        }
        _ => return None,
    })
}

/// Description of the app-only `app_snapshot` tool.
pub const APP_SNAPSHOT: &str = "Lanes and units for the lane card, with metrics when asked, \
the lanes of linked boxes when no lane is asked for, and the number of each lane's newest card.";

/// Description of the app-only `app_logs` tool.
pub const APP_LOGS: &str = "Log lines of units for the card's log viewer.";

/// Description of the app-only `app_action` tool.
pub const APP_ACTION: &str = "A button of the card: start, stop, restart, reload, clean, \
clean_plan or pair, on a lane or unit. Only the user's click calls it, and the click is the \
consent: clean and pair run without asking again.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The exec rule and the blocking rule fit in the first 512 characters.
    #[test]
    fn key_rules_come_first() {
        let start = &INSTRUCTIONS[..512.min(INSTRUCTIONS.len())];
        for rule in ["with exec", "\"command\"", "nohup", "block until ready"] {
            assert!(start.contains(rule), "{rule} is past 512 characters");
        }
        let start = &READ_ONLY_INSTRUCTIONS[..512.min(READ_ONLY_INSTRUCTIONS.len())];
        for rule in ["only reads", "blocks until ready"] {
            assert!(start.contains(rule), "{rule} is past 512 characters");
        }
    }
}
