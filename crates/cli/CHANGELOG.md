# devlanes-cli

## 0.8.0 — 2026-10-28

### Minor changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key; devlanes no longer creates worktrees.
  
  Create one with `git worktree add <path> <branch>` and run `devlanes start` in it. Configs that still set `lanes.worktree_dir` fail to load until it is removed. `devlanes lane new --name` still adds another lane for the current folder.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Changed projects to be identified by their `project:` name, so one repository can hold several projects: a `devlanes.yaml` in a subdirectory, or another file such as `devlanes.prod.yaml` named once with the new `--file` flag and then with `-p`/`--project` (which also reads `DEVLANES_PROJECT`).
  
  Renaming `project:` now makes a new project; the old one's lanes stay until `devlanes lane gc --yes` cleans them. Another repository can no longer use a name already taken. In a project file below the worktree root, relative `cwd`, `run_script.file` and `sync` paths now start at that file's directory. `lanes.max` comes from the primary worktree's file, and a worktree whose file sets it, `lanes.primary` or shared infra differently gets a warning. Commands run inside a process devlanes started now default to that lane's project; pass `-p` to choose another. See [several projects in one repository](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/services.md).

### Patch changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added `devlanes reload`, which applies project file and `.env` edits to a running lane: it stops removed services, restarts changed ones and starts added ones.
  
  `devlanes status` and the TUI show what each edit changed, `r` on a changed lane in the TUI reloads it, and `devlanes restart` without services restarts the whole lane instead of asking which to restart.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added `devlanes debug config`, which prints a lane's project config as devlanes reads it, after `extends`, local overrides and templates, with the files it came from; `${{ }}` expressions are printed as written.

## 0.7.1 — 2026-10-28

### Patch changes

