//! What agents read: the server instructions and the tool descriptions. Every
//! `devlanes ...` command quoted here is parsed by a test.

/// The server `instructions`. Hosts may keep only the start, so the rules that
/// change what an agent does come in the first 512 characters.
pub const INSTRUCTIONS: &str = "\
devlanes runs a dev environment per git worktree (a lane) with its own ports, URLs and env. \
Call the devlanes tool with the command line after \"devlanes\" as args.
- Run dev servers, watchers, workers and tunnels with exec, such as devlanes exec --name web -- pnpm dev. \
devlanes owns the process and gives it the lane's ports and env. Never use &, nohup or a background shell for them.
- start, restart and wait block until ready or failed: no sleep or curl polling.

More:
- Read a unit with devlanes logs web --tail 100, the lane with devlanes status, a failure with devlanes debug why web.
- Run builds, tests, git and other commands that exit yourself, or with devlanes with -- pnpm test when they need the lane running.
- Stop what you started with devlanes stop --mine. Leave the lane running when you hand work to the user for review.
- Commands that delete data or reach other machines ask the user first.
- cwd picks the worktree; without it the call uses the project the agent runs in.";

/// Description of the `devlanes` tool.
pub const DEVLANES: &str = "\
Run any devlanes command. args is the command line after \"devlanes\", such as [\"start\"], \
[\"exec\", \"--name\", \"web\", \"--\", \"pnpm\", \"dev\"], [\"stop\", \"--mine\"] or \
[\"logs\", \"web\", \"--tail\", \"100\"]. Use exec for anything that keeps running \
(dev servers, watchers, workers, tunnels); its result says how to read and stop it. start, \
restart and wait return once the units are ready or failed. Returns the command's agent output, \
with its JSON in structuredContent.";

/// Description of the `devlanes_read` tool.
pub const DEVLANES_READ: &str = "\
Run a devlanes command that changes nothing: status, logs, wait, debug why, debug errors, url, \
env show, doctor, lane ls, remote ls, debug config, debug top, or --help. args is the command \
line after \"devlanes\", such as [\"status\"] or [\"logs\", \"web\", \"--tail\", \"100\"]. Use \
the devlanes tool for commands that start, stop or change something.";

/// Description of the `args` argument of both tools.
pub const ARGS: &str = "The devlanes command line without \"devlanes\", one element per argument.";

/// Description of the `cwd` argument of both tools.
pub const CWD: &str = "Directory to run in, which picks the worktree and its lane. \
Relative paths start from the server's directory.";

/// Description of the app-only `app_snapshot` tool.
pub const APP_SNAPSHOT: &str = "Lanes and units for the lane card, with metrics when asked.";

/// Description of the app-only `app_logs` tool.
pub const APP_LOGS: &str = "Log lines of units for the card's log viewer.";

/// Description of the app-only `app_action` tool.
pub const APP_ACTION: &str = "A button of the card: start, stop, restart, reload, clean, \
clean_plan or pair, on a lane or unit.";
