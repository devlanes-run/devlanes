---
title: States and exit codes
description: The states of units, containers and lanes, the exit codes of the CLI, its output modes and the shape of its JSON output.
kind: reference
---

`devlanes status`, the TUI and `--json` output report a state for each unit, container and lane. Each command ends with an exit code that scripts and agents can act on.

## Unit states

| State | Units | Meaning |
|---|---|---|
| `idle` | services | Configured, and not run since the daemon started. |
| `queued` | units a start asked for | Waiting to start: for a place in the machine-wide start gate, or for an earlier step of the lane's start, such as its containers. `reason` says which. |
| `starting` | services | The process runs and its ready check has not passed yet. |
| `ready` | services | The process runs and its ready check passed. |
| `running` | tasks, `exec` commands, `infra.setup` and `infra.teardown` entries | The process runs. These units have no ready check. |
| `exited` | tasks, `exec` commands, `infra.setup` and `infra.teardown` entries | The process ended on its own with exit code 0. |
| `failed` | every unit with a process | The process exited with a code other than 0, could not start, or did not pass its ready check. `reason` and `exit_code` say which. |
| `stopping` | every unit with a process | A stop was sent and the process has not ended yet. |
| `stopped` | every unit with a process | Ended by a stop, or a service that ended on its own with exit code 0. |
| `orphaned` | every unit with a process | A process started by an earlier daemon that still runs. Nothing supervises it: it has no output, ready check or terminal. `start` refuses it; `stop`, a lane stop, `restart` and `lane clean` end it. |
| `external` | `infra.external` entries | An address devlanes routes to and runs nothing for. The state stays the same whether or not something listens there. |

A unit stays `failed` until something acts on it. `devlanes stop NAME` makes a failed service or `exec` command `stopped`, and so does stopping the whole lane. A failed task keeps its state until it runs again.

### Words shown for a ready unit

Human output and the TUI show some `ready` units with another word. JSON keeps `state: ready` and adds a field.

| Shown | Meaning | In JSON | In agent output |
|---|---|---|---|
| `frozen` | The lane had no activity for a while and its processes are suspended. The next request, connection or command resumes it. | `power: "frozen"` | `ready` with `power=frozen` |
| `throttled` | The lane had no activity for a while and its processes are slowed down. | `power: "throttled"` | `ready` with `power=throttled` |
| `running` | A service whose ready check is only that its process started. | `ready_check: "started"` | `ready` |

[Idle lanes](../concepts/idle-lanes.md) explains what counts as activity.

### Drift

A unit that the project file has changed since it started carries `drift`, which `devlanes reload` applies.

| `drift.kind` | Meaning |
|---|---|
| `changed` | The unit runs, and the file would start it differently. `drift.fields` names what differs, such as `run`, `cwd`, `ports`, `pty`, `ready`, `stop`, `files` or `env API_URL`. |
| `removed` | The unit runs, and the file no longer has it. |
| `added` | The lane's profiles select the unit now, and it is not running. |

## Container states

Containers of a lane are listed under `containers`, each with the state Docker reports.

| State | Meaning |
|---|---|
| `created` | Created and not started. |
| `starting` | Running, and its health check has not passed yet. |
| `running` | Running, with no health check. |
| `healthy` | Running, and its health check passed. |
| `unhealthy` | Running, and its health check fails. |
| `exited`, `exited <code>` | Ended, with its exit code when Docker gives one. |
| `oom` | Ended because it ran out of memory. |
| `removed` | Removed. |
| `missing` | There is no container. |
| Another word | Docker's own word for that state. |

For a Compose service with several replicas, the state is that of the worst one.

## Lane states

A lane takes the first state in this table that applies.

| State | Meaning |
|---|---|
| `no-config` | The worktree has no project file, or its file names another project, and the lane has no number. |
| `unassigned` | The lane has no number or no port block yet. `reason` says why: `lanes_max` (the project has `lanes.max` lanes), `pool_exhausted` (no free block is left in the port pool) or `no_ports`. |
| `stopping` | A stop of the whole lane is under way. |
| `starting` | A start is running its steps, or a service is starting or queued. |
| `failed` | A service of the lane has failed. |
| `running` | A unit of the lane runs, an orphaned one included. |
| `idle` | Nothing of the lane runs. |

Other fields of a lane:

