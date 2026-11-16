---
title: States and exit codes
description: The states of units and lanes, the exit codes of the CLI, its output modes and the shape of its JSON output.
kind: reference
---

`devlanes status`, the TUI and `--json` output report a state for each unit and lane. Each command ends with an exit code that scripts and agents can act on.

## Unit states

| State | Meaning |
|---|---|
| `idle` | A service that has not run since the daemon started. |
| `queued` | Waiting for its turn to start. |
| `starting` | A service whose process runs and whose ready check has not passed yet. |
| `ready` | A service whose process runs and whose ready check passed. |
| `running` | A task, `exec` command, or setup or teardown entry whose process runs. |
| `exited` | A task, `exec` command, or setup or teardown entry that ended with exit code 0. |
| `failed` | The process exited with another code, could not start, or did not pass its ready check. |
| `stopping` | A stop was sent and the process has not ended yet. |
| `stopped` | Ended by a stop, or a service that ended by itself with exit code 0. |
| `orphaned` | A process an earlier daemon started that still runs, unsupervised. `devlanes stop` ends it. |
| `external` | An `infra.external` entry: devlanes routes to it and runs nothing. |

A unit stays `failed` until you act on it. `devlanes stop NAME` clears a failed service or `exec` command; a failed task stays failed until it runs again.

Human output and the TUI show a `ready` service of an unused lane as `throttled` or `frozen` ([Throttling and freezing](../concepts/freezing.md)). JSON keeps `state: ready` and adds `power`.

## Lane states

| State | Meaning |
|---|---|
| `starting` | A start is under way, or a service is starting. |
| `running` | A unit of the lane runs. |
| `failed` | A service of the lane has failed. |
| `stopping` | The whole lane is stopping. |
| `idle` | Nothing of the lane runs. |
| `unassigned` | The lane has no number or no ports yet, such as when the port pool is full. |
| `no-config` | The worktree has no project file for this project. |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Failure. `status` also exits 1 while a unit it shows has failed. |
| `2` | Usage error, or a confirmation that needs `--yes`. |
| `3` | A lane, unit or project that matches nothing or several things, or no `devlanes.yaml` in the directory or its parents. |
| `4` | The daemon cannot be reached or cannot start. |
| `124` | `--timeout` passed before the units were ready. |
| `130` | Ctrl-C detached the command and left the units running. |

`devlanes with` exits with the code of the command it ran, and `devlanes run` with the code of the task.

## Output modes

| Mode | Output |
|---|---|
| `human` | Colors and symbols, for a person at a terminal. |
| `plain` | The same information without color, one line per event. |
| `agent` | Terse lines for coding agents: failures first, each with the command to run next, capped in size. |
| `json` | The daemon's data as JSON. |

`--output MODE` or `--json` chooses the mode, then `DEVLANES_OUTPUT`. Without them, devlanes uses `agent` when a coding agent runs it, `plain` in CI or without a terminal, and `human` otherwise.

## JSON output

With `--json`, a command prints one JSON document. A list has the fields `items`, `truncated` and `next_cursor`. Commands that stream, such as `start`, `logs -f` and `debug events`, print one JSON object per line. Errors print as `error:` and `hint:` lines on standard error in every mode, `--json` included.