- [ab1c65e](https://github.com/devlanes-run/devlanes/commit/ab1c65edc4924def207485353865d0a551bb10ec) Added `NODE_EXTRA_CA_CERTS` and `DENO_CERT` (the devlanes CA) and `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` (the CA bundle) to the environment of the command `devlanes with` runs, unless the shell already sets them, so tests trust the lane's `https://` URLs.
- [ab1c65e](https://github.com/devlanes-run/devlanes/commit/ab1c65edc4924def207485353865d0a551bb10ec) Changed the summary of `devlanes with` and `devlanes stop --logs-dir`: it now names the lane (and for `with`, the exit code), lists each unit's state, run time and average and peak CPU and memory, and the GitHub step summary adds the last log lines of each failed unit in a collapsed block.
- [ab1c65e](https://github.com/devlanes-run/devlanes/commit/ab1c65edc4924def207485353865d0a551bb10ec) Fixed `devlanes start` on GitHub Actions printing `::endgroup::` lines without the `::group::` that opens them; each start phase now gets its own log group.
- [ab1c65e](https://github.com/devlanes-run/devlanes/commit/ab1c65edc4924def207485353865d0a551bb10ec) Fixed `devlanes certs trust --hosts` and `devlanes doctor` on Windows assuming Windows resolves every `*.localhost` name; when the lookup fails, as on Windows Server, they now say to add the hostnames to the hosts file from an elevated shell or use the resolver strategy.

## 0.7.0 — 2026-10-28

### Minor changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Removed `devlanes remote connect --install`; a box without devlanes now fails with `devlanes is not installed on <box>` and a link to the remote setup guide.
  
  Install devlanes on the box yourself, the same release as on this machine, with `install.sh` (`install.ps1` on Windows), Homebrew, npm or cargo binstall, so that it is on the `PATH` of a non-interactive SSH session or in `~/.devlanes/bin` (`install.sh --dir ~/.devlanes/bin`; `%USERPROFILE%\.devlanes\bin` on Windows). See `docs/guides/remote-setup.md`.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed `devlanes remote connect` so a box serves only machines it paired with, and added `devlanes daemon pair`, `devlanes daemon clients`, `devlanes daemon revoke`, `remote.require_pairing` and `remote.allow_connections`.
  
  A machine's first connection to a box needs a one-time code: run `devlanes daemon pair` on the box, then `devlanes remote connect <box> --code <code>` here, or enter the code when `devlanes remote connect <box>` asks. Later connections, including those after a daemon restart, need none. To let every machine that gets in over SSH connect without a code, as before, set `remote.require_pairing: false` in the box's user config. To turn every machine away, set `remote.allow_connections: false`. See "Pairing" in `docs/remote.md`.

### Patch changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed `devlanes remote connect` to reconnect boxes after a daemon restart until `devlanes remote disconnect`, and to wait for another `devlanes remote connect` instead of failing when a reconnect needs a password or new credentials.
  
  While a box waits, `devlanes remote ls` and `devlanes status --json` show `needs_auth` with the command to run, and its hostnames show a sign-in page (DL205). Boxes connected before this release are not reconnected on their own; run `devlanes remote connect <box>` once.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed opening the URLs of a box whose proxy serves HTTPS on another port than this machine's, such as 8443 on a box that cannot bind 443: the client now serves that box's hostnames on that port too, and `devlanes remote ls` warns when it cannot.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `infra.external.<name>` for a server devlanes does not run: every lane lists it as a unit of kind `external`, the proxy routes its `hosts` to its loopback `address`, and machines connected with `devlanes remote connect` mirror it while a lane of the project runs.
  
  A server only you run goes in `devlanes.local.yaml`; see `docs/guides/infra.md#external-ports`:
  
  ```yaml
  infra:
    external:
      pgadmin:
        address: 127.0.0.1:5050
        hosts: "pgadmin.${{ lane.domain }}"
  ```
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed the remote protocol to version 4, so `devlanes remote connect` works with boxes whose login shell or profile prints to stdout; the client and the box both need this release.
  
  To upgrade, install this release on the box yourself (devlanes no longer installs it there) and on this machine, then run `devlanes daemon restart` on both. Each daemon upgrades its state database on that first start, and earlier releases cannot open it afterwards. Then connect each box once with a pairing code: `devlanes daemon pair` on the box, `devlanes remote connect <box> --code <code>` here.
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Fixed `devlanes start` of services on a lane that runs or is being brought up waiting for the lane's other starts: each start now shows its services as queued, with what they wait for, the moment it arrives, and joins the bring-up instead of running its tasks again.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added a boxes view to the TUI (`B`) that connects, resumes and disconnects boxes and asks ssh's prompts and the pairing code there.
  
  Connected boxes' projects get tabs after the local ones (`acme @hetzner`), read-only, and a box unit's pane shows its ports, mirror state and the URLs that open it here. The view also lists this machine's clients: `x` revokes one after asking and `p` shows a pairing code for another machine. `devlanes remote ls --json` entries also carry the box's `registry`, each port's `state` and each route's `url`.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed `devlanes remote connect` with several boxes serving the same hostname: the box connected first keeps it, and the other box's lane is skipped with a warning naming that box.
  
  A port another box's mirror holds is reported with that box and unit, and boxes whose `ports.range` overlap get a warning.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added the URLs connected clients open to `devlanes url print` on a box: in a terminal it prints `on <client>: <url>` for each machine connected with `devlanes remote connect`, or why the unit does not open there, and `--json` lists them under `clients`.
  
  `devlanes url ls` counts the hostnames each client reaches, and `devlanes status --json` lists each connected client with a stable `id`, its host `name` and what it mirrors (`clients[].mirror`).
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `ports[].last_error` to `devlanes remote ls` and an `open_failed` warning for connections to a mirrored port that fail on the box, such as a unit that is not running there or a port nothing listens on.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `remote.boxes.<name>.command` to the user config, so `devlanes remote connect <name>` reaches a container, pod, WSL distribution or codespace by running a command such as `docker exec` instead of SSH.
  
  ```yaml
  remote:
    boxes:
      acme:
        command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
  ```
  
  See `docs/guides/remote-command.md`.

## 0.6.1 — 2026-10-27

### Patch changes

- [2a26145](https://github.com/devlanes-run/devlanes/commit/2a26145f9557581e398d512bc8d4f00fad683256) Added a `devlanes doctor` warning when another program listens on the proxy's HTTP or HTTPS port, which on Windows happens with a local reverse proxy on 443 or 80 that then stops getting requests to `localhost`.

## 0.6.0 — 2026-10-26

### Minor changes

- [8015a60](https://github.com/devlanes-run/devlanes/commit/8015a60fe48fae958e0fd0c22e0aa260c7c3acbc) Added idle lanes: after `lanes.freeze_after` (default 15m) without activity, devlanes freezes a lane's processes and its own containers, and the next request, connection or command resumes them.
  
  On macOS and Windows, `lanes.throttle_after` (default 5m) first runs an idle lane at background priority. Set `services.<name>.keep_awake: true` on workers that must keep running. `0s` turns either setting off. See `docs/guides/lifecycle.md`.

### Patch changes

- [ccb763b](https://github.com/devlanes-run/devlanes/commit/ccb763b1808ffaa61af47dd781c70cca0644aa88) Changed `devlanes status` and `devlanes lane ls` to say when nothing is running, and for a single idle lane to print the `devlanes start --lane` command that starts it.
- [a7472f6](https://github.com/devlanes-run/devlanes/commit/a7472f6affae0e0d4e86ccfd7a26bb614a6c40b0) Fixed the first `devlanes` command on a busy machine, or one run while another started the daemon, failing with "daemon is unreachable" while the daemon was still starting.
  
  The command now waits as long as the daemon makes progress. When one startup step hangs for 30 seconds it fails with "the daemon has not finished starting", names the step and hints `devlanes daemon logs`.

## 0.5.0 — 2026-10-26

### Minor changes

- [edc8274](https://github.com/devlanes-run/devlanes/commit/edc8274b1423f7370cbaaae258f617037857edf3) Changed a plain `devlanes start` (and `s` on a lane in the TUI) to bring back the services the lane ran before, after a reboot, a daemon crash or `daemon stop`, including ones started by name.
  
  `devlanes stop` without units now also forgets the lane's profiles, so the next plain start uses the `default` profile. A plain start names the other lanes that ran so `devlanes lane resume` can bring them back, and `devlanes with` no longer changes what a lane brings back. Set `start.restore: false` in `~/.devlanes/config.yaml` to always start the `default` profile.

### Patch changes

- [6b60828](https://github.com/devlanes-run/devlanes/commit/6b608289d00c02d7f6181fad56def7e8546e3977) Added `devlanes config get`, `set` and `unset` to read and change the user config.yaml.
- [488f5be](https://github.com/devlanes-run/devlanes/commit/488f5bee3ef717386e6ea4ece47870af144ce613) Changed http readiness timeouts to name the request that failed, such as `GET /-/health (Host: 127.0.0.1:20008) answered 404`, and to suggest `ready.http.host` alongside `path` and `status`.

## 0.4.2 — 2026-10-26

### Patch changes

- [9a6e93f](https://github.com/devlanes-run/devlanes/commit/9a6e93f88aa5ab73e833c4d16a7e5a654d448a88) Changed the stop timeout warning in `devlanes doctor` and the TUI's daemon page to a shorter line, with its units listed on a separate line.
- [b3a2ee0](https://github.com/devlanes-run/devlanes/commit/b3a2ee03f66aa8c7515008f37031468fbc92c2ae) Added `devlanes infra logs <container> [-f]` and Docker logs in the TUI pane when an infra container is selected, read from Docker and followed across container restarts.

## 0.4.1 — 2026-10-26

### Patch changes

- [181698e](https://github.com/devlanes-run/devlanes/commit/181698ebbe25612c1149bb1c157857ff6acadf85) Changed `daemon status` and `doctor` on Windows to label the daemon's named pipe as `pipe` instead of `socket`.
- [181698e](https://github.com/devlanes-run/devlanes/commit/181698ebbe25612c1149bb1c157857ff6acadf85) Added `devlanes daemon start`, which starts the daemon in the background unless it already runs and prints its status.

## 0.4.0 — 2026-10-25

### Minor changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Removed the fnox integration: the `fnox` config key and `devlanes secrets unlock`.
  
  Configs with a `fnox:` key fail to load. Run the tool in the command instead, for example `fnox exec -- npm run dev`. `${{ env.X }}` now reads the client environment, then `.env`.

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added `devlanes env export --shell pwsh` for Windows PowerShell 5.1 and PowerShell 7. The default, `posix`, is unchanged.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Changed `devlanes with` to skip the peak table when it is empty, and on an interactive terminal; CI output, pipes and the step summary still get it.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Fixed `devlanes stop --logs-dir` printing output with `--quiet`.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added the `infra.hosts` URLs of a container to `devlanes status` and the TUI container pane.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added a prompt to `devlanes lane new` that asks whether to start the new lane when run without `--start` on a terminal.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added WSL support: devlanes detects WSL and adapts to it.
  
  `devlanes url open` and `o` in the TUI open URLs in the Windows browser. A new `windows` target for `tls.trust` adds the CA to the Windows user store. Starting in a worktree under `/mnt/<drive>` warns, `doctor` gets a `wsl` group, and `daemon install` without systemd fails with the `wsl.conf` fix. See `docs/guides/wsl.md`.

