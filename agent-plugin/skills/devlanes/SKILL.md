---
name: devlanes
description: Start, stop, inspect and test dev services with the devlanes CLI. Use in repositories that have a devlanes.yaml, whenever you would otherwise start a dev server, watcher or other long-running command in the background with `&`, and when writing or fixing a devlanes.yaml or migrating to devlanes from mprocs, dekit, process-compose, a Procfile, pm2, tmuxinator, tmuxp, concurrently, npm-run-all, mise or Docker Compose.
---

# devlanes

devlanes runs each Git worktree of a project as a lane with its own ports, hostnames and data. A per-user daemon owns every process. The CLI sends it requests and prints a result, so a dev server keeps running after your shell command returns, and its logs stay queryable.

When the CLI detects an agent it prints terse, size-capped text and never prompts. Add `--json` to any command for complete structured output.

This page covers daily use. Read a reference first when the task is about the config itself:

- [references/configuration.md](references/configuration.md) before creating or editing `devlanes.yaml`, or when a failure comes from the config (a service never ready, wrong port, missing hostname, a task in the wrong place).
- [references/migration.md](references/migration.md) when the user asks to move an mprocs, dekit, process-compose, Procfile, pm2, tmuxinator, tmuxp, package.json or mise setup to devlanes.

## Rules

- Never start a long-running process with `&`, `nohup`, `setsid` or a background shell. Use `devlanes start` for the services in `devlanes.yaml` and `devlanes exec --name NAME -- COMMAND` for anything else.
- `start`, `restart` and `wait` return once units are ready or have failed. Do not poll with `sleep` or `curl` loops.
- Read logs with `--tail`, `--head` (the first lines, such as how a start began), `--grep` or `--since`. Output is capped at 16 KiB per command, and truncated output ends with the command that fetches the rest. Follow logs (`-f`) only together with `--timeout`.
- A command that needs confirmation exits 2 and names the flag. Rerun it with `--yes` only when the user asked for that action.
- A command run without the unit, task or lane it needs (`devlanes url open`, `devlanes run`, `devlanes logs`) exits 2 and lists the options with a command to run; with `--json` they are in `error.data.options`. Pick one from that list. Exit 3 there means nothing can be chosen, such as no devlanes.yaml in this directory or its parents.
- Clean up with `devlanes stop --mine`. It stops only what your session started. Stopping or restarting a unit a human started works, and the result says so; do it only when the task requires it.
- Interrupting a command (Ctrl-C or a tool timeout) detaches from it. Services keep running.

## Lanes and units

A lane reference is `project@N` (lane number), `project@branch`, or `N` / `branch` alone inside the repository. Lane 0 is the main checkout.

A unit is a service, task, `exec` command or `infra.external` entry, referenced as `[lane:]unit`: `api`, `2:api`, `acme@feat/login:web`. Commands that take several units accept globs and tags: `"api-*"`, `+backend`.

Output prints lanes as `acme@2 (feat/billing)`. Paste references back unchanged. A reference that matches nothing or several lanes fails with exit 3 and lists the candidates.

Commands use the project of the nearest `devlanes.yaml` above the current directory, up to the repository root. A repository can hold more projects: a `devlanes.yaml` in a subdirectory, or another file such as `devlanes.prod.yaml`, which is named once with `--file devlanes.prod.yaml` (`devlanes --file devlanes.prod.yaml start`) and afterwards reached with `-p PROJECT`. `-p` also reads `DEVLANES_PROJECT`, which devlanes sets in every process it starts, so devlanes commands run from a service or task stay in its project. Renaming `project:` makes a new project with new lanes, ports and hostnames; the old project's lanes stay listed until `devlanes lane gc --yes`, which you run only when the user agrees.

## Commands

