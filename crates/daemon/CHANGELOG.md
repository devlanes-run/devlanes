# devlanes-daemon

## 0.8.0 — 2026-10-28

### Minor changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key; devlanes no longer creates worktrees.
  
  Create one with `git worktree add <path> <branch>` and run `devlanes start` in it. Configs that still set `lanes.worktree_dir` fail to load until it is removed. `devlanes lane new --name` still adds another lane for the current folder.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Changed projects to be identified by their `project:` name, so one repository can hold several projects: a `devlanes.yaml` in a subdirectory, or another file such as `devlanes.prod.yaml` named once with the new `--file` flag and then with `-p`/`--project` (which also reads `DEVLANES_PROJECT`).
  
  Renaming `project:` now makes a new project; the old one's lanes stay until `devlanes lane gc --yes` cleans them. Another repository can no longer use a name already taken. In a project file below the worktree root, relative `cwd`, `run_script.file` and `sync` paths now start at that file's directory. `lanes.max` comes from the primary worktree's file, and a worktree whose file sets it, `lanes.primary` or shared infra differently gets a warning. Commands run inside a process devlanes started now default to that lane's project; pass `-p` to choose another. See [several projects in one repository](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/services.md).

### Patch changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added `devlanes reload`, which applies project file and `.env` edits to a running lane: it stops removed services, restarts changed ones and starts added ones.
  
  `devlanes status` and the TUI show what each edit changed, `r` on a changed lane in the TUI reloads it, and `devlanes restart` without services restarts the whole lane instead of asking which to restart.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Fixed edits to the project file of a running lane: an invalid or deleted file keeps its services on the proxy, saved profiles and services the file dropped are skipped with a warning, edits to the primary worktree's `devlanes.local.yaml` and to `.env` are detected, edited `infra.setup` entries and `lane.create` tasks run again, and `lane gc` keeps port offsets other worktrees still use.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added `devlanes debug config`, which prints a lane's project config as devlanes reads it, after `extends`, local overrides and templates, with the files it came from; `${{ }}` expressions are printed as written.
