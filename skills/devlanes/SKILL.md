---
name: devlanes
description: Start, stop, inspect and test dev services with the devlanes CLI. Use in repositories that have a devlanes.yaml, whenever you would otherwise start a dev server, watcher or other long-running command in the background with `&`, when setting devlanes up in a repository or on a machine, when writing or fixing a devlanes.yaml, and when moving a project to devlanes from mprocs, dekit, process-compose, a Procfile, pm2, tmuxinator, tmuxp, concurrently, npm-run-all, mise tasks or Docker Compose.
---

# devlanes

devlanes runs each Git worktree of a project as a lane with its own ports, hostnames and data folder. A per-user daemon owns every process, so a dev server keeps running after your shell command returns and its logs stay readable.

Open a reference when the task calls for it:

| Situation | Read |
|---|---|
| devlanes is not installed, or the repository has no `devlanes.yaml` yet | [references/setup.md](references/setup.md) |
| Writing or changing `devlanes.yaml` or `devlanes.local.yaml`, or a failure that comes from the config | [references/configuration.md](references/configuration.md) |
| Moving a project from mprocs, dekit, process-compose, a Procfile, pm2, tmuxinator, tmuxp, `concurrently`, npm-run-all, mise tasks or Docker Compose | [references/migration.md](references/migration.md) |
| Registering `devlanes mcp`, or your shell cannot reach the daemon | [references/mcp.md](references/mcp.md) |
| A box: a lane on another machine, pairing, `devlanes remote` | [references/remote.md](references/remote.md) |
| A unit that fails, a ready check that never passes, a port, hostname, certificate, daemon or sandbox problem | [references/troubleshooting.md](references/troubleshooting.md) |

## Words

| Word | Meaning |
|---|---|
| project | What one `devlanes.yaml` describes, named by its `project:` key. |
| lane | One worktree's place to run the project, kept whether its services run or not: lane 0 for the primary worktree (`myapp.localhost`), lane 1 for the next (`myapp1.localhost`), each with its own block of ports. |
| unit | A service, task, `exec` command, `infra.setup` or `infra.teardown` entry, or `infra.external` entry. Containers under `infra` sit next to the units and are not units. |
| box | A machine whose lanes are mirrored to another machine under the same hostnames and ports. Its lanes are started and stopped on the box. |

## Long-running processes go through devlanes

- The services in `devlanes.yaml`: `devlanes start`, or `devlanes start api web`.
- Anything else that keeps running (a watcher, storybook, a tunnel): `devlanes exec --name storybook -- pnpm storybook`.

Never run these with `&`, `nohup`, `setsid`, `Start-Process` or a background shell, and never track their PIDs. Commands that end by themselves (builds, tests, git, scripts) run in your shell as usual; when one needs the lane running, run it with `devlanes with -- COMMAND`.

## Run this worktree's lane

| Goal | Command |
|---|---|
| Start the lane, wait until ready or failed | `devlanes start` |
| Start some units | `devlanes start api web`, `devlanes start --profile e2e` |
| Continue a start that hit its timeout | `devlanes wait 2 --timeout 180s` |
| Restart after a code change | `devlanes restart api`, or `devlanes restart` for the whole lane |
| Apply an edit of `devlanes.yaml` or `.env` | `devlanes reload` |
| Run a configured task | `devlanes run --list`, then `devlanes run migrate` |
| Environment of a unit | `devlanes env show api`, `devlanes env show api --explain` |
| Machine problems | `devlanes doctor` |

`start`, `restart`, `reload` and `wait` return once the units are ready or one failed, or when `--timeout` passes (180 seconds by default for `start` and `wait`), with exit 124. Do not poll with `sleep` or `curl` loops. A plain `devlanes start` brings back what the lane ran before, else the project's default profile. A start into a running lane starts only the selected services the lane lacks and runs no tasks.

A new worktree gets its own lane on its first `devlanes start`, with new ports, hostnames, data folder and lane number. It needs the project file in the worktree, so `devlanes.yaml` must be committed on its branch. That first start runs the project's `lane.create` tasks (installs, codegen) and setup entries; when the project has none, install dependencies in the new worktree before you start. A worktree without its own `devlanes.local.yaml` or `.env` reads the primary worktree's.

```sh
git worktree add ../myapp-checkout -b feat/checkout
cd ../myapp-checkout
devlanes start
```

Real output of a first start, by an agent on Windows 11:

```text
myapp@2 (agent/perf): 2/2 ready
ready api http://127.0.0.1:20080 https://api.myapp2.localhost
ready web http://127.0.0.1:20082 https://myapp2.localhost
```

## URLs and what to tell the user

| Goal | Command |
|---|---|
| Lanes of this project | `devlanes status` |
| One lane with its units, URLs and ports | `devlanes status 2` |
| Only what your session started | `devlanes status --mine` |
| One service's URL | `devlanes url print web` |
| Every routed hostname of the project | `devlanes url ls` |