| Goal | Command |
|---|---|
| Bring up this worktree's lane | `devlanes start`, or `devlanes start api web` |
| States, URLs and ports | `devlanes status`, `devlanes status --mine` |
| Wait for a unit or lane | `devlanes wait api --timeout 180s` |
| Recent logs | `devlanes logs api --tail 100` |
| Search logs | `devlanes logs api --grep 'ECONNREFUSED' --since 10m` |
| Lines around a log cursor | `devlanes logs api --around CURSOR -C 20` |
| Crashes, exit codes, port conflicts | `devlanes debug errors --since 15m` |
| Reason a unit is not running | `devlanes debug why worker` |
| Restart after a code change | `devlanes restart api`, or `devlanes restart` for the whole lane |
| Apply a `devlanes.yaml` edit to what runs | `devlanes reload`, or `devlanes reload api` |
| Run a configured task | `devlanes run --list`, then `devlanes run migrate` |
| Ad-hoc long-running command | `devlanes exec --name storybook -- pnpm storybook` |
| Route a server devlanes does not run | `infra.external` in `devlanes.local.yaml` ([Ports outside the config](#ports-outside-the-config)) |
| URL of a service | `devlanes url print web` |
| Environment of a unit | `devlanes env show api`, or `eval "$(devlanes env export api)"` |
| Tests against the stack | `devlanes with -- pnpm test:e2e` |
| Stop what you started | `devlanes stop --mine` |
| Setup problems | `devlanes doctor` |

## Running tests

`devlanes with [units...] [--profile P] -- COMMAND` starts the lane, waits for ready, runs COMMAND in the foreground and exits with its exit code. COMMAND gets the lane's environment plus `DEVLANES_PUBLIC_URL_<SERVICE>` for every service, so tests need no hardcoded ports. Afterwards it stops what it started. If a unit fails to start, it prints that unit's log tail and exits 1 without running COMMAND. `--keep` leaves the stack running; `--logs-dir DIR` writes every unit's log to DIR.

## Reading output

`devlanes start` with one failing service:

```
acme@2 (feat/billing): 7/8 ready, 1 failed, seed skipped
failed admin-api exit=1
  | Error: Cannot find module './env'
  |     at Module._resolveFilename (node:internal/modules/cjs/loader:1145:15)
  |     at /.../apps/admin-api/src/main.ts:3:1
  more: devlanes logs acme@2:admin-api --tail 200
ready crm-api https://crm.acme-2.localhost
ready user-web https://app.acme-2.localhost
ready +5 (devlanes status acme@2)
```

`devlanes status`:

```
running acme@0 main 8/8
failed acme@2 feat/billing 7/8 failed=admin-api
idle acme@3 fix/login
unassigned acme spike/xyz reason="pool full"
```

Failures come first, each with its last log lines and the command that shows more. URLs are printed in full. The first word of a status line is the state:

| State | Meaning and next step |
|---|---|
| `ready` | Up and passing its ready check. Use it. |
| `frozen`, `throttled` | A ready service in a lane that had no activity for a while (agent output shows `ready` with `power=frozen` or `power=throttled`, JSON `state: ready` with a `power` field). Any request, connection or devlanes command on the lane resumes it; no action needed. |
| `starting`, `queued` | Starting, or waiting for a start slot. `devlanes wait UNIT --timeout 180s`. |
| `running` | A task or exec in progress. Wait for it or read its logs. |
| `external` | An `infra.external` entry: devlanes routes its port and hostnames but runs nothing there, so it has no logs and `start` or `stop` do not apply. Whatever listens on the port is yours to run. |
| `failed` | Exited or never got ready. Read the printed tail, then `devlanes logs UNIT --tail 200` and `devlanes debug why UNIT`; fix the code and `devlanes restart UNIT`, or fix the config and `devlanes reload`. A service that keeps failing its ready check usually has the wrong port or check: see references/configuration.md. |
| `exited` | A task or exec that ended. `devlanes logs UNIT --tail 50` shows how. |
| `stopping`, `stopped`, `idle` | Not running. `devlanes start UNIT` when you need it. |
| `orphaned` | A previous daemon left the process running and nothing supervises it. `devlanes stop UNIT --lane LANE`, then start it again. |

An error ends with a `hint:` line naming the command that fixes it. Ask the user before running a hint that stops services this session did not start. Examples are `devlanes stop --lane LANE` when `lane clean` refuses a lane that still runs, and `devlanes daemon restart` when the running daemon is another devlanes version (`protocol_mismatch` in JSON), which stops every service the daemon runs.

With `--json`, a command prints one JSON document; lists have the shape `{items, truncated, next_cursor}`. Streams (`start --json`, `logs -f --json`, `debug events -f`) print NDJSON.

Exit codes: 0 success, 1 failure, 2 usage error or missing `--yes`, 3 unknown or ambiguous reference, 4 daemon unreachable, 124 timeout.

## Typical loop

```sh
devlanes start
devlanes debug errors                # only if something failed
devlanes logs api --tail 200
# fix the code, then
devlanes restart api
# or, after editing devlanes.yaml
devlanes reload
devlanes with -- pnpm test
devlanes stop --mine
```

## Ports outside the config

An `exec` command gets no port or hostname from devlanes, and only what devlanes runs or declares is reachable from machines connected with `devlanes remote connect`. To give an ad-hoc server, such as `devlanes exec --name docs-server -- python -m http.server 8000`, a hostname or make it reachable from those machines, declare it in `devlanes.local.yaml` next to `devlanes.yaml`. That file holds one developer's changes and stays out of Git; create it when it does not exist, and add it to `.gitignore` if it is not ignored yet:

```yaml
infra:
  external:
    docs:
      address: 127.0.0.1:8000
      hosts: "docs.${{ lane.domain }}"
```

Every lane then lists the unit `docs` of kind `external`, `https://docs.<lane domain>` routes to the port, and connected machines mirror it while a lane of the project runs. Give it a name no exec unit has (`exec` refuses it), use a port outside `ports.range` (20000-32767 by default), and only `127.0.0.1:<port>` or `[::1]:<port>`. `start`, `stop` and `logs` do not apply. Put it in `devlanes.yaml` when the team runs that server; `hidden: true` keeps it off connected machines.

## Working on a remote machine

When the user works on another machine connected to this one with `devlanes remote connect`, the URLs devlanes prints here are this machine's, and the user opens services on theirs. Before giving the user a URL, run `devlanes url print UNIT --json`. Its `clients` list has one entry per connected machine: with `reachable: true`, give the user that entry's `url`; with `reachable: false`, its `reason` says why the unit does not open there (a port in use there, a lane or hostname that machine does not mirror). An empty `clients` list means no connected machine reports to this one. When `devlanes daemon clients --json` lists the user's machine as paired, tell them to run `devlanes remote connect BOX` on it. Otherwise, unless this machine's user config sets `remote.require_pairing: false`, run `devlanes daemon pair` here and pass on its line: the user runs `devlanes remote connect BOX --code CODE` on their machine, with BOX the name they reach this machine by (SSH host or `remote.boxes` name). A code works once, within 10 minutes. When the user says their machine waits in `needs_pairing`, it needs a new code the same way. Never set `remote.require_pairing: false` unless the user asks for it.

## Daemon and sandboxes

The CLI starts the daemon on first use. Inside a sandbox (for example Codex, which sets `CODEX_SANDBOX`) it refuses to and fails with `auto-spawn refused in sandbox`. Ask the user to run `devlanes daemon install` outside the sandbox. It registers a per-user service that starts the daemon now and at login. Then retry. `devlanes doctor` reports the socket and sandbox state, and never starts the daemon: without one it says `daemon: not running` and checks what it can locally.
