# devlanes

`devlanes` runs a project's dev environment once per git worktree, each in its own lane with its own ports, hostnames and data. This package is the single binary: the CLI, the TUI and the daemon, wired together with every concrete integration.

## Usage

```sh
devlanes start                      # infra, setup, tasks, then services of this worktree's lane
devlanes start web --profile e2e    # one service, with a profile
devlanes status                     # lanes of this project; outside a repo, all projects
devlanes status acme@2              # one lane, by project and lane number
devlanes logs web --tail 50 -f      # follow one unit's output
devlanes attach web                 # raw terminal; C-a d detaches
devlanes lane ls                    # worktree lanes, including idle and unassigned ones
devlanes with --profile e2e -- pnpm test:e2e
devlanes daemon status
devlanes daemon install             # launchd agent or systemd user unit
```

Run `devlanes` with no arguments on a terminal to open the TUI; elsewhere it prints `status`. Add `--json` to any command for machine-readable output. Closing the TUI, or pressing Ctrl-C during `start`, `logs -f` or `wait`, leaves everything running.

On a terminal, `start`, `restart`, `reload`, `lane resume` and `lane new --start` run inside the TUI, like mprocs: the request goes out first, and the TUI opens on the lane once the daemon has accepted it, so a refused start (no config, unknown unit) is a normal CLI error. The TUI follows the start's own events and exits 0 when it was ready, 1 when it failed, and 130 when the user quits before the result. `--no-ui`, `DEVLANES_NO_UI=1` or `tui.open_on_start: false` keep the text output; `start --attach` forces the TUI past the last two. Agent, JSON, plain, CI and piped output never open it. `with`, `wait`, `run` and `stop` stay text.

`devlanes with` starts the lane, waits until it is ready, runs the command with the lane's env and `DEVLANES_PUBLIC_URL_<SERVICE>` variables, stops what it started and exits with the command's code.

Configuration examples and design notes are in `docs/` at the repository root. The agent skill is in `skills/devlanes/`.

## Config example

```yaml
# ~/.devlanes/config.yaml: per-user settings for every project.
# Ports the HTTPS and HTTP proxy listen on.
proxy:
  https_port: 443
  http_port: 80
# Start progress in the terminal tab's progress bar: auto, always or never.
terminal:
  progress: auto
```

Project settings live in each repository's `devlanes.yaml`; `docs/examples/user-config.yaml` lists every user setting. `devlanes config get [KEY]`, `config set KEY VALUE` and `config unset KEY` read and change `config.yaml` without the daemon, keeping its comments.

## Behaviour