- [89491d4](https://github.com/devlanes-run/devlanes/commit/89491d43c487349a863dc38c7766998bb6179903) Fixed colored lines from Windows services showing an empty row under each in the TUI, and `devlanes lane clean` leaving the lane's logs behind for the next lane given its number.

## 0.7.1 — 2026-10-28

### Patch changes

- [ab1c65e](https://github.com/devlanes-run/devlanes/commit/ab1c65edc4924def207485353865d0a551bb10ec) Fixed CI checkouts of a pull request or a non-primary branch getting lane 1: when a CI variable such as `CI` or `GITHUB_ACTIONS` is set, the checkout now takes lane 0 when it is free (`acme@0` on `acme.localhost`).
- [ab1c65e](https://github.com/devlanes-run/devlanes/commit/ab1c65edc4924def207485353865d0a551bb10ec) Changed the summary of `devlanes with` and `devlanes stop --logs-dir`: it now names the lane (and for `with`, the exit code), lists each unit's state, run time and average and peak CPU and memory, and the GitHub step summary adds the last log lines of each failed unit in a collapsed block.

## 0.7.0 — 2026-10-28

### Minor changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed `devlanes remote connect` to reconnect boxes after a daemon restart until `devlanes remote disconnect`, and to wait for another `devlanes remote connect` instead of failing when a reconnect needs a password or new credentials.
  
  While a box waits, `devlanes remote ls` and `devlanes status --json` show `needs_auth` with the command to run, and its hostnames show a sign-in page (DL205). Boxes connected before this release are not reconnected on their own; run `devlanes remote connect <box>` once.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Removed `devlanes remote connect --install`; a box without devlanes now fails with `devlanes is not installed on <box>` and a link to the remote setup guide.
  
  Install devlanes on the box yourself, the same release as on this machine, with `install.sh` (`install.ps1` on Windows), Homebrew, npm or cargo binstall, so that it is on the `PATH` of a non-interactive SSH session or in `~/.devlanes/bin` (`install.sh --dir ~/.devlanes/bin`; `%USERPROFILE%\.devlanes\bin` on Windows). See `docs/guides/remote-setup.md`.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed the remote protocol to version 4, so `devlanes remote connect` works with boxes whose login shell or profile prints to stdout; the client and the box both need this release.
  
  To upgrade, install this release on the box yourself (devlanes no longer installs it there) and on this machine, then run `devlanes daemon restart` on both. Each daemon upgrades its state database on that first start, and earlier releases cannot open it afterwards. Then connect each box once with a pairing code: `devlanes daemon pair` on the box, `devlanes remote connect <box> --code <code>` here.
- [76f6716](https://github.com/devlanes-run/devlanes/commit/76f67163f90f6857092f8e42e44f1cbdc1acaf6d) Changed `devlanes daemon restart` to finish as soon as the new daemon answers, with the services that ran coming back as ordinary starts; once the old daemon was asked to stop, the restart completes even if the terminal or TUI that asked goes away.
  
  `daemon restart --json` now lists the lanes it starts again in `resuming` instead of their start results in `resumed`; use `devlanes status` or `devlanes wait` to follow them.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed `devlanes remote connect` so a box serves only machines it paired with, and added `devlanes daemon pair`, `devlanes daemon clients`, `devlanes daemon revoke`, `remote.require_pairing` and `remote.allow_connections`.
  
  A machine's first connection to a box needs a one-time code: run `devlanes daemon pair` on the box, then `devlanes remote connect <box> --code <code>` here, or enter the code when `devlanes remote connect <box>` asks. Later connections, including those after a daemon restart, need none. To let every machine that gets in over SSH connect without a code, as before, set `remote.require_pairing: false` in the box's user config. To turn every machine away, set `remote.allow_connections: false`. See "Pairing" in `docs/remote.md`.
- [0e02c1b](https://github.com/devlanes-run/devlanes/commit/0e02c1b8e218bb470869e1e98c0631a5586ebf85) Changed how devlanes keeps a unit's output: `devlanes logs` holds each line once, as the unit's terminal showed it, also after a Windows console repaints its window on a resize, and the TUI's unit pane scrolls from the screen into older output and on into the log as one view, without flickering or switching to a separate stored-logs view.
  
  Typing into a unit or zooming its pane keeps its output on screen instead of reopening the stream. Lines a program rewrites in place (progress lines) are logged again with their new text, and output a program draws on the alternate screen (editors, pagers) is not logged.

### Patch changes

- [76f6716](https://github.com/devlanes-run/devlanes/commit/76f67163f90f6857092f8e42e44f1cbdc1acaf6d) Changed `daemon.exit_when_idle` so a daemon that only commands used, with no TUI ever open, also exits once nothing has run for about 5 minutes; before, it kept running until stopped.
- [76f6716](https://github.com/devlanes-run/devlanes/commit/76f67163f90f6857092f8e42e44f1cbdc1acaf6d) Added step-by-step progress to `devlanes daemon stop` and `daemon restart` on a terminal, and the time each shutdown step took to `daemon.log`, with any process that took more than 2 seconds to stop.
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
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed `devlanes remote connect` mirroring more than it should: boxes no longer send the names of `hidden` units or sync a hidden service's `sync` folders, a port a local program holds on one loopback address (on Windows, also the wildcard address) is no longer mirrored, and a box can no longer claim `localhost`, `*.localhost` or names under a local wildcard.
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Fixed `devlanes start` of services on a lane that runs or is being brought up waiting for the lane's other starts: each start now shows its services as queued, with what they wait for, the moment it arrives, and joins the bring-up instead of running its tasks again.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added a boxes view to the TUI (`B`) that connects, resumes and disconnects boxes and asks ssh's prompts and the pairing code there.
  
  Connected boxes' projects get tabs after the local ones (`acme @hetzner`), read-only, and a box unit's pane shows its ports, mirror state and the URLs that open it here. The view also lists this machine's clients: `x` revokes one after asking and `p` shows a pairing code for another machine. `devlanes remote ls --json` entries also carry the box's `registry`, each port's `state` and each route's `url`.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed `devlanes remote connect` with several boxes serving the same hostname: the box connected first keeps it, and the other box's lane is skipped with a warning naming that box.
  
  A port another box's mirror holds is reported with that box and unit, and boxes whose `ports.range` overlap get a warning.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added Windows boxes for `devlanes remote connect`: a Windows machine with Windows' OpenSSH Server and devlanes installed can be a box, with cmd.exe, PowerShell or Git Bash as its SSH shell.
  
  See "Windows boxes" in `docs/guides/remote-setup.md`.
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Added timings for every start step to `-v` output, with repeated steps added up into one row, and a `start finished` line per start in the daemon log that names its slowest phases and start slot waits.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added the URLs connected clients open to `devlanes url print` on a box: in a terminal it prints `on <client>: <url>` for each machine connected with `devlanes remote connect`, or why the unit does not open there, and `--json` lists them under `clients`.
  
  `devlanes url ls` counts the hostnames each client reaches, and `devlanes status --json` lists each connected client with a stable `id`, its host `name` and what it mirrors (`clients[].mirror`).
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Changed `devlanes start` to leave Compose services that already run healthy with unchanged config alone instead of running `docker compose up` again, and to reuse `docker compose config` until a compose or env file changes. After `docker compose pull`, a running container keeps its old image until it is recreated.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `ports[].last_error` to `devlanes remote ls` and an `open_failed` warning for connections to a mirrored port that fail on the box, such as a unit that is not running there or a port nothing listens on.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `remote.boxes.<name>.command` to the user config, so `devlanes remote connect <name>` reaches a container, pod, WSL distribution or codespace by running a command such as `docker exec` instead of SSH.
  
  ```yaml
  remote:
    boxes:
      acme:
        command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
  ```
  
  See `docs/guides/remote-command.md`.
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Changed the lane's Wrangler dev registry (`WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH` and `MINIFLARE_REGISTRY_PATH`) to be set for every process of a lane, so workers started by `wrangler dev`, a script, a task or the Cloudflare Vite or a Next plugin use the lane's registry even when the shell exports one, and starts no longer scan the worktree for a Wrangler config. `platform: none` now skips platform detection without a `config check` warning.

## 0.6.1 — 2026-10-27

### Patch changes

- [2a26145](https://github.com/devlanes-run/devlanes/commit/2a26145f9557581e398d512bc8d4f00fad683256) Added a `devlanes doctor` warning when another program listens on the proxy's HTTP or HTTPS port, which on Windows happens with a local reverse proxy on 443 or 80 that then stops getting requests to `localhost`.

## 0.6.0 — 2026-10-26

### Minor changes

- [8015a60](https://github.com/devlanes-run/devlanes/commit/8015a60fe48fae958e0fd0c22e0aa260c7c3acbc) Added idle lanes: after `lanes.freeze_after` (default 15m) without activity, devlanes freezes a lane's processes and its own containers, and the next request, connection or command resumes them.
  
  On macOS and Windows, `lanes.throttle_after` (default 5m) first runs an idle lane at background priority. Set `services.<name>.keep_awake: true` on workers that must keep running. `0s` turns either setting off. See `docs/guides/lifecycle.md`.

### Patch changes

- [94453b9](https://github.com/devlanes-run/devlanes/commit/94453b9363165ffe038e4e55ef40287ba4e35e1e) Fixed the monitor (`M`) erasing the graph history of units, lanes and containers once they stopped or were cleaned.
  
  They stay in the table as stopped until their samples leave the graph. A stopped lane no longer counts toward the monitor's current CPU and memory, and `status` no longer shows the last reading of a unit that failed.
- [a7472f6](https://github.com/devlanes-run/devlanes/commit/a7472f6affae0e0d4e86ccfd7a26bb614a6c40b0) Fixed the first `devlanes` command on a busy machine, or one run while another started the daemon, failing with "daemon is unreachable" while the daemon was still starting.
  
  The command now waits as long as the daemon makes progress. When one startup step hangs for 30 seconds it fails with "the daemon has not finished starting", names the step and hints `devlanes daemon logs`.

## 0.5.0 — 2026-10-26

### Minor changes

- [edc8274](https://github.com/devlanes-run/devlanes/commit/edc8274b1423f7370cbaaae258f617037857edf3) Changed a plain `devlanes start` (and `s` on a lane in the TUI) to bring back the services the lane ran before, after a reboot, a daemon crash or `daemon stop`, including ones started by name.
  
  `devlanes stop` without units now also forgets the lane's profiles, so the next plain start uses the `default` profile. A plain start names the other lanes that ran so `devlanes lane resume` can bring them back, and `devlanes with` no longer changes what a lane brings back. Set `start.restore: false` in `~/.devlanes/config.yaml` to always start the `default` profile.

### Patch changes

- [488f5be](https://github.com/devlanes-run/devlanes/commit/488f5bee3ef717386e6ea4ece47870af144ce613) Changed http readiness timeouts to name the request that failed, such as `GET /-/health (Host: 127.0.0.1:20008) answered 404`, and to suggest `ready.http.host` alongside `path` and `status`.
- [da2f753](https://github.com/devlanes-run/devlanes/commit/da2f7531a3069c65cb1de8a54a4c0542198f6ed9) Fixed `infra.setup` entries with a `container` running again at the next start after `devlanes run infra.setup.<name>`, a TUI run of the entry or a task step ran them, even when the container had not restarted.
  
  Starting a service in a running lane is also faster: devlanes plans only the services it starts and checks every setup entry's container with one Docker Compose lookup.
- [488f5be](https://github.com/devlanes-run/devlanes/commit/488f5bee3ef717386e6ea4ece47870af144ce613) Added `ready.http.host` to set the Host header an http readiness check sends, for apps that answer their health route only on their own hostname, such as `host: ${{ self.hosts[0] }}`.

## 0.4.4 — 2026-10-26

### Patch changes

- [b3a2ee0](https://github.com/devlanes-run/devlanes/commit/b3a2ee03f66aa8c7515008f37031468fbc92c2ae) Added `devlanes run infra.setup.<name>` and `devlanes run infra.teardown.<name>` to run one setup or teardown entry. In the TUI, `s` and `r` on a setup step now run that entry instead of failing with "no lane or unit named setup.<name>" (a teardown step asks first), and steps are listed by their entry name, such as `databases` under `setup`.
- [057966d](https://github.com/devlanes-run/devlanes/commit/057966d225932708635cc616c5e38afd80dce095) Added the daemon's own CPU and memory to the TUI: a row pinned under the monitor's table (`M`) with its graphs, and a `usage` line on the daemon page (`D`).
- [057966d](https://github.com/devlanes-run/devlanes/commit/057966d225932708635cc616c5e38afd80dce095) Changed the memory graph on the TUI monitor (`M`) to scale to the host's memory, draw the machine's used memory as a line and color only the seconds that were under memory pressure.
- [b3a2ee0](https://github.com/devlanes-run/devlanes/commit/b3a2ee03f66aa8c7515008f37031468fbc92c2ae) Added `devlanes infra logs <container> [-f]` and Docker logs in the TUI pane when an infra container is selected, read from Docker and followed across container restarts.
- [b3a2ee0](https://github.com/devlanes-run/devlanes/commit/b3a2ee03f66aa8c7515008f37031468fbc92c2ae) Changed the daemon log to record failed start, stop, restart, run and other requests that change lanes, with the error code and data.

## 0.4.3 — 2026-10-26

Version bump only, to follow other packages.

## 0.4.2 — 2026-10-26

### Patch changes

- [f81274b](https://github.com/devlanes-run/devlanes/commit/f81274b9c8094c87886180d81da6ed5a88187ec0) Changed the proxy's error pages: each names an error code documented at devlanes.run/docs/errors, lists commands to diagnose it, and comes as Markdown to clients that do not ask for HTML. Requests to a starting service or a reconnecting box now wait for it, refused connections are retried for a few seconds, and forwarded responses carry `Via: 1.1 devlanes.run` unless `proxy.via: false`.

## 0.4.1 — 2026-10-26

### Patch changes

- [e7fccbd](https://github.com/devlanes-run/devlanes/commit/e7fccbd548210a7d6a802badcaad39c8c8d47043) Changed the daemon log entry for a service that could not start to a warning that names the cause without listing every PATH directory; the service's own log still lists them.

## 0.4.0 — 2026-10-25

### Minor changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Removed the fnox integration: the `fnox` config key and `devlanes secrets unlock`.
  
  Configs with a `fnox:` key fail to load. Run the tool in the command instead, for example `fnox exec -- npm run dev`. `${{ env.X }}` now reads the client environment, then `.env`.

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Fixed a freshly started daemon exiting when the client that spawned it stopped waiting for it.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added native support for Windows 11 24H2 or later, on x64 and arm64.
  
  Install with `irm https://devlanes.run/install.ps1 | iex`. `*.localhost` resolves without a hosts file, `daemon install` starts the daemon at logon, and `certs trust --resolver` writes an NRPT rule for custom domains. See `docs/guides/windows.md`.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Changed TUI lane chips to show the lane's branch, with `@N` only when two lanes share a name. An extra lane on the same folder shows its own name.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Fixed daemon startup waiting for a sleeping Docker engine (for example Docker Desktop's Resource Saver) to wake up.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added an idle exit for daemons that a client started on demand, and the `daemon.exit_when_idle` user config key to turn it off.
  
  Such a daemon now exits about 30 seconds after the last TUI left, when nothing ran or was starting for that whole time. Containers keep running. Daemons installed with `devlanes daemon install` never exit when idle. Set `daemon.exit_when_idle: false` to keep the old behavior.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Fixed remote sync overwriting files on case-insensitive folders when the box has names that differ only by case. Those names are skipped and listed as sync issues.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added a per-core CPU view to the TUI monitor. `m` puts memory in the large graph box and CPU in a small box with a square per core, and `c` switches back.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added `devlanes remote connect` on Windows, using the OpenSSH client in System32 or on `PATH`. Boxes stay Linux or macOS.
  
  Passphrase prompts are relayed to the terminal. Names Windows can't hold are skipped and listed as sync issues. Running the OpenSSH Authentication Agent service avoids a prompt on every reconnect. See `docs/guides/remote-setup.md`.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added WSL support: devlanes detects WSL and adapts to it.
  
  `devlanes url open` and `o` in the TUI open URLs in the Windows browser. A new `windows` target for `tls.trust` adds the CA to the Windows user store. Starting in a worktree under `/mnt/<drive>` warns, `doctor` gets a `wsl` group, and `daemon install` without systemd fails with the `wsl.conf` fix. See `docs/guides/wsl.md`.