Each service line gives its loopback URL and, when it has hostnames, its HTTPS URL. A URL can carry a port, such as `https://myapp.localhost:8443` when the proxy could not get 443; pass it on as printed. Tests and scripts read `DEVLANES_PUBLIC_URL_<SERVICE>` instead of a fixed port.

You cannot open the user's browser or the TUI. `devlanes url open` only prints the URL for you. Give the user the HTTPS URL to open, and tell them they can watch every lane by running `devlanes` in their own terminal. When the user's browser runs on another machine connected to this one, read [references/remote.md](references/remote.md) before giving a URL.

## Logs and failures

| Goal | Command |
|---|---|
| Last lines | `devlanes logs api --tail 100` |
| First lines | `devlanes logs api --head 50` |
| Search | `devlanes logs api --grep ECONNREFUSED --since 10m` |
| Since the lane's last start, every unit | `devlanes logs --all --since start` |
| Lines around a cursor | `devlanes logs api --around api#120 -C 20` |
| Continue after a cursor | `devlanes logs api --cursor api#120` |
| Follow for a while | `devlanes logs api -f --timeout 30s` |
| Why a unit is not running or not ready | `devlanes debug why api` |
| Failed units, crashes, port conflicts | `devlanes debug errors --since 15m` |
| A container's own logs | `devlanes infra logs postgres` |

Each log line starts with a cursor such as `api#120`. In agent output `logs` gives the last 100 lines by default. `-f` needs `--timeout`.

A failed start prints the failed units first, each with the end of its log and a `more:` line with the command that shows the rest. Read it, then `devlanes debug why UNIT`. Fix the code and run `devlanes restart UNIT`, or fix the config and run `devlanes reload`. devlanes never restarts a unit by itself: a failed unit stays `failed` until something starts or stops it. [references/troubleshooting.md](references/troubleshooting.md) goes by message.

## Tests against a lane

```sh
devlanes with -- pnpm test
devlanes with --profile e2e -- pnpm test:e2e
devlanes with api --logs-dir devlanes-logs -- cargo test --test api
```

`devlanes with` starts what the lane needs, waits until ready, runs the command, then stops what it started, and the daemon too when it started it. Without units or `--profile`, it starts what the lane ran before, else the default profile. The command gets the lane's environment, `DEVLANES_PUBLIC_URL_<SERVICE>` for each service, and variables that point Node, Deno and Python at the devlanes CA unless your shell sets them. Everything after `--` runs directly, without a shell; use `sh -c '...'` for pipes or `&&`.

- When a unit fails to start, it prints the failure and exits 1 without running the command.
- Otherwise it exits with the command's code: 127 when the command was not found or could not start, 128+N when signal N ended it.
- On a running lane it starts only the selected services the lane lacks, runs no tasks and afterwards stops only those. When `--profile` would give the running services other settings (a script, `env`, `infra`), it exits 1 before it starts anything: stop the lane, run in another worktree, or name the services instead of the profile.
- `--keep` leaves what it started running. `--logs-dir DIR` writes the log of each unit and task it started to DIR.

## Ad-hoc long-running commands

```sh
devlanes exec --name watch-tests -- node --test --watch "api/*.test.mjs"
```

Real output, by an agent on Windows 11:

```text
running myapp@2:watch-tests actor=opencode pid=53356
  logs: devlanes logs myapp@2:watch-tests --tail 100
  stop: devlanes stop myapp@2:watch-tests
```

The daemon owns the process. It gets your shell's environment, then the lane's, including `DEVLANES_PUBLIC_URL_<SERVICE>`; `--env KEY=VALUE` wins over both. The command runs directly, without a shell. Give it a name no configured unit has. It gets no port or hostname of its own; [references/configuration.md](references/configuration.md) shows `infra.external` for a server that needs a hostname. `devlanes wait NAME` waits for an `exec` command to exit, so never wait on one that serves.

## Stop only what you started

| Goal | Command |
|---|---|
| Stop what your session started in this project | `devlanes stop --mine` |
| Stop one unit | `devlanes stop api`, `devlanes stop myapp@2:watch-tests` |

`devlanes stop --mine` stops the units your agent session started in every lane of the current project, and leaves the rest running, the user's units and shared containers included. Outside a project it covers every project. When your units also run in other projects, it names them with the command for each, such as `devlanes -p myapp-prod stop --mine`. A bare `devlanes stop` stops the whole lane, the user's units included; run it only when the user asks.

- When you hand the work to the user for review, leave the lane running and give them its URL.
- As a subagent, stop the units you started by name before you return. Your parent may run in another session, which `stop --mine` does not reach, or in the same one, where your `stop --mine` stops its units too.
- Stopping or restarting a unit someone else started works, and the result says so. Do it only when the task needs it.