| Field | Meaning |
|---|---|
| `reference` | The lane reference to paste into commands, such as `myapp@1` ([Lane and unit references](lane-and-unit-references.md)). |
| `power` | Set while the lane is idle: `level` is `throttled` or `frozen`, `since_ms` when it got there, `last_activity` what it last saw: `proxy`, `connection`, `cpu`, `command` or `stop`. |
| `config_changed` | A unit has a `drift`, or the lane's infra settings changed. |
| `infra_changed` | The lane's infra settings changed, so a reload restarts the whole lane. |
| `needs_env` | The daemon restarted and the lane needs one start from a shell before its services run. |
| `moves_to` | The worktree changed branch, and its next start moves it to this lane number. |
| `warnings` | Conditions that do not stop the lane: `ports_pending` (the edited config needs ports the running lane's block lacks) and `host_conflict` (another lane routes the same hostname). |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Failure: a unit failed, a check failed, or the daemon refused the request. |
| `2` | Usage error: an unknown flag, an invalid reference, a missing argument, or a confirmation that needs `--yes`. |
| `3` | Not found or ambiguous: a lane, unit or project that matches nothing or several things, or no `devlanes.yaml` in the directory or its parents. |
| `4` | The daemon cannot be reached, cannot start, refuses to start in a sandbox, or runs a release with another protocol (`protocol_mismatch`). |
| `124` | A timeout passed first: `--timeout` of `start`, `restart`, `reload` or `wait`. |
| `127` | `devlanes with`: the command was not found or could not be started. |
| `130` | Ctrl-C detached the command and left the units running, a picker was cancelled, or the TUI of a start was closed before the start ended. |
| `128 + N` | `devlanes with` ended by signal N, after stopping what it started: 130 for SIGINT, 143 for SIGTERM. |
| The command's code | `devlanes with` exits with the code of the command it ran, and `devlanes run` with the code of the task. |

Per command:

| Command | Exit code |
|---|---|
| `start`, `restart`, `reload` | `0` when every started unit is ready, `1` when one failed, `124` on timeout. |
| `wait` | `0` when ready or, for a task or `exec` command, when it exited with 0; `1` when it failed; `124` on timeout. |
| `status`, `stop`, `lane ls` | `1` while a lane or unit it shows is `failed`, a task or `exec` command that exited with another code than 0 included. |
| `with` | `1` without running the command when a unit fails to start; otherwise the command's code. |
| `run` | The task's exit code. |
| `doctor` | `1` when a check fails. |
| A start in the TUI | `0` when the start was ready, `1` when it failed, `130` when the TUI closed before the start ended. |

## Output modes

| Mode | Output |
|---|---|
| `human` | Colors and symbols, for a person at a terminal. A start on a terminal runs in the TUI. |
| `plain` | The same information as `human`, without color, one line per event. |
| `agent` | Terse lines for coding agents: failures first, each with the command to run next. Output is capped at 16 KiB, or at `--budget BYTES` (at least 256); capped output ends with the command that prints the rest. Progress is not printed, and nothing asks for confirmation. |
| `json` | The daemon's data as JSON, or NDJSON for streams. |

The mode is the first of these that applies:

| Step | Mode |
|---|---|
| 1 | `--output MODE`. |
| 2 | `--json`: `json`. |
| 3 | `DEVLANES_OUTPUT`. |
| 4 | A coding agent runs the command: `agent`. |
| 5 | A CI system runs the command: `plain`. |
| 6 | Standard output is a Windows console that cannot show escape sequences: `plain`. |
| 7 | Standard output is a terminal: `human`. |
| 8 | Otherwise: `plain`. |

[Environment variables](environment-variables.md#agents-and-sandboxes) lists the variables that mark an agent and a CI system. `devlanes doctor` prints the mode and the reason it was chosen.

Color is on in `human` output to a terminal outside CI. With `CLICOLOR_FORCE`, and on GitHub Actions and GitLab CI, `human` and `plain` output have color too. `NO_COLOR` turns it off in every mode. `--quiet` prints nothing when the command succeeds.

`devlanes status` in agent output with both lanes running, and in human output with a service of lane 0 failed:

```console title="agent output, on Windows 11"
$ devlanes status
running myapp@0 main 3/3
running myapp@1 feat/checkout 3/3
```

```console title="human output, exit code 1, on Windows 11"
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           failed   2/3       0.0%  74.6 MiB  https://myapp.localhost
  1     feat/checkout  running  3/3       0.0%  92.8 MiB  https://myapp1.localhost
```

## JSON output

With `--json` or `--output json`, a command prints one JSON document on standard output: the data the daemon answered with. Its exit code is the same as in the other modes.

A list has this shape:

| Field | Meaning |
|---|---|
| `items` | The entries. |
| `truncated` | `true` when entries were left out. |
| `next_cursor` | The cursor that fetches the next entries, or `null`. |

These commands stream and print NDJSON, one JSON object per line:

- `start`, `restart`, `reload`
- `lane new --start`
- `with`
- `wait`
- `run TASK`, without `--list` and `--detach`
- `logs -f`, `infra logs -f`
- `status --watch`
- `debug events -f`

Each event line has the same fields:

| Field | Meaning |
|---|---|
| `seq` | The event's number. |
| `event` | Its name, such as `log`, `lane.phase` or `lane.warning`. |
| `data` | The event's data. |

A streaming command that ends, such as `start`, prints its result as the last line: the same document a command without a stream prints.

## Errors

An error goes to standard error, in every mode:

| Line | Content |
|---|---|
| `error: MESSAGE` | What went wrong. |
| Indented lines | Context, such as the last lines of a log. |
| `hint: COMMAND` | The command that fixes it or shows more. |

In agent output the error is capped like the rest of the output. A command that needs an argument it did not get, and cannot ask for it, prints this instead in `json` mode, on standard error:

| Field | Meaning |
|---|---|
| `error.code` | `argument_missing`. |
| `error.exit_code` | `2`, or `3` when there is no project or lane to choose from. |
| `error.data.command` | The command, such as `url open`. |
| `error.data.argument` | What it needs: `unit`, `service`, `task` or `lane`. |
| `error.data.options` | Up to 20 choices, each with `value` and `detail`. |
| `error.data.total` | How many choices there are. |
| `error.data.example` | The command with the first choice filled in. |
| `error.data.reason`, `error.data.message` | When there is nothing to choose: `no_config`, `no_lane` or `no_options`, and why. |

The tools of `devlanes mcp` return every error with `code`, `exit_code` and `data` ([MCP tools](mcp-tools.md)). The codes for references are on [Lane and unit references](lane-and-unit-references.md#errors).
