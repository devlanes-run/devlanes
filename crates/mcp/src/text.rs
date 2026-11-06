//! What agents read beyond clap's help: the server instructions, the
//! `devlanes` tool, the guidance each curated tool adds to its command's
//! about line, and the global options whose CLI help does not fit a tool. A
//! test in `tools` turns every tool call and args array quoted here into a
//! command line and parses it.

/// The server `instructions`. Hosts may keep only the start, so the rules that
/// change what an agent does come in the first 512 characters.
pub const INSTRUCTIONS: &str = "\
devlanes runs a dev environment per Git worktree (a lane) with its own ports, URLs and env. \
Common commands have tools of their own; the devlanes tool runs the others.
- Run dev servers, watchers, workers and tunnels with exec: `exec {\"name\": \"web\", \"command\": \
[\"pnpm\", \"dev\"]}`. devlanes owns the process and gives it the lane's ports and env. Never use \
&, nohup or a background shell for them.
- start, restart, reload and wait block until ready or failed. At the timeout they return what is \
ready so far; call wait to keep waiting instead of sleeping or polling with curl.
- Read a unit with `logs {\"units\": [\"web\"], \"tail\": 100}`, the lane with status, a failure \
with `debug_why {\"unit\": \"web\"}`, CPU and memory with debug_top, setup problems with doctor.
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
- Read a unit with `logs {\"units\": [\"web\"], \"tail\": 100}`, the lane with status, a failure \
with `debug_why {\"unit\": \"web\"}`, CPU and memory with debug_top, setup problems with doctor.
- cwd picks the worktree; without it the call uses the project the agent runs in.";

/// Description of the `devlanes` tool, which runs any command line.
pub const DEVLANES: &str = "\
Run a devlanes command that has no tool of its own, such as infra status, remote ls, config get, \
debug config or lane new, or the --help of any command. Use a typed tool when there is one. \
Returns the command's agent output, with its JSON in structuredContent.";

/// Description of the `devlanes` tool under `--read-only`, where it only runs
/// commands that change nothing.
pub const DEVLANES_READ_ONLY: &str = "\
Run a devlanes command that changes nothing and has no tool of its own, such as infra status, \
remote ls, config get, run --list or lane clean --dry-run, or the --help of any command. Use a \
typed tool when there is one; commands that change something are refused.";

/// Description of the `args` argument of the `devlanes` tool.
pub const ARGS: &str = "The command line after \"devlanes\", one element per argument, such as \
[\"infra\", \"logs\", \"postgres\", \"--tail\", \"50\"] or [\"remote\", \"ls\"]";

/// Description of every tool's `cwd` argument.
pub const CWD: &str =
    "Worktree directory to run in, which picks the project and lane [default: the agent's project]";

/// What a curated tool's description adds to its command's about line, by
/// tool name: when to pick it over the other tools, and what an agent should
/// know about its result. Only what a terminal user would not need belongs
/// here; the rest goes in the command's help. A tool without an entry has the
/// about line alone.
pub const GUIDANCE: &[(&str, &str)] = &[
    (
        "status",
        "Call it first to see what runs and whether it is ready, and to find the URL that opens \
a service.",
    ),
    (
        "start",
        "To run a command devlanes.yaml does not define, use exec. It returns once \
every unit is ready or one failed, or at the timeout with what is ready so far; then call wait \
instead of sleeping or polling.",
    ),
    (
        "exec",
        "Use it for dev servers, watchers, workers and tunnels, never nohup or a background shell: \
the process gets the lane's ports and env, and logs, status and stop find it by name.",
    ),
    (
        "logs",
        "For why a unit failed or is not ready, call debug_why first; logs reads more of what a \
unit printed.",
    ),
    (
        "stop",
        "Call it with mine to stop what you started once you are done. Leave the lane running \
when you hand work to the user for review.",
    ),
    (
        "restart",
        "Call it when a service has to pick up code or env it reads only at startup; after editing \
devlanes.yaml or .env, reload restarts only what changed. It returns once the units are ready or \
one failed, or at the timeout with what is ready so far; then call wait.",
    ),
    (
        "reload",
        "Call it after editing devlanes.yaml or .env instead of restarting the lane. It returns \
once the units are ready or one failed, or at the timeout with what is ready so far; then call \
wait.",
    ),
    (
        "wait",
        "Call it when start, restart or reload returned at their timeout, or for units another \
client is starting; a failure comes back with its reason and log tail.",
    ),
    (
        "with",
        "Use it for tests and other commands that exit and need the lane running; the command gets \
the lane's env and service URLs, and the result has its exit code and output. Run commands that \
do not need the lane in your own shell.",
    ),
    (
        "run",
        "Use it for project tasks such as migrations or seeds; the server asks the user first when \
the task sets confirm.",
    ),
    (
        "debug_why",
        "Call it first when a unit failed, exited or stays starting, before reading its logs.",
    ),
    (
        "debug_errors",
        "Use it to find what broke when you do not know which unit; debug_why explains one unit.",
    ),
    (
        "debug_top",
        "It answers any question about which process, service or container uses the most CPU or \
memory.",
    ),
    (
        "url_print",
        "Use it when you know the service; status shows the URLs of every service in the lane.",
    ),
    (
        "url_ls",
        "It tells which lane and service answer a hostname; url_print gives one service's URL.",
    ),
    (
        "env_show",
        "Use it to check the variables a unit gets, including those devlanes adds that no .env \
file shows.",
    ),
    (
        "lane_ls",
        "Use it to see which lanes exist, such as before cleaning; status shows what runs in them.",
    ),
    (
        "lane_clean",
        "The server shows the user a dry run and asks before deleting anything. To choose lanes to \
delete, list them with lane_ls, then call this with dry_run on each to see what it frees.",
    ),
    (
        "daemon_pair",
        "Call it on the box the user wants to reach from another machine; the result has the \
command to run there. The server asks the user first.",
    ),
    (
        "doctor",
        "Call it when hostnames do not resolve, HTTPS is not trusted or Docker fails, before \
debugging a single unit.",
    ),
];

/// The guidance of the tool `name`, from [`GUIDANCE`].
pub fn guidance(name: &str) -> Option<&'static str> {
    GUIDANCE
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, guidance)| *guidance)
}

/// The descriptions of global options whose CLI help does not fit a tool
/// call, by argument id.
pub fn global_help(id: &str) -> Option<&'static str> {
    Some(match id {
        "timeout" => {
            "How long to wait or stream, such as 30s or 5m [default for commands that wait: a \
             little under the host's tool call limit]"
        }
        "env" => "Variables for the processes the command starts, as KEY=VALUE such as DEBUG=1",
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
