---
title: CLI reference
description: Every devlanes command and option, with the help text the command prints.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Each page below documents one `devlanes` command and its subcommands, with the same help text that `devlanes <command> --help` prints.

Run a dev environment per git worktree, side by side.

Every worktree of a project is a lane with its own ports, hostnames such as web.acme2.localhost and environment, so branches run at the same time. A per-user daemon owns the processes and containers and keeps them running when the terminal closes; this command and the TUI are its clients. Without a command, devlanes opens the TUI on a terminal and prints status otherwise.

```text
devlanes [OPTIONS] [COMMAND]
```

## Commands

- [`devlanes start`](start.md): Bring up a lane: infra, setup, tasks and services, then wait until ready.
- [`devlanes stop`](stop.md): Stop units, or the whole lane when none are named.
- [`devlanes restart`](restart.md): Stop and start units again with a fresh environment, or the whole lane when none are named.
- [`devlanes reload`](reload.md): Apply project file and .env changes to the running lane.
- [`devlanes status`](status.md): Show lanes and units with state, ports, URLs, CPU and memory.
- [`devlanes logs`](logs.md): Read unit logs; each line carries a cursor to continue from.
- [`devlanes wait`](wait.md): Wait until a unit or lane is ready or failed.
- [`devlanes with`](with.md): Start the lane, run a command against it, then stop what was started.
- [`devlanes run`](run.md): Run a task from devlanes.yaml and exit with its exit code.
- [`devlanes exec`](exec.md): Run an ad-hoc command as a unit of the lane instead of backgrounding it with &.
- [`devlanes attach`](attach.md): Attach this terminal to a unit's terminal; C-a d detaches.
- [`devlanes ui`](ui.md): Open the TUI; closing it never stops anything.
- [`devlanes lane`](lane.md): Create, list, clean and resume worktree lanes.
- [`devlanes debug`](debug.md): Explain failures: why a unit is not up, errors, events and resource use.
- [`devlanes env`](env.md): Show, export or enter the environment of a lane or unit.
- [`devlanes url`](url.md): Print, open and list service URLs and routed hostnames.
- [`devlanes infra`](infra.md): Start, stop and inspect the project's containers.
- [`devlanes config`](config.md): Validate devlanes.yaml, or read and change the user config.yaml.
- [`devlanes setup`](setup.md): Trust the devlanes CA and set up hostnames for this machine; asks once.
- [`devlanes update`](update.md): Install the latest devlanes release the way devlanes was installed, then restart the daemon.
- [`devlanes uninstall`](uninstall.md): Remove devlanes from this machine: stop the daemon, then remove its login service, what devlanes setup changed and this install.
- [`devlanes remote`](remote.md): Connect to boxes and mirror their lanes here.
- [`devlanes daemon`](daemon.md): Run, stop and install the background daemon, and pair other machines with it.
- [`devlanes doctor`](doctor.md): Check the daemon, Docker, proxy, CA, hosts file and output mode, each with a fix; never starts the daemon.
- [`devlanes completions`](completions.md): Print shell completions; lane and unit names come from the running daemon.
- [`devlanes mcp`](mcp.md): Serve devlanes to an AI agent as an MCP server on stdin and stdout.

## Global options

Accepted by every command, before or after the command name.

- `--json`: Print JSON: one document, or NDJSON for streams (same as --output json).
- `--output <MODE>`: Output mode [default: agent when an AI agent runs devlanes, plain in CI or without a terminal, else human].
  - Values: `human` (Colors and symbols, for a terminal), `plain` (The same information without color, one line per event), `agent` (Terse lines for AI agents: failures first, with next commands, size-capped), `json` (The daemon's data as JSON, or NDJSON for streams).
- `-p, --project <PROJECT>`: Use this project instead of the one containing the current directory [env: DEVLANES_PROJECT].
- `--file <FILE>`: Use this project file instead of the nearest devlanes.yaml (for example devlanes.prod.yaml); after the first use, -p \<name> finds its project.
- `--lane <LANE>`: Use this lane ([project@]lane, such as 2, feat/checkout or acme@2) instead of the current worktree's.
- `--yes`: Confirm actions that would ask first; needed when no terminal can prompt.
- `--quiet`: Print nothing on success; errors still go to stderr.
- `--env <KEY=VALUE>`: Set a variable for started processes (start, restart, run TASK, exec, lane new --start); repeatable.
- `--timeout <DURATION>`: Limit waits and streams, such as 30s, 5m or 1h [default for start and wait: 180s].
- `--budget <BYTES>`: Cap agent-mode output at this many bytes [default: 16384, minimum: 256].
- `-v, --verbose`: Print timestamped diagnostics of the client and daemon to stderr; -vv for more. A bare devlanes -v prints the version [env: DEVLANES_VERBOSE=1|2].

## Examples

```text
Examples:
  devlanes start                                bring up this worktree's lane
  devlanes status                               lanes and units of this project
  devlanes logs acme@2:api --tail 200           last 200 lines of api in lane 2
  devlanes with --profile e2e -- pnpm test:e2e  start, run the tests, stop
  devlanes stop --mine                          stop what this agent session started
```

A command names lanes and units with references such as `2`, `feat/checkout` or `acme@2:api`; [lane and unit references](../lane-and-unit-references.md) lists every form.

## Hidden commands

Hidden from `--help` because you rarely type them. A `remote.<box>.command` must end with `devlanes tunnel --stdio` ([connect to a box by command](../../how-to/remote/connect-by-command.md)); devlanes, ssh and the install scripts run the others.

- `devlanes update --finish`: Restart the daemon into this binary after an update (run by devlanes update and the install scripts).
- `devlanes daemon run [OPTIONS]`: Run the daemon in this process (spawned by the client).
- `devlanes tunnel [OPTIONS] --stdio`: Serve a connecting client over stdin and stdout (run by remote connect over SSH, or by a remote.\<box>.command).
- `devlanes askpass [OPTIONS] [PROMPT]`: Answer an ssh prompt through the connected client (SSH_ASKPASS).