Interrupting a command (Ctrl-C, or your tool's timeout) does different things:

| Command | On interrupt |
|---|---|
| `start`, `restart`, `reload`, `wait`, `logs -f` and the others | Detaches with exit 130. Units keep running. |
| `devlanes run TASK` in agent output | Stops the task. |
| `devlanes with` | Passes the signal to the command, stops what it started, exits 128+N. |

## Output, states and exit codes

devlanes detects coding agents and prints terse agent output: failures first, each with the command to run next, capped at 16 KiB (`--budget BYTES` changes the cap). Capped output ends with the command that prints the rest. Agent output never asks a question. `--json` prints the daemon's data as one JSON document; lists are `{items, truncated, next_cursor}`. These stream NDJSON, one event per line, ending with the result: `start`, `restart`, `reload`, `lane new --start`, `with`, `wait`, `run TASK` (without `--list` or `--detach`), `logs -f`, `infra logs -f`, `status --watch` and `debug events`.

An error goes to stderr as text in every mode, `--json` included: an `error:` line, indented context lines, and a `hint:` line with the command that fixes it or shows more. The one exception is a command that needs a unit, task or lane and got none: with `--json` it prints `{"error": {"code": "argument_missing", ...}}` on stderr, with the choices in `error.data.options` and a filled-in command in `error.data.example`. In other modes it lists the choices and ends with `hint:`. Pick one from the list.

Before you run a hint that stops what you did not start, ask the user. `devlanes daemon restart`, the hint when the daemon runs a release with another protocol (`protocol_mismatch`), restarts every service the daemon runs.

A command that needs confirmation prints `...; rerun with --yes` and exits 2. Add `--yes` only when the user asked for that action.

| Unit state | Meaning | Next step |
|---|---|---|
| `idle` | A service not run since the daemon started. | `devlanes start UNIT` when you need it. |
| `queued` | Waiting for a start slot or an earlier step, such as containers. | `devlanes wait UNIT` |
| `starting` | Running, ready check not passed yet. | `devlanes wait UNIT` |
| `ready` | Running, ready check passed. A lane nobody used for a while shows `ready` with `power=throttled` or `power=frozen`; the next request, connection or devlanes command thaws it. | Use it. |
| `running` | A task, `exec` command or setup entry still running. Human output and the TUI also show a ready service whose ready check is `started` as `running`. | Read its logs. |
| `exited` | A task, `exec` command or setup entry that ended with 0. | |
| `failed` | Exited with another code, could not start, or failed its ready check. A service that failed its ready check keeps running and holds its port. | Read the tail, `devlanes debug why UNIT`. |
| `stopping`, `stopped` | Stopping, or ended by a stop. | |
| `orphaned` | Left running by an earlier daemon, unsupervised. | `devlanes stop UNIT --lane LANE`, then start it. |
| `external` | An `infra.external` entry: routed, nothing to run. | Whatever listens there is the user's. |

`devlanes stop NAME` makes a failed service or `exec` command `stopped`; a failed task stays failed until it runs again. A lane's first word in `status` is its state: `running`, `starting`, `failed`, `stopping`, `idle`, `unassigned` or `no-config`. A lane that needs one start from a shell after a daemon restart shows `needs start from a shell: devlanes start --lane LANE`. `config changed` on a lane means the project file differs from what runs; `devlanes reload` applies it.

| Exit code | Meaning |
|---|---|
| 0 | Success. |
| 1 | Failure: a unit failed, a check failed, the daemon refused. `status`, `stop`, `lane ls` and `url ls` exit 1 while something they show is `failed`, a task or `exec` command that exited non-zero included. |
| 2 | Usage error, a missing `--yes`, a missing argument, or a lane with nothing of the kind asked for. |
| 3 | Lane, unit or project not found or ambiguous, no `devlanes.yaml` here or above, or a project file no command has started. |
| 4 | Daemon unreachable, not startable, refused inside a sandbox, or on another protocol. |
| 124 | `--timeout` passed first. |
| 127 | `with`: the command was not found or could not start. |
| 130 | Detached by Ctrl-C, units still running. |
| 128+N | `with` ended by signal N. |
| other | `with` and `run` exit with the command's or task's code. |

[States and exit codes](https://devlanes.run/docs/reference/states-and-exit-codes.md) has every field.

## References to lanes and units

| Form | Example |
|---|---|
| Lane: number, branch, lane name, folder name or path, with an optional `project@` | `2`, `feat/checkout`, `lane1`, `myapp-checkout`, `myapp@2` |
| Unit: `[lane:]unit` | `api`, `2:api`, `myapp@feat/checkout:web` |
| Replica | `worker.2` |
| Several units, in commands that take several: glob, tag, or a service name for every replica | `"api-*"`, `+backend`, `worker` |

Output names lanes as `myapp@2` and units as `myapp@2:api`; paste them back unchanged. Quote globs so the shell leaves them alone. Without a lane, a command uses the lane of the current worktree, and without `-p` the project of the nearest `devlanes.yaml` up to the repository root. A reference that matches nothing or several lanes exits 3 and lists `candidates`. Units of one command must name their lane in the same words: `1:api myapp@1:web` fails with `mixed_lanes`. Another project file, such as `devlanes.prod.yaml`, is named once with `devlanes --file devlanes.prod.yaml start` and reached with `-p myapp-prod` afterwards. [Lane and unit references](https://devlanes.run/docs/reference/lane-and-unit-references.md) has the matching rules.
