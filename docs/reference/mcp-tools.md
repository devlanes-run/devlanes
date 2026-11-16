---
title: MCP tools
description: The tools, results, timeouts and consent questions of the devlanes MCP server.
kind: reference
---

`devlanes mcp` serves devlanes to a coding agent over MCP on standard input and output. [Set up a coding agent](../how-to/agents/set-up.md) registers it with each agent host.

## Server options

| Option | Effect |
|---|---|
| `--read-only` | Offers only the tools of commands that change nothing. The `devlanes` tool refuses the other commands. |
| `--path DIR` | Calls that name no `cwd` run in DIR. |
| `--lane LANE` | The global option. Calls that name no `cwd` and no lane get `--lane LANE`. Ignored when `--path` is set. |

A call runs in the first of these directories:

| Step | Directory |
|---|---|
| 1 | The call's `cwd`, relative to the directory the server started in. |
| 2 | `--path`. |
| 3 | `CLAUDE_PROJECT_DIR`, when the host sets it. |
| 4 | The directory the agent host started the server in. |

The server connects to no daemon itself. Each call runs the devlanes binary as a separate process with `--output agent`, in the call's directory, and that process starts the daemon as the CLI would. The server ends when the host closes its standard input, or on SIGTERM, SIGINT or SIGHUP; on Windows, when its console closes or the system shuts down.

## Instructions

When a host connects, the server sends instructions that hosts put in the agent's context. They say that a lane is a dev environment per worktree, that the common commands have tools and the `devlanes` tool runs the others, that dev servers, watchers, workers and tunnels run with `exec` and never with `&`, `nohup` or a background shell, that `start`, `restart`, `reload` and `wait` block and return what is ready at their timeout so `wait` continues, which tools read logs, status, failures, resource use and setup problems, that builds and tests run in the agent's own shell or with `with`, that `stop` with `mine` stops what the agent started, that `lane_clean`, `daemon_pair` and commands that delete data or reach other machines ask the user, and that `cwd` picks the worktree. With `--read-only`, the server sends instructions for the read-only tool set.

## Tools

Each tool runs the command it is named after, with `_` between the words of the command: `debug_why` runs `devlanes debug why`. Its arguments are the command's arguments and options, with `_` in place of `-`. Each command's page, such as [`devlanes logs`](cli/logs.md), describes them.

Every tool takes `cwd`. The global options `lane`, `timeout`, `env` and `yes` appear on the tools whose commands use them.

| Argument | Type | Command line |
|---|---|---|
| `cwd` | string | The directory the command runs in, which picks the project and the lane. |
| `lane` | string | `--lane`. |
| `timeout` | string, such as `30s` or `5m` | `--timeout`. |
| `env` | array of `KEY=VALUE` strings | `--env`, once per item. |
| `yes` | boolean | `--yes`, which skips the question to the user. |

A boolean argument set to `true` adds its flag, and `false` leaves it out. An array argument also takes a single string. A positional value that starts with `-` is refused.

| Tool | Command | Read-only | Arguments |
|---|---|---|---|
| `status` | `status` | yes | `lane`, `mine`, `cwd` |
| `start` | `start` | no | `units`, `profile`, `all_at_once`, `lane`, `timeout`, `env`, `cwd` |
| `exec` | `exec` | no | `name` (required), `no_pty`, `command` (required), `lane`, `env`, `cwd` |
| `logs` | `logs` | yes | `units`, `all`, `tail`, `head`, `follow`, `since`, `grep`, `level`, `around`, `context`, `cursor`, `mine`, `lane`, `timeout`, `cwd` |
| `stop` | `stop` | no | `units`, `mine`, `logs_dir`, `lane`, `cwd` |
| `restart` | `restart` | no | `units`, `all_at_once`, `lane`, `timeout`, `env`, `cwd` |
| `reload` | `reload` | no | `units`, `all_at_once`, `lane`, `timeout`, `cwd` |
| `wait` | `wait` | yes | `target`, `lane`, `timeout`, `cwd` |
| `with` | `with` | no | `units`, `profile`, `logs_dir`, `keep`, `all_at_once`, `command` (required), `lane`, `timeout`, `cwd` |
| `run` | `run` | no | `task`, `list`, `detach`, `lane`, `env`, `yes`, `cwd` |
| `debug_why` | `debug why` | yes | `unit`, `lane`, `cwd` |
| `debug_errors` | `debug errors` | yes | `since`, `lane`, `cwd` |
| `debug_top` | `debug top` | yes | `cwd` |
| `url_print` | `url print` | yes | `unit`, `lane`, `cwd` |
| `url_ls` | `url ls` | yes | `cwd` |
| `env_show` | `env show` | yes | `unit`, `explain`, `lane`, `cwd` |
| `lane_ls` | `lane ls` | yes | `cwd` |
| `lane_clean` | `lane clean` | no | `lane`, `dry_run`, `yes`, `cwd` |
| `daemon_pair` | `daemon pair` | no | `yes`, `cwd` |
| `doctor` | `doctor` | yes | `cwd` |
| `devlanes` | any other command | no, or yes with `--read-only` | `args` (required), `cwd` |

