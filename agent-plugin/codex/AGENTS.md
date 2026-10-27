## Dev services (devlanes)

This repository runs its dev services with devlanes. A per-user daemon owns the processes; the `devlanes` CLI talks to it.

- Start this worktree's lane with `devlanes start`. It waits for ready or failure.
- Never background a process with `&` or `nohup`. Use `devlanes exec --name NAME -- COMMAND` instead. To route a port it serves or make it reachable from a connected machine, declare it under `infra.external` in `devlanes.local.yaml` under a name the exec unit does not have (`docs: { address: 127.0.0.1:8000, hosts: "docs.${{ lane.domain }}" }` for an exec named `docs-server`). An entry's state is `external`: devlanes does not run it, so `start`, `stop` and `logs` do not apply.
- When the user works on another machine connected to this one, give them the reachable `url` from `clients` in `devlanes url print UNIT --json`; with no clients, tell them to run `devlanes remote connect BOX` on their machine.
- Lanes are `project@N` or `project@branch`; units are `LANE:UNIT`. Globs and `+tags` select several units.
- Inspect with `devlanes status`, `devlanes wait UNIT --timeout 180s`, `devlanes debug errors` and `devlanes debug why UNIT`.
- Read logs with `devlanes logs UNIT --tail 100`, `--head 50` or `--grep RE`, and continue with `--cursor`. Follow logs only with `--timeout`.
- Run tests with `devlanes with -- COMMAND`. It starts the lane, waits, runs COMMAND, stops what it started and exits with COMMAND's code.
- Clean up with `devlanes stop --mine`, which stops only what this session started; `devlanes status --mine` lists it.
- A command that needs confirmation exits 2. Rerun it with `--yes` only when the user asked for that action.
- A command run without the unit, task or lane it needs exits 2 and lists the options with a command to run; pick one from that list. Exit 3 there means nothing can be chosen, such as no devlanes.yaml in this directory or its parents.
- Ctrl-C detaches from services. Foreground tasks started with `devlanes run TASK` are the exception.
- A status line starts with the state: `ready`, `starting`, `queued` (waiting for a start slot), `running`, `failed`, `stopping`, `stopped`, `exited` (a task or exec that ended), `idle` or `orphaned`.
- `frozen` or `throttled` (`ready` with `power=frozen` or `power=throttled` in agent output, a `power` field in JSON) means the lane had no activity for a while. Any request, connection or devlanes command on the lane resumes it, so no action is needed.
- `orphaned` means a previous daemon left the process running and nothing supervises it. Run `devlanes stop UNIT --lane LANE`, then start it again.
- An error ends with a `hint:` line naming the command that fixes it. Ask the user before running a hint that stops services this session did not start, such as `devlanes stop --lane LANE` when `lane clean` refuses a running lane, or `devlanes daemon restart`, which stops every service the daemon runs.
- Add `--json` for complete structured output.
- When a failure is unclear, rerun the command with `-v` (or `-vv`): the client's and the daemon's diagnostics go to stderr, so stdout keeps its format.
- Inside a sandbox (Codex sets `CODEX_SANDBOX`) the CLI does not start the daemon. If a command fails with `auto-spawn refused in sandbox`, ask the user to run `devlanes daemon install` outside the sandbox, then retry.
