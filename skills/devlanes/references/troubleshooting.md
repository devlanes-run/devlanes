# Troubleshooting

Read this when a command fails or a unit is not up. Find the message or state you see; text in capitals, such as `UNIT` or `PORT`, stands for what devlanes prints there. Two commands find most causes: `devlanes debug why UNIT` for one unit, `devlanes doctor` for the machine. Before a fix stops or changes what someone else started (another lane, the user's units, a program outside devlanes), ask the user.

## A unit failed

A start prints failed units first, with the end of their log and a `more:` command; `devlanes status` exits 1 while one is `failed`.

```sh
devlanes debug why api
devlanes logs api --tail 200
devlanes debug errors --since 15m
```

| Message | Cause | Fix |
|---|---|---|
| `UNIT couldn't start: PROGRAM: command not found on PATH` | The program is not on the `PATH` of the shell that started the lane, or dependencies are not installed in this worktree. | Install the dependency, or start from a shell that has the tool: `devlanes restart UNIT`. Through MCP on Windows, see `DEVLANES_SHELL_ENV` in [mcp.md](mcp.md). |
| `UNIT couldn't start: PROGRAM is not executable` | The file lacks execute permission. | `chmod +x` it, or run it through its interpreter in `run`. |
| `UNIT couldn't start: working directory DIR does not exist` | `cwd` names a folder this worktree lacks. | Fix `cwd`, then `devlanes reload`. |
| `exited with code N`, `killed by signal N` | The process ended. | The log shows why. Fix the code, then `devlanes restart UNIT`. |
| `task TASK failed with exit code N` | A task of the lane failed; services after it may not have started. | `devlanes logs TASK --tail 200`, fix, then `devlanes run TASK` or `devlanes start`. |
| `infra setup failed: ...` | An `infra.setup` entry failed. | `devlanes logs setup.NAME`, fix, then `devlanes run infra.setup.NAME`. |
| `Docker isn't running or can't be reached` | The project has containers and Docker is down. | Ask the user to start Docker, then `devlanes start`. |
| `UNIT is still running from a previous daemon (pid N); start does not take it over` | An `orphaned` unit. | `devlanes stop UNIT --lane LANE`, then start it. |
| `NAME is a configured service of LANE; give exec another --name, or run the configured service` | `exec` reused a unit name. | Pick another `--name`. |

A service that crashed under a watcher (`node --watch`, `nodemon`) can still show `ready`: the watcher keeps the process alive, and a ready check runs only during a start. Read its logs when it misbehaves. A failed unit stays `failed` until you restart or stop it; devlanes does not restart it.

## A ready check never passes

`UNIT wasn't ready in time`, often with the last answer, such as `GET /health (Host: 127.0.0.1:20060) answered 404`. The service keeps running and holds its port.

| Cause | Fix |
|---|---|
| `http` path the app does not serve, or a non-2xx answer | Point `ready.http.path` at a route that answers 2xx, set `ready.http.host` when the app routes by hostname, or set `ready.http.status`. |
| `nothing accepts connections on port PORT; it listens on ...` | The app ignores the port devlanes gave it. Make it listen on `PORT` (`devlanes env show LANE:UNIT` shows it), with a preset or `--port ${{ self.port }}` ([configuration.md](configuration.md)). |
| A `log` regex that never matches | Compare it with `devlanes logs UNIT --tail 50`; escape regex characters. |
| A first compile takes longer than the check's `timeout` | Raise `ready.timeout`. |

After the fix, `devlanes restart UNIT` runs it with the edited check.

## A port is held

| Message | Fix |
|---|---|
| `port PORT is taken by pid N` | Another program holds a port of the lane. `devlanes debug errors` names it. Ask the user before you end a program devlanes did not start. A `port PORT is held by PID N` line under a unit that failed its ready check can be that unit's own process. |
| `port PORT is taken by LANE:UNIT` | Another lane's unit holds it. The hint is `devlanes stop LANE:UNIT`; run it only for your own units or with the user's consent. |
| `port PORT is mirrored from BOX for UNIT` | A box's mirror holds the port; [remote.md](remote.md). |
| `the port pool has no free block of N ports for another lane`, a lane `unassigned` | Every block of `ports.range` belongs to a lane. `devlanes lane ls` lists them; with the user's agreement, `devlanes lane gc --yes` cleans lanes whose worktree is gone, and `devlanes lane clean LANE --yes` cleans one. |
| `config changed ports; devlanes reload --lane LANE restarts the lane with them` | An edit needs ports the running lane lacks. Run the reload; it restarts the whole lane. |
| `doctor`: `port PORT is held by PROGRAM (pid N); the LISTENER proxy will fall back to another port` | Another program holds 443 or 80, so URLs carry 8443 or 8080. Pass the URLs on as printed, or tell the user. |

## The daemon

| Message | Cause | Fix |
|---|---|---|
| `auto-spawn refused in sandbox (REASON); run devlanes daemon install outside the sandbox, then retry`, exit 4 | Your shell is sandboxed. | "When the shell cannot reach the daemon" in [setup.md](setup.md). |
| `daemon is unreachable: ...`, exit 4 | The daemon does not answer on its socket or pipe. | `devlanes doctor`, then `devlanes daemon logs --tail 100`. |
| `the daemon has not finished starting: step "STEP" made no progress for Ns` | A starting daemon is stuck on one step. | `devlanes daemon logs --tail 100`; report it to the user. |
| `the running daemon is devlanes VERSION ... and cannot serve this CLI`, `protocol_mismatch` | The daemon runs another release than the CLI. | The hint is `devlanes daemon restart`, which restarts every service on the machine. Ask the user first. |
| `doctor`: `terminal: this terminal ends the processes it started when it closes` (Windows) | A daemon started from this shell ends with the terminal. | Ask the user to run `devlanes daemon install` and `devlanes daemon start` in their own terminal. |
| `doctor`: `daemon` not running | No daemon yet. | Nothing; the next command that needs it starts it. |
| `doctor`: `user config PATH:LINE: MESSAGE` | The user config does not load, and the daemon will not start with it. | Show the user the line; `devlanes config unset KEY` restores a default. |

## A hostname does not resolve or is not trusted

| What you see | Cause | Fix |
|---|---|---|
| `ERR_CERT_AUTHORITY_INVALID` or another certificate error in the user's browser | That browser or the system does not trust the devlanes CA. | `devlanes doctor` shows `tls` lines. The user runs `devlanes setup` ([setup.md](setup.md)); Firefox and Chrome on Linux need `tls.trust`, see [Trust the devlanes CA](https://devlanes.run/docs/how-to/hostnames/trust-the-ca.md). |
| A certificate error from `curl` or a script in your shell | Your shell's tool does not use the system store. | Use the loopback URL from `devlanes status`, or `curl --cacert ~/.devlanes/ca/ca.pem` on macOS and Linux. On Windows, `curl.exe` reads the system store and needs `--ssl-revoke-best-effort`, since the devlanes CA publishes no revocation list. Units already get the CA. |
| A program cannot resolve a `*.localhost` name, such as Node's `ENOTFOUND` | Its resolver does not send `*.localhost` to loopback. | Between services, use `${{ services.api.url }}`, the loopback URL. Otherwise `lanes.hosts_file: always` in the project file writes the exact hostnames to `/etc/hosts` on macOS and Linux, through the helper `devlanes setup` installs. |
| Over HTTPS the connection fails with no page (`curl: (35)`, `ERR_SSL_PROTOCOL_ERROR`); over HTTP the DL101 page | No lane routes that hostname. | `devlanes url ls` lists the routed hostnames. Check `hosts` and that the lane runs. |
| `HOST is routed by LANE and OTHER; LANE keeps it, so give one of them another hostname` | Two lanes claim one hostname. | Keep hostnames under `lane.domain`. |
| DL201 `Service not running`, DL202 `Service starting`, DL203 `Service failed`, DL301 `Service not listening` | The proxy reached no working service. | `devlanes status`, then `devlanes debug why UNIT`. |

[Fix programs that cannot resolve *.localhost](https://devlanes.run/docs/how-to/hostnames/resolve-localhost.md) covers each OS.

## A lane waits for a start from a shell

`devlanes status` shows `needs start from a shell: devlanes start --lane LANE`, or a command fails with `the daemon restarted and lost this lane's shell environment`. The daemon restarted, and it never stores a shell's environment, so the lane needs one start from a shell before its services run.

- Your own lane: run `devlanes start` in its worktree.
- Lanes the user runs: ask the user to run `devlanes lane resume` in their terminal, which starts every lane that was running with that terminal's environment.
- Through MCP, the server retries with the login shell's environment by itself ([mcp.md](mcp.md)).

## The config changed (drift)

`devlanes status` shows `config changed` on a lane, and units carry a line such as `api: ready changed since it started`. The project file or `.env` differs from what runs; nothing restarted.

- `devlanes reload` applies every change: removed units stop, changed ones restart, added ones start. A change to infra or ports restarts the whole lane.
- `devlanes reload api` applies one unit's change.
- On a lane the user runs, a reload restarts the user's services; ask first.

## Other messages

| Message | Fix |
|---|---|
| `no devlanes.yaml in DIR or its parents`, exit 3 | The command ran outside a project. `cd` into the worktree, or write one ([setup.md](setup.md)). |
| An error with the hint `devlanes config check` | Run it; it names the file, line and column. |
| `project NAME is already used by OTHER; set another project: in this file` | Pick another `project`. |
| `project_ambiguous` | Several projects share the folder. Run in one project's folder, or pass `-p PROJECT`. |
| `no profile NAME; profiles: ...` | Use a listed profile. |
| `not selected by profile NAME`, `autostart is off; it only starts when named` | Name the unit: `devlanes start UNIT`. |
| `...; rerun with --yes`, exit 2 | Add `--yes` only when the user asked for that action. |
| The MCP server answers that a command does not run here | The command needs a terminal; ask the user to run it. |

[Troubleshooting](https://devlanes.run/docs/troubleshooting.md) lists more messages, each with the page that fixes it.