Any command except `devlanes daemon status`, `daemon stop`, `daemon reset-state`, `doctor`, `setup`, `update` and `uninstall` auto-spawns the daemon (`devlanes daemon run`, a hidden subcommand) when none is running; `daemon status` reports it as not running and exits 1, and `doctor` reports it as a warning and runs the checks it can without it (home, user config, CA trust, processes on the proxy ports, the hosts helper, release). The daemon inherits only `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `LC_*`, `TZ`, `TMPDIR`, `TERM`, `XDG_*`, `DOCKER_*`, `KUBECONFIG`, `SSH_AUTH_SOCK`, the proxy variables, `DEVLANES_*` and CI markers from that shell. Services and tasks still get the environment of the shell that sent each `start`, `run` or `exec`, and Docker Compose gets the daemon's Docker-related variables plus `infra.env` and the primary worktree's `.env`. Inside an agent sandbox the CLI refuses to auto-spawn; start the daemon from a normal shell or install it once with `devlanes daemon install`. `devlanes daemon stop`, `devlanes daemon restart` and `devlanes daemon reset-state` are the only commands that stop all managed processes, and they ask first; `reset-state` also empties `state.db`. Without a running daemon, `reset-state --yes` moves `state.db` to `state.db.legacy-<time>` and the lane data folders into `<data folder>/.legacy-<time>` instead, deleting nothing, since a new database numbers lanes from 1 again. `daemon restart` checks the user config first, so an invalid file stops nothing.

Everything lives under `~/.devlanes`, or `DEVLANES_HOME` when set: `state.db`, `config.yaml`, `logs/` (including `daemon.log`), `ca/` and `run/daemon.sock`. When the socket path is too long for macOS, it moves to a short temporary directory. devlanes refuses a home that other users can read or write, so create it with `mkdir -m 700 ~/.devlanes` when you add `config.yaml` before the first run.

## Daemon shutdown

`devlanes daemon stop` and `devlanes daemon restart` first run the `lane.stop` tasks of every lane with running units, as `devlanes stop` does for one lane: lanes in parallel, each lane's tasks in file order and within their own timeouts. Then every unit stops with its stop policy, all at once. A failed task is logged and listed in the result, and the other tasks and the stops still run. The result names the stopped units and each task's outcome. Containers keep running.

SIGTERM or SIGINT (launchd, systemd, logout, system shutdown, `kill`) stops the units the same way but runs no `lane.stop` task, so a machine shutdown does not wait for them. A signal that arrives while `daemon stop` runs its tasks interrupts them and skips the rest.

`devlanes daemon install` gives the daemon 60 seconds between SIGTERM and SIGKILL: `ExitTimeOut` in the launchd plist, `TimeoutStopSec` in the systemd unit. Without it, launchd allows 5 seconds (macOS 26; `launchd.plist(5)` only says the default is system-defined) and systemd 90 seconds. A shutdown takes about the longest unit stop timeout (10 seconds unless `stop.timeout` sets one) plus a few seconds, so the window covers stop timeouts up to 55 seconds. `devlanes doctor` names each unit whose stop timeout does not fit, with the fix. Running `daemon install` again rewrites a definition that differs and reloads it: systemd keeps the running daemon, and launchd restarts the daemon it runs, which stops its units.

## Diagnostics

`-v` (or `DEVLANES_VERBOSE=1`) prints timestamped debug lines on stderr: the output mode and why it was chosen, the home, config and socket paths, connecting to or spawning the daemon and how long that took, the versions exchanged, each request with its duration, and why the exit code is what it is. The daemon adds its own lines for that request, marked `daemon +<ms>`: lane resolution, config reads, port allocation, the Docker Compose commands it runs and their output, start gate waits, readiness probes and each spawn with its program, working directory and environment variable names. The run ends with a table of timings per request and start phase. `-vv` (or `DEVLANES_VERBOSE=2`) adds trace detail and span durations. stdout is untouched, so `--json -v` still prints parseable JSON. Environment and secret values never appear, only variable names. `-v` keeps `start` out of the TUI so the lines stay visible.

The daemon writes its own log to `~/.devlanes/logs/daemon.log` (rotated at 10 MiB to `daemon.log.1`). `DEVLANES_LOG` sets its level with `RUST_LOG` syntax, such as `DEVLANES_LOG=debug` or `DEVLANES_LOG=info,devlanes_daemon=trace`; the default is `info`. The variable is read when the daemon starts, so restart it with `devlanes daemon restart` after changing it. `devlanes daemon logs` (`-f` to follow, `--tail N`) prints the file.

`devlanes debug bundle` writes a `.tar.gz` for bug reports and prints its path: client and daemon versions, `doctor`, `config check` and `status` as JSON, and the end of `daemon.log`. `devlanes debug bundle -- start api` also runs `devlanes -vv start api` and adds its output. Values of variables named like tokens, keys or passwords, passwords in URLs, `env` maps and the home directory are replaced before anything is written. It never starts a daemon. `--to PATH` chooses the file; by default it goes to the temporary directory.

```sh
devlanes -v start                   # what the client and daemon did, with timings
devlanes --json -vv start 2>trace.txt
DEVLANES_LOG=debug devlanes daemon restart
devlanes debug bundle -- start api
```

## Updates

The daemon checks for new releases every 4 hours and when the TUI opens (`updates.mode` in `config.yaml`: `disable`, `notify` or `auto`). `devlanes update`, or `U` in the TUI, installs the latest release the way devlanes was installed and restarts the daemon; it never installs an older one, and `install.sh` and `install.ps1` take no version either (npm and PyPI install a given release: `npm install -g devlanes@X.Y.Z`, `uv tool install devlanes==X.Y.Z`). A build of `main`, installed with `install.sh --channel next` or `install.ps1 -Channel next`, updates to the latest build of `main`. `devlanes uninstall` removes devlanes. [Updates](../../docs/guides/updates.md) covers builds of `main`, background installs, Windows and the install scripts.

## License

Apache-2.0