### Arguments of each tool

| Argument | Tools | Type | Command line |
|---|---|---|---|
| `units` | `start`, `logs`, `stop`, `restart`, `reload`, `with` | array | The unit references, positional. |
| `unit` | `debug_why`, `url_print`, `env_show` | string | The unit reference, positional. |
| `target` | `wait` | string | The unit or lane reference, positional. |
| `task` | `run` | string | The task name, or `infra.setup.NAME` or `infra.teardown.NAME`, positional. |
| `lane` | `status`, `lane_clean` | string | The lane reference, positional. |
| `name` | `exec` | string | `--name`. |
| `command` | `exec`, `with` | array | The command and its arguments, after `--`. |
| `args` | `devlanes` | array | The command line after `devlanes`, one element per argument. |
| `profile` | `start`, `with` | array | `--profile`, once per item. |
| `all_at_once` | `start`, `restart`, `reload`, `with` | boolean | `--all-at-once`. |
| `mine` | `status`, `logs`, `stop` | boolean | `--mine`. |
| `no_pty` | `exec` | boolean | `--no-pty`. |
| `all` | `logs` | boolean | `--all`. |
| `tail` | `logs` | integer | `--tail`. |
| `head` | `logs` | integer | `--head`. |
| `follow` | `logs` | boolean | `--follow`. |
| `since` | `logs`, `debug_errors` | string | `--since`. |
| `grep` | `logs` | string | `--grep`. |
| `level` | `logs` | string | `--level`. |
| `around` | `logs` | string | `--around`. |
| `context` | `logs` | integer | `--context`. |
| `cursor` | `logs` | string | `--cursor`. |
| `logs_dir` | `stop`, `with` | string | `--logs-dir`. |
| `keep` | `with` | boolean | `--keep`. |
| `list` | `run` | boolean | `--list`. |
| `detach` | `run` | boolean | `--detach`. |
| `explain` | `env_show` | boolean | `--explain`. |
| `dry_run` | `lane_clean` | boolean | `--dry-run`. |

The tools leave out `--output`, `--json`, `--quiet`, `--verbose`, `--budget`, `--project`, `--file`, `start --attach`, `--no-ui`, `status --watch` and `status --statusline`. The `devlanes` tool takes `--project` and `--file`, and drops `--output` and `--json`.

### Read-only tools

Every tool carries a `title` of `devlanes COMMAND` and `openWorldHint: false`. Tools of commands that change nothing carry `readOnlyHint`. The others carry `destructiveHint`, and `start`, `stop` and `reload` also `idempotentHint`. With `--read-only` the server offers `status`, `logs`, `wait`, `debug_why`, `debug_errors`, `debug_top`, `url_print`, `url_ls`, `env_show`, `lane_ls`, `doctor`, to a host with MCP Apps `app_snapshot` and `app_logs`, and the `devlanes` tool, which then runs only these commands:

- `status`, `logs`, `wait`, `doctor`
- `run --list`
- `lane ls`, `lane clean --dry-run`
- `debug why`, `debug errors`, `debug events`, `debug top`, `debug config`
- `env show`
- `url print`, `url open`, `url ls`
- `infra status`, `infra logs`
- `config check`, `config get`
- `remote ls`, `remote sync`
- `daemon status`, `daemon logs` without `--follow`, `daemon clients`

