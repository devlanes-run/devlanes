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

Configuration examples and design notes are in `docs/` at the repository root. The Claude Code and Codex plugin, with the agent skill, is in `agent-plugin/`.

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

Any command except `devlanes daemon status`, `daemon stop`, `daemon reset-state` and `doctor` auto-spawns the daemon (`devlanes daemon run`, a hidden subcommand) when none is running; `daemon status` reports it as not running and exits 1, and `doctor` reports it as a warning and runs the checks it can without it (home, user config, CA trust, processes on the proxy ports, the hosts helper, release). The daemon inherits only `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `LC_*`, `TZ`, `TMPDIR`, `TERM`, `XDG_*`, `DOCKER_*`, `KUBECONFIG`, `SSH_AUTH_SOCK`, the proxy variables, `DEVLANES_*` and CI markers from that shell. Services and tasks still get the environment of the shell that sent each `start`, `run` or `exec`, and Docker Compose gets the daemon's Docker-related variables plus `infra.env` and the primary worktree's `.env`. Inside an agent sandbox the CLI refuses to auto-spawn; start the daemon from a normal shell or install it once with `devlanes daemon install`. `devlanes daemon stop`, `devlanes daemon restart` and `devlanes daemon reset-state` are the only commands that stop all managed processes, and they ask first; `reset-state` also empties `state.db`. Without a running daemon, `reset-state --yes` moves `state.db` to `state.db.legacy-<time>` and the lane data folders into `<data folder>/.legacy-<time>` instead, deleting nothing, since a new database numbers lanes from 1 again. `daemon restart` checks the user config first, so an invalid file stops nothing.

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

## Release notices

The daemon reads the latest version 30 seconds after it starts, then every 4 hours, and when a TUI opens if the last check is over an hour old. It reads the registry it was installed from, with `curl`:

- `install.sh`, `install.ps1`, standalone binaries and cargo: the GitHub release manifest (`releases/latest/download/release-manifest.json`, or `releases/download/next/release-manifest.json` on the `next` channel).
- npm, pnpm, Bun and Yarn: `https://registry.npmjs.org/devlanes/latest`.
- uv, uvx, pipx and pip: `https://pypi.org/pypi/devlanes/json`.
- Homebrew: the formula in `devlanes-run/homebrew-tap`.

Only `install.sh`, `install.ps1` and standalone binaries follow `next`; the registries carry stable releases. Project-local installs and source builds are not checked. For a new release the daemon also reads its `CHANGELOG.md` and keeps the entries since the running version. Answers are cached in `update-check.json`, and a failed request keeps the cached answer.

In human output, a command then prints one line on stderr, at most once a day per version, such as `devlanes 0.9.1 is available (this is 0.9.0): devlanes update`, with `(breaking changes)` for a new minor version. Installs devlanes cannot update itself (cargo, `uvx`, `pipx run`, pip, a project's own dependency, a source build, a script install in a directory this user cannot write to) name their own command instead. When the daemon runs another version than the CLI, the line says to run `devlanes daemon restart` when convenient, since a restart stops services. Agent, JSON, plain and CI output never get these lines. The TUI shows the same notice in its header, where `U` updates, restarts the daemon and opens the TUI again on the same lane, and the summary printed when the TUI closes has an `Update` line. `devlanes doctor` always shows the version, build, install method and the path that runs it, the update mode, channel and registry, and the latest release with its command. Shown and hidden notices are kept in `notices.json`.

`devlanes update` checks, asks (`[Y/n]` for a patch release; a minor release lists its breaking changes and asks `[y/N]`), installs and restarts the daemon, which starts the lanes that ran again. Script and standalone installs download the release archive into `~/.devlanes/update/`, check it against the release's `SHA256SUMS`, its exact entries and the version the new binary reports, and rename the binaries into place. npm, pnpm, Bun, Yarn, uv, pipx and Homebrew installs run their package manager, with its output in `~/.devlanes/logs/update.log`. One update runs at a time (`~/.devlanes/update/lock`). The installed binary then runs `devlanes update --finish`, which restarts the daemon and prints what changed.

`devlanes update --next` and `devlanes update --stable` move between the two channels: they set `updates.channel` in `config.yaml`, keeping its comments, then update on that channel, with the switch shown as the first step. `next` is the latest build of `main` (`0.9.1-next.14`), published only as release archives, so `--next` needs an `install.sh` or `install.ps1` install in a directory you can write to; any other install fails before the config changes, with the commands that reinstall devlanes with the script. `--stable` from a `next` build installs only a stable release newer than that build; until one ships, it records the channel and says the build stays. `--stable --force` installs the latest stable release anyway, with a warning: its daemon may refuse the state database the newer build migrated, and `devlanes daemon reset-state` then empties it.

```sh
devlanes update --next               # follow the latest build of main
devlanes update --stable             # back to releases once one is newer
devlanes update --stable --force     # the latest release now, even if older
```

Each build carries the notes of its `CHANGELOG.md`. The first TUI opened on a newer stable release shows "What's new" after the welcome: every release since the version it last showed, breaking changes first. A fresh home records its version without showing anything, and `next` and local builds never show it. The Esc menu's `what's new` opens it again, and `devlanes update --notes` prints the running version's notes. The version last shown is kept in `notices.json`.

With `updates.mode: auto`, the daemon installs patch releases in the background: script installs download and stage them, package managers install them. Nothing restarts; the next interactive `devlanes` (the TUI or a start that opens it) switches to the new version and restarts the daemon without asking. When the daemon runs an older minor version than the TUI opening, it asks first. A project's own devlanes never restarts the shared daemon; it prints a warning. A background install that failed shows in `devlanes update --check` and is tried again at the next check.

On Windows, the daemon, the TUI and `devlanes update` keep a hard link to their executable in `~/.devlanes/run/exe/<pid>.exe` while they run, so a package manager can replace `devlanes.exe` meanwhile.

Checks are off in debug builds, when the daemon starts with CI variables set, with `DEVLANES_NO_UPDATE_CHECK=1`, or with this in `config.yaml`:

```yaml
updates:
  mode: disable # notify (the default) or auto
  channel: stable # or next; defaults to the running build's channel
```

## Uninstall

`devlanes uninstall` stops the daemon (asking first when it runs processes), removes the login service, runs `devlanes setup --remove` with sudo asked for once, and removes this install: `install.sh` and `install.ps1` files and, on Windows, the user `PATH` entry; for npm, pnpm, Bun, Yarn, uv, pipx, pip, Homebrew and cargo installs, their remove command. A project's own dependency, `uvx`, `pipx run` and source builds stay, with a line saying why. A step that fails is reported and the rest still run. Last it lists `~/.devlanes`, lane data under a `lanes.data_dir` outside it and a daemon socket outside it, with their sizes, and asks before deleting them (no by default). `--remove-data` deletes them without asking, `--keep-data` keeps them, and without a terminal they stay. `--dry-run` prints the plan and changes nothing. On Windows a running `devlanes.exe` cannot delete itself, so a detached `cmd.exe` deletes it once the command exits. `install.sh --uninstall` and `install.ps1 -Uninstall` run it from the install directory.

```sh
devlanes uninstall --dry-run
devlanes uninstall --yes --keep-data   # no questions, data kept
```

## License

Apache-2.0
