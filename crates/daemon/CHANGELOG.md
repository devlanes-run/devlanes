# devlanes-daemon

## 0.12.0 — 2026-11-07

### Minor changes

- [a0aca99](https://github.com/devlanes-run/devlanes/commit/a0aca991de5bac6b10fff605302f1df06858055e) Changed `devlanes stop --mine` without units to stop only your units in the current project.
  
  Use `-p PROJECT` for another project, or run it outside any project to cover every project. The output names the command when your units still run elsewhere, and `status --mine` lists the same units.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Changed commands run in a folder shared by several projects, such as the repository root, to ask for `-p`.
  
  They used to act on the first registered project. Pass `-p <project>` or run them in a project's folder; `status` and `lane ls` there list every project, and a full lane reference such as `--lane acme@1` still works.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Changed `devlanes wait` on an exec unit to wait until its command exits.
  
  A server started with `devlanes exec` never exits, so `wait` on it now times out with exit 124; read `devlanes logs NAME` or open its URL instead.
  `wait` on a task or exec unit that exited 0 now exits 0 instead of 1, and a timeout while it still runs says so.
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Changed `devlanes with --profile` to exit 1 when the lane's running services lack the profile's settings.
  
  Stop the lane first, or run `with` in another worktree. `devlanes start --profile` on such a lane now warns about those services, and `devlanes status` shows them as changed until `devlanes reload`.

### Patch changes

- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes status` and the TUI showing a lane as `running` while its services are still starting or queued.
- [a0aca99](https://github.com/devlanes-run/devlanes/commit/a0aca991de5bac6b10fff605302f1df06858055e) Fixed `devlanes env show --explain` printing the values of secret-looking variables in full.
- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes lane clean --dry-run`, which lists what a clean would stop, run and delete, with sizes.
- [1c7b3d9](https://github.com/devlanes-run/devlanes/commit/1c7b3d986454893e4f6a4d96fb423645e1f5c4f6) Changed `devlanes stop`, `start` and `restart` to suggest `--lane` when given a lane number or `lane new` name.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Changed `devlanes logs` on a task with only `steps` to show the logs of its steps.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes stop`, `restart` and `logs` rejecting the name of a service with `replicas` instead of selecting every replica.
- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Fixed boxes showing no name for a Windows client in `devlanes daemon clients` and `daemon revoke`.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes debug why` blaming the profile for a service started by name and missing a service on the wrong port.
- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes mcp`, an MCP server that lets AI agents run devlanes commands as tools.
  
  Register it with your agent, such as `claude mcp add devlanes -- devlanes mcp` for Claude Code. See [coding agents](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/mcp.md) for Codex, OpenCode, Cursor, VS Code, Claude Desktop and Goose.
  
  <!-- highlight
  title: Use devlanes from your agent over MCP
  body: Run `claude mcp add devlanes -- devlanes mcp` (or see `devlanes mcp --help` for other agents) and the agent starts dev servers with `exec`, reads logs and stops what it started, asking you before it deletes lane data.
  -->
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes debug errors` missing the exit of a service that had failed its ready check.
- [3c82602](https://github.com/devlanes-run/devlanes/commit/3c82602efaf1333d255dd02c3c647c18f67ade82) Fixed `devlanes start` and `with` naming another project's lane re-adding the current folder's project after `lane clean`.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Fixed `-p` commands such as `devlanes -p acme logs web` failing outside that project's folders.
  
  Outside them, `start`, `stop`, `restart`, `logs` and other one-lane commands act on the project's main lane (lane 0). `lane clean`, `lane new`, `run` and `exec` ask for `--lane`, which `lane new` now takes as the lane whose folder it adds to.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Fixed a whole-lane `devlanes reload` or `devlanes restart` stopping exec units without saying so.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `ready.log` patterns that start with `^` never matching on Windows services with `pty: true`.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed `devlanes run` hiding the output of `infra.setup` and `infra.teardown` steps.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed `devlanes start` rerunning a `container:` setup entry before its restarted container is healthy.
  
  The start result now lists the setup entries and tasks it ran instead of saying nothing started.
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Fixed `devlanes reload` not starting services that were removed from the project file and added back.
  
  `reload` no longer restarts the whole lane for an `infra.external`, `infra.hosts` or `infra.hidden` edit. `devlanes status` drops removed services once they stop, says a unit `changed since it started` instead of naming the project file, and no longer keeps a stale `config changed` note on a lane where nothing runs.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed services, routes and `devlanes status` keeping an infra container's old port after it restarted.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed a service that could not start showing as idle instead of failed.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Changed a plain `devlanes start` to name only its own project's lanes waiting for `lane resume`.
  
  It counts the other projects' waiting lanes, which `lane resume` also starts.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed `devlanes with`, `devlanes -v start` and CI logs showing infra and setup steps for a project without them.
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes status --mine` showing lane URLs without `https://` and the proxy port.
  
  `status --json` lanes now carry that URL as `url`.
- [1c7b3d9](https://github.com/devlanes-run/devlanes/commit/1c7b3d986454893e4f6a4d96fb423645e1f5c4f6) Fixed `devlanes lane new --start` ignoring this shell's environment, and allowed `--env` with it.
- [f750a6c](https://github.com/devlanes-run/devlanes/commit/f750a6c559639fc6725f44e7e39aa0ba8c8a608e) Fixed `devlanes env show --explain` not printing launch notes, such as the detected package manager.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Changed `devlanes stop <name>` to clear the failed state of a service or `exec` command.
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Fixed `devlanes restart <unit>` keeping an `--env` value from an earlier command.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Changed `devlanes url ls`, `debug top` and `status --watch` to cover only the current project, or the one `-p` names.
  
  Outside every project they cover all of them. `status --watch` lines count each lane's units by state, and agent output prints them.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `run` lists failing to start when an item is one `${{ }}` expression that gives a number, such as `self.port`.
- [1c7b3d9](https://github.com/devlanes-run/devlanes/commit/1c7b3d986454893e4f6a4d96fb423645e1f5c4f6) Fixed `devlanes lane new --name` refusing a cleaned lane's name.
  
  The name now gives a new lane with empty data. A taken name gets its own error, `main` and `lane<N>` are refused,
  and `lane new` prints the project folder instead of the repository root. `lane new` and `lane clean` name a lane data
  folder they could not delete.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes debug freeze` refusals naming the wrong cause for a running exec unit and wrong keys for replicas.
- [f750a6c](https://github.com/devlanes-run/devlanes/commit/f750a6c559639fc6725f44e7e39aa0ba8c8a608e) Fixed `devlanes remote ls` suggesting `devlanes daemon install` for a box with linger off.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Fixed `devlanes exec` commands missing `global_env` and CA variables such as `NODE_EXTRA_CA_CERTS`.
  
  `devlanes env show` without a unit and the command `devlanes with` runs now include `global_env` and the profile's `env` too, except entries that read `self` or `replica`. Until the lane's infra is up, entries that read `infra.*` are left out and named.
  `devlanes exec` now refuses while another of those values does not evaluate (a `${{ env.X }}` the shell does not set, for example), or while the project file does not load and nothing in the lane runs.

## 0.11.3 — 2026-11-06

### Patch changes

- [91c4f2b](https://github.com/devlanes-run/devlanes/commit/91c4f2b60a70695afc248f0b045a499292336a43) Added a `devlanes doctor` warning when Windows refuses to let devlanes start programs.

## 0.11.2 — 2026-11-02

### Patch changes

- [fef7c86](https://github.com/devlanes-run/devlanes/commit/fef7c863cf1a266174c8bed86a23767e0a30dee4) Changed `daemon.exit_when_idle` to stop the daemon about 15 seconds after the last TUI closes instead of 30.
  
  The summary printed when the TUI closes now says `stopping in 15s` instead of `running` when nothing keeps the daemon up.
- [36c4aef](https://github.com/devlanes-run/devlanes/commit/36c4aefc33e33d3e7af468e65af7d27762ad866f) Changed a deleted worktree's lane to keep its data and ports until `devlanes lane gc --yes` cleans it.
  
  Set `lanes.removed_worktrees: clean` to have the daemon clean these lanes by itself, as before.
- [e5602c9](https://github.com/devlanes-run/devlanes/commit/e5602c98d0084a6277dd39ba8277b581afeff653) Fixed `devlanes daemon stop` and `daemon restart` sometimes leaving out their last steps.

## 0.11.1 — 2026-11-01

Version bump only, to follow other packages.

## 0.11.0 — 2026-11-01

### Minor changes

- [9d7557c](https://github.com/devlanes-run/devlanes/commit/9d7557cf5829274623142c4393958b76b536d5a2) Changed lanes to export their project as `DEVLANES_LANE_PROJECT` instead of `DEVLANES_PROJECT`, so a lane's variables no longer pin devlanes commands in another project's folder or checkout to that project.
  
  `DEVLANES_PROJECT` is now only a setting you export yourself, and it wins over the current folder like `-p`. Scripts that read the lane's project from its environment use `DEVLANES_LANE_PROJECT`.

### Patch changes

- [545075f](https://github.com/devlanes-run/devlanes/commit/545075fff72aa6c314a8edd8fc3a3d86cd25eb1c) Fixed `devlanes remote connect` sometimes leaving a box's lanes neither mirrored nor listed as skipped until something on the box changed.
- [9d7557c](https://github.com/devlanes-run/devlanes/commit/9d7557cf5829274623142c4393958b76b536d5a2) Fixed `devlanes debug thaw` sometimes saying a frozen lane was not throttled or frozen after it had thawed it.

## 0.10.0 — 2026-11-01

### Minor changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Removed `devlanes certs trust` and `certs untrust`; use `devlanes setup`, which works out what this machine needs and asks once.
  
  Run `devlanes setup` instead of `certs trust --hosts --resolver`, and `devlanes setup --remove` instead of `certs untrust --hosts --resolver`.
  The TUI offers to run it when HTTPS or hostnames need it.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Removed `updates.check`; use `updates.mode: disable | notify | auto` in the user config.
  
  Replace `updates.check: false` with `updates.mode: disable`; a config that still sets `updates.check` fails to load until it is changed.

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed every devlanes update asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.
  
  `devlanes doctor` and the hosts status now report an old helper as `helper_revision_outdated` instead of `helper_version_mismatch`.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Changed release checks to run every 4 hours and when the TUI opens, reading the version from the registry devlanes was installed from.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added each box's and each connected client's devlanes version to the TUI's Boxes view, `devlanes remote ls` and `devlanes daemon clients`.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added background installs of patch releases with `updates.mode: auto`; devlanes switches to them the next time you open it.

## 0.9.0 — 2026-10-29

### Minor changes

- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane that several boxes run.
  
  A switch moves the lane's hostnames and ports to the other box at once and closes open connections to the old box. The same lane on two boxes is now mirrored from one of them, even when their hostnames differ. To work with both at once, run the lane under another lane number or project on one box. In the TUI, box lanes now join the lanes row of the local project with the same name, and lanes this machine does not mirror are grey. See [several boxes with the same project](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/remote-multiple-boxes.md).
- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Changed default lane domains to `<project><N>.localhost`, such as `acme2.localhost` instead of `acme-2.localhost`.
  
  Lane 0 keeps `acme.localhost`, and the `resolver` strategy gives `acme2.test`. A project that sets `lanes.vars.domain` keeps its own. Replace `acme-2.localhost` wherever you wrote it, such as `.env` files and OAuth redirect URIs. Lane 2 of `acme` and lane 0 of a project named `acme2` now share `acme2.localhost`, and devlanes warns about it with `host_conflict`.

## 0.8.1 — 2026-10-28

Version bump only, to follow other packages.

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