`env export` counts as read-only too, and every tool refuses it ([Refused commands](#refused-commands)). A refused command returns an error that tells the agent to ask the user to run it.

### Refused commands

Every tool refuses these commands and says what to run instead:

| Command | Reason given |
|---|---|
| `ui` | It opens the TUI. |
| `attach` | It attaches a terminal to the unit. |
| `start --attach` | It opens the TUI. |
| `status --statusline` | It is the line an editor or agent shows under its prompt. |
| `logs --follow`, `status --watch`, `debug events --follow`, `infra logs --follow` without `--timeout` | It follows output until interrupted. |
| `env shell` | It opens a subshell. |
| `env export` | It prints export lines for `eval` in a shell. |
| `daemon logs --follow` | It follows the daemon log until interrupted. |
| `daemon run` | The client starts the daemon itself. |
| `setup`, `update`, `uninstall` | It asks questions and may ask for a password. |
| `completions` | It prints a script for a shell's startup file. |
| `mcp` | It is the MCP server itself. |
| `tunnel`, `askpass` | devlanes runs it itself for remote connections. |

A command line that `with` or `debug bundle` runs after `--` is checked the same way.

## Results

A result has one text block and `structuredContent`. It is marked as an error when the command exits with a code other than 0 or the call was cancelled.

The text is the command's agent output, then its standard error, then notes from the server. A result with no text reads `devlanes COMMAND exited CODE`.

| `structuredContent` field | Content |
|---|---|
| `command` | The command, such as `debug why`. |
| `args` | The command line that ran, after `devlanes`. |
| `cwd` | The directory it ran in. |
| `exit_code` | Its exit code. |
| `data` | The JSON document `--json` prints, an array of documents for a command that streams, or `null`. |
| `data_truncated` | `true` when the JSON is over 256 KiB, and then `data` is `null`. |
| `view` | The card that shows the result in a host with MCP Apps, or `null`. |
| `card` | The number of this card among the cards of the same lane, or `null`. |
| `read_only` | Whether the server runs with `--read-only`. |

An error of the command is in `data` as `{"error": {"code", "exit_code", "data"}}`, the same document `--json` prints on standard error. [States and exit codes](states-and-exit-codes.md) lists the exit codes, and [Lane and unit references](lane-and-unit-references.md#errors) the codes for references. Arguments the tool does not take, or of the wrong type, return an error result before anything runs.

### Progress and cancellation

For `start`, `restart`, `reload`, `with`, `lane resume` and `lane new --start`, a host that sends a progress token receives a progress notification for each step of the lane's start, such as `myapp@1 infra started`.

A cancelled call interrupts its command as Ctrl-C would in a terminal, and kills what is left in its process group after 5 seconds. Units the daemon runs keep running, and the result says so. When the server stops, it interrupts the commands still running the same way, with 1 second before the kill. [Windows](windows.md) describes how a command is interrupted there.

## Timeouts

`start`, `restart`, `reload`, `wait`, `with`, `lane resume` and `lane new --start` wait for the lane. When a call to one of them names no `timeout`, the server adds `--timeout` from the client name the host sends:

| Host | Client name contains | `--timeout` added |
|---|---|---|
| Claude Code | `claude-code` | 170 s. With `MCP_TOOL_TIMEOUT` in the server's environment, a tenth less than its value, by at least 1 s and at most 10 s. |
| Codex | `codex` | 50 s |
| OpenCode | `opencode` | 25 s |
| Other hosts | | 55 s |

`MCP_TOOL_TIMEOUT` is in milliseconds; values below 1000 are ignored. A `timeout` the call names is kept as it is. At the timeout the command returns what is ready so far, and the units keep starting in the daemon.

## Consent

These commands ask the user through the host before they run:

| Command | Question |
|---|---|
| `lane clean` without `--dry-run` | `Clean lane LANE? This stops it, runs its clean tasks and teardown, and deletes its data and logs.`, or `Clean this worktree's lane? ...` without a lane, followed by what `lane clean --dry-run` lists |
| `lane gc` | `Clean every lane whose worktree or project file is gone, with its data?` |
| `daemon pair` | `Print a one-time code that lets another machine pair with this one?` |
| `daemon stop` | `Stop the daemon and every process it manages?` |
| `daemon restart` | `Restart the daemon? Running services stop and start again.` |
| `daemon reset-state` | `Stop everything and empty the state database? Lanes get new numbers and ports.` |
| `daemon revoke CLIENT` | `Disconnect CLIENT and delete its pairing?` |
| `remote connect BOX` | `Connect to BOX and mirror its lanes here?` |
| `remote disconnect` | `Disconnect from BOX?`, or `Disconnect from every box?` |
| `config set` of a `remote.*` key | `Set KEY to VALUE in the user config?` |
| `config unset` of a `remote.*` key | `Remove KEY from the user config?` |

Each question ends, after a blank line, with the command line that will run. A command that stops to ask for confirmation, such as `run` of a task with `confirm`, is asked about after its first run: the question is the command line followed by the command's own message, and the command runs again with the flag it named when the user accepts. A read-only tool, and every tool of a server with `--read-only`, never answers a command's own question with yes.

A host on MCP protocol 2026-07-28 or later gets the question as a result that asks for input, and sends the answer with a second call. Older hosts get it as an elicitation request.

| Answer | Result |
|---|---|
| Accepted | The command runs with `--yes`. |
| Declined | Nothing runs. The error reads `the user declined; devlanes ARGS did not run`, with the command line that would have run. |
| Dismissed, or the call cancelled | Nothing runs. The error reads `the question to the user was cancelled; devlanes ARGS did not run`. |
| The host cannot ask | Nothing runs. The error holds the question and tells the agent to ask the user, then call the tool again with `yes: true`, or with `--yes` through the `devlanes` tool. |

A call with `yes` set, or with `--yes` in `args`, is not asked about.

## Environment of a lane started through MCP

A call first runs without sending an environment, so a lane that a shell started keeps that shell's environment. When the lane has none, because no shell started it since the daemon started, the call runs again with the environment below, and the result says which it got. `lane new --start` and `daemon restart` run once, with that environment.

| OS | Environment |
|---|---|
| macOS, Linux | The login shell's, read once per server process. Skipped when `SHLVL` is above 0, unless `DEVLANES_SHELL_ENV` is `1`; `DEVLANES_SHELL_ENV=0` skips it always. |
| Windows | The server's own. With `DEVLANES_SHELL_ENV` set to `pwsh` or `powershell`, that PowerShell's, with its profile, except `PSModulePath`. |

When the login shell's environment cannot be read, the lane gets the server's environment, and a unit that fails with `command not found on PATH` adds a line with the reason. [Environment variables](environment-variables.md) lists `DEVLANES_SHELL_ENV` and `DEVLANES_SHELL_ENV_TIMEOUT`.

When the server's environment names no coding agent, the commands get `AI_AGENT` from the client name, so units they start record that agent as their actor:

| Client name contains | Agent |
|---|---|
| `claude-code` | `claude-code` |
| `codex` | `codex` |
| `opencode` | `opencode` |
| `cursor` | `cursor` |
| `gemini` | `gemini-cli` |
| `goose` | `goose` |
| `copilot`, `visual studio code` | `github-copilot` |

## MCP Apps

A host that shows MCP Apps in the `text/html;profile=mcp-app` type gets a cards resource, `ui://devlanes/lane.html`, which the tools whose results can have a card name, the `devlanes` tool included. The `view` of a result picks the card:

| Command | Card |
|---|---|
| `start`, `restart`, `reload` | `lane`, or `failure` when a unit or the command failed |
| `status` | `lane` for one lane, `board` for several |
| `debug why`, `debug errors` | `failure` |
| `logs` | `failure`, except with `--follow`, `--head`, `--around`, `--grep`, `--cursor`, `--since` or `--level` |
| `lane clean` | `clean` |
| `daemon pair` | `pair` |
| `debug top` | `top` |

The same host gets three tools that only the cards call:

| Tool | Arguments | Runs |
|---|---|---|
| `app_snapshot` | `cwd`, `lane`, `metrics` | `status`, with `debug top` when `metrics` is set and `remote ls` when a box is connected. It never starts the daemon. |
| `app_logs` | `cwd`, `units` (required), `tail`, `cursor`, `grep`, `since` | `logs`. |
| `app_action` | `cwd`, `action` (required), `target` | `start`, `stop`, `restart` or `reload` of the target, `lane clean --yes` (`clean`), `lane clean --dry-run` (`clean_plan`) or `daemon pair` (`pair`). Not offered with `--read-only`. |

A button on a card runs its command without a consent question.
