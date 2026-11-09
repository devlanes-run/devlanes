# devlanes-cli

## 0.13.0 — 2026-11-09

### Minor changes

- [0e2e4a8f](https://github.com/devlanes-run/devlanes/commit/0e2e4a8f551687caa5a86985cf7d1db333962cd6) Changed `devlanes lane new --start --output json` to print the new lane, then the start as `devlanes start --json` does.
  
  Read the start's result from the last line instead of the `start` key. `--timeout` now applies to that start. A devlanes 0.12 client gets an error for `lane new --start` from this daemon; run `devlanes lane new`, then `devlanes start --lane <name>`.
- [174ed796](https://github.com/devlanes-run/devlanes/commit/174ed79624c5188c6c4a94924d050d9e31641234) Changed `devlanes env show` in agent output and the MCP `env_show` tool to hide secret-looking values.
  
  `devlanes mcp` no longer runs `env export`. In a shell, `env show --output plain` and `eval "$(devlanes env export)"` still print real values, and `exec` or `with` pass them to a command. A `--read-only` MCP server can no longer read secret values.
- [087f74bb](https://github.com/devlanes-run/devlanes/commit/087f74bb4a2cfd4da7f7333eb7434172531a61dd) Changed `devlanes with --profile` to refuse a running lane whose profiles give its services other settings, before starting anything.
  
  Stop the lane, run `with` in another worktree, or name the services instead of the profile. A profile that only picks services works on a running lane whose profiles set no `env`, `run_scripts` or `infra`.

### Patch changes

- [a09fef90](https://github.com/devlanes-run/devlanes/commit/a09fef900b3f5e765ca47cb4420d932124c6a221) Fixed `devlanes with --logs-dir` leaving out the logs of `infra.setup` entries and failed tasks the start ran.
  
  `ran` in the `start --json` result now also lists the tasks and setup entries that failed.
- [5ed93a92](https://github.com/devlanes-run/devlanes/commit/5ed93a9209ff9afe7e2c283d346eb8e8bec49fc7) Fixed `devlanes mcp` reporting another lane's start as the progress of a start call.
  
  Calls of `with` and `lane new --start` now report the phases of the lane they start too.
- [4158a194](https://github.com/devlanes-run/devlanes/commit/4158a1943384987241920e909beed7bb111b584d) Fixed `devlanes with` clearing the `--env` values of the lane's running services.
- [ed0c90d3](https://github.com/devlanes-run/devlanes/commit/ed0c90d3b5d7034af9ff0e5b01655a361a879d27) Fixed `devlanes start --json` leaving out the `lane.phase` events of phases as they start and progress.
- [94511d28](https://github.com/devlanes-run/devlanes/commit/94511d280c3a565085c90e60b3bbfbffe938d359) Fixed `devlanes remote use` right after `devlanes remote connect` saying the lane is chosen but not mirrored yet.
  
  A choice that applies later, once its box connects or its lane is mirrored, now sends the same `lane_switched` notice as one that applies at once, and `remote use` and the TUI say which of the two it waits for. Choosing a box lane with nothing to mirror now says so, instead of pointing to `remote ls`.
- [dd50a481](https://github.com/devlanes-run/devlanes/commit/dd50a48127f7f59693f9e66eae32c53c86b26f40) Fixed errors and `devlanes lane ls` showing a lane that waits for a free index as `unknown` or `–`.
- [a09fef90](https://github.com/devlanes-run/devlanes/commit/a09fef900b3f5e765ca47cb4420d932124c6a221) Fixed `devlanes start` on a terminal replacing a running task's line with the line of one of its finished steps.
- [4e8e5636](https://github.com/devlanes-run/devlanes/commit/4e8e56369ab3a49b4bbd993b7af345a10abcbcad) Changed `devlanes lane clean` so a lane that comes back starts empty, without the old `--env` or history.
  
  The primary and worktree lanes get a new data folder path, and `lane clean --dry-run` lists the run history. The daemon migrates its database on the next start and deletes the history of lanes cleaned before. `devlanes lane gc` deletes lane data folders a clean could not delete.
- [4e8e5636](https://github.com/devlanes-run/devlanes/commit/4e8e56369ab3a49b4bbd993b7af345a10abcbcad) Changed `devlanes lane clean` of a project's last lane to stop its shared containers, keeping their data.
  
  The next `devlanes start` in the project brings them up again.
- [36cbe7b4](https://github.com/devlanes-run/devlanes/commit/36cbe7b497f773aac89902cb65a42ecfc8ec28fe) Changed `devlanes status` to report a running exec command, task or infra command as `running`, JSON included.
  
  Scripts and MCP consumers that matched `ready` for exec units or `starting` for running tasks should match `running`. A task stopped by its own `timeout:` now reads `failed` with reason `task_timeout` everywhere. `run` answers, `task` events and the last run in `run --list` use the same words as `status`: `exited`, `failed` or `stopped` (they said `done`, a stopped task `failed` in events and `interrupted` in the list).
- [40afbabe](https://github.com/devlanes-run/devlanes/commit/40afbabe21bbc5431a65a080085d9e023dfbe5ce) Fixed the unit picker and the TUI choosing a `lane new` lane instead of the folder's own lane.
- [a3adedfa](https://github.com/devlanes-run/devlanes/commit/a3adedfab7e0226e781dbfbd0d96cd5d3fd4421a) Changed the error for a config value with no value in the lane, such as an unset `${{ env.X }}`, to stop calling `devlanes.yaml` invalid.
  
  Its JSON code is `unresolved_value` instead of `config_error` or `lane_env_unresolved`, for services, tasks, `exec` and `env show` alike. Older clients show the code and message without the variable's name.
- [4158a194](https://github.com/devlanes-run/devlanes/commit/4158a1943384987241920e909beed7bb111b584d) Fixed `devlanes lane new --start` and `daemon restart` through `devlanes mcp` starting lanes without the login shell's environment.
- [4a4678ec](https://github.com/devlanes-run/devlanes/commit/4a4678ec5ac2014623f664c14b2ac83db7bd96c1) Changed `devlanes exec` and `env show` to fail on a misspelled `infra.<name>`, which `config check` now reports too.
  
  `exec` and `env show` leave out a value only while its container is stopped or Docker is not running, and say which. They refuse when Docker does not answer in time, and with compose files on a machine without the Docker CLI; pass the value with `--env KEY=...` or start Docker. `env export` and `env shell` now include `NODE_EXTRA_CA_CERTS`, `DENO_CERT`, `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` unless the shell sets them.
- [ed0c90d3](https://github.com/devlanes-run/devlanes/commit/ed0c90d3b5d7034af9ff0e5b01655a361a879d27) Fixed `after: services` tasks running when `devlanes start` timed out.
  
  `devlanes start` prints `services ready` once every service it started is ready, and `--json` gets a `lane.phase` event with `state: ready` before `services` `done`, which now comes after the wait. A `devlanes stop` of the lane during that wait ends the start with `start_cancelled`.
- [679d435c](https://github.com/devlanes-run/devlanes/commit/679d435cefcd5ea90bbf431c141aa315dcb9ec2f) Changed `url ls`, `debug top`, `logs` and other reads to leave a project removed by `lane clean` removed.
  
  Only `start`, `restart`, `reload`, `run`, `exec`, `lane new`, `infra`, `env`, `url`, `status` and `lane ls` add the folder's project. Other reads in a project nobody started say so; `stop` and `lane gc` do nothing there. A `devlanes.yaml` that does not load now shows its error in `status` and `url ls`. `config check` warns about hostnames before the first start. A main checkout off the primary branch gets its lane on its first `status` or `start`, as other worktrees do.
- [4158a194](https://github.com/devlanes-run/devlanes/commit/4158a1943384987241920e909beed7bb111b584d) Fixed `devlanes restart <unit>` through `devlanes mcp` keeping an `--env` value from an earlier command.
- [e4cf749b](https://github.com/devlanes-run/devlanes/commit/e4cf749b66c80f3ce28a67fb338ad73e1698a03d) Fixed `devlanes with` leaving units running when Ctrl-C interrupted its start.
  
  `with` needs a daemon of this release: run `devlanes daemon restart` when it says the daemon is outdated.
- [a4108390](https://github.com/devlanes-run/devlanes/commit/a4108390ab5093951c1075317bc93a2000030196) Fixed Windows units ended by a stop showing exit codes such as -1073741510, 255 or 1 instead of a signal, as on macOS and Linux.
  
  just recipes and Git's `bin\bash.exe` now show the code they return, `512` when their shell ended on Ctrl-C.
- [f322ca79](https://github.com/devlanes-run/devlanes/commit/f322ca79aee9a20312c079c4a40709ce871fd32f) Changed `ready.log` to match the lines `devlanes logs` shows, so a line drawn over a spinner or progress text now matches.
  
  Text a program draws only on the alternate screen (full-screen terminal apps) no longer matches, and a prompt or a line still being redrawn matches once the program moves past it or pauses. Match a line the program prints before it switches to full screen, or use a `tcp` or `http` check. Trailing spaces are not part of a logged line, so end a pattern before them: `^password>` or `^password>$` instead of `"^password> "`.

## 0.12.0 — 2026-11-07

### Minor changes

- [a0aca99](https://github.com/devlanes-run/devlanes/commit/a0aca991de5bac6b10fff605302f1df06858055e) Changed `devlanes stop --mine` without units to stop only your units in the current project.
  
  Use `-p PROJECT` for another project, or run it outside any project to cover every project. The output names the command when your units still run elsewhere, and `status --mine` lists the same units.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Changed commands run in a folder shared by several projects, such as the repository root, to ask for `-p`.
  
  They used to act on the first registered project. Pass `-p <project>` or run them in a project's folder; `status` and `lane ls` there list every project, and a full lane reference such as `--lane acme@1` still works.
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Changed `devlanes with --profile` to exit 1 when the lane's running services lack the profile's settings.
  
  Stop the lane first, or run `with` in another worktree. `devlanes start --profile` on such a lane now warns about those services, and `devlanes status` shows them as changed until `devlanes reload`.

### Patch changes

- [a0aca99](https://github.com/devlanes-run/devlanes/commit/a0aca991de5bac6b10fff605302f1df06858055e) Fixed `devlanes stop --mine` and `status --mine` treating every OpenCode session as the same session.
- [a0aca99](https://github.com/devlanes-run/devlanes/commit/a0aca991de5bac6b10fff605302f1df06858055e) Fixed `devlanes env show --explain` printing the values of secret-looking variables in full.
- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes lane clean --dry-run`, which lists what a clean would stop, run and delete, with sizes.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Changed agent and plain output to show a running exec unit as `running`, as human output does.
- [1c7b3d9](https://github.com/devlanes-run/devlanes/commit/1c7b3d986454893e4f6a4d96fb423645e1f5c4f6) Changed `devlanes stop`, `start` and `restart` to suggest `--lane` when given a lane number or `lane new` name.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes stop`, `restart` and `logs` rejecting the name of a service with `replicas` instead of selecting every replica.
- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Fixed stopped units on Windows showing exit codes such as 255 or 512 instead of reading as interrupted.
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes start` printing the results of `after: services` tasks before saying the services were ready.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes debug why` blaming the profile for a service started by name and missing a service on the wrong port.
- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes mcp`, an MCP server that lets AI agents run devlanes commands as tools.
  
  Register it with your agent, such as `claude mcp add devlanes -- devlanes mcp` for Claude Code. See [coding agents](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/mcp.md) for Codex, OpenCode, Cursor, VS Code, Claude Desktop and Goose.
  
  <!-- highlight
  title: Use devlanes from your agent over MCP
  body: Run `claude mcp add devlanes -- devlanes mcp` (or see `devlanes mcp --help` for other agents) and the agent starts dev servers with `exec`, reads logs and stops what it started, asking you before it deletes lane data.
  -->
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes with --logs-dir` leaving out the logs of the tasks its start ran.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Fixed `-p` commands such as `devlanes -p acme logs web` failing outside that project's folders.
  
  Outside them, `start`, `stop`, `restart`, `logs` and other one-lane commands act on the project's main lane (lane 0). `lane clean`, `lane new`, `run` and `exec` ask for `--lane`, which `lane new` now takes as the lane whose folder it adds to.
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Fixed `devlanes env show <unit>` failing on a lane that has not started yet.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Fixed a whole-lane `devlanes reload` or `devlanes restart` stopping exec units without saying so.
- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes status --statusline`, one line for the current lane to show in Claude Code's status line.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed `devlanes start` rerunning a `container:` setup entry before its restarted container is healthy.
  
  The start result now lists the setup entries and tasks it ran instead of saying nothing started.
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Fixed `devlanes reload` not starting services that were removed from the project file and added back.
  
  `reload` no longer restarts the whole lane for an `infra.external`, `infra.hosts` or `infra.hidden` edit. `devlanes status` drops removed services once they stop, says a unit `changed since it started` instead of naming the project file, and no longer keeps a stale `config changed` note on a lane where nothing runs.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Changed a plain `devlanes start` to name only its own project's lanes waiting for `lane resume`.
  
  It counts the other projects' waiting lanes, which `lane resume` also starts.
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes status --mine` showing lane URLs without `https://` and the proxy port.
  
  `status --json` lanes now carry that URL as `url`.
- [1c7b3d9](https://github.com/devlanes-run/devlanes/commit/1c7b3d986454893e4f6a4d96fb423645e1f5c4f6) Fixed `devlanes lane new --start` ignoring this shell's environment, and allowed `--env` with it.
- [f750a6c](https://github.com/devlanes-run/devlanes/commit/f750a6c559639fc6725f44e7e39aa0ba8c8a608e) Fixed `devlanes env show --explain` not printing launch notes, such as the detected package manager.
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes start` reporting `failed` when `--timeout` ends the wait while services are still starting.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Changed `devlanes stop <name>` to clear the failed state of a service or `exec` command.
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes start` and `devlanes with` leaving `… tasks: <task>` lines above finished tasks in a terminal.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Changed `devlanes wait` on an exec unit to wait until its command exits.
  
  A server started with `devlanes exec` never exits, so `wait` on it now times out with exit 124; read `devlanes logs NAME` or open its URL instead.
  `wait` on a task or exec unit that exited 0 now exits 0 instead of 1, and a timeout while it still runs says so.
- [8c98423](https://github.com/devlanes-run/devlanes/commit/8c98423b5737190e5c480826fac0d0d4bc4b84a3) Fixed `devlanes logs --grep` and `--level` printing three extra lines around each match.
  
  `-C N` adds N lines around each match.
- [00d1870](https://github.com/devlanes-run/devlanes/commit/00d1870829ef88ec951452efd6d0fbde4af9069b) Changed `devlanes url ls`, `debug top` and `status --watch` to cover only the current project, or the one `-p` names.
  
  Outside every project they cover all of them. `status --watch` lines count each lane's units by state, and agent output prints them.
- [1c7b3d9](https://github.com/devlanes-run/devlanes/commit/1c7b3d986454893e4f6a4d96fb423645e1f5c4f6) Fixed `devlanes lane new --name` refusing a cleaned lane's name.
  
  The name now gives a new lane with empty data. A taken name gets its own error, `main` and `lane<N>` are refused,
  and `lane new` prints the project folder instead of the repository root. `lane new` and `lane clean` name a lane data
  folder they could not delete.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed `devlanes infra down` printing `containers: -` while it stops the shared Compose containers.
- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `devlanes debug freeze` refusals naming the wrong cause for a running exec unit and wrong keys for replicas.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Fixed `devlanes exec` commands missing `global_env` and CA variables such as `NODE_EXTRA_CA_CERTS`.
  
  `devlanes env show` without a unit and the command `devlanes with` runs now include `global_env` and the profile's `env` too, except entries that read `self` or `replica`. Until the lane's infra is up, entries that read `infra.*` are left out and named.
  `devlanes exec` now refuses while another of those values does not evaluate (a `${{ env.X }}` the shell does not set, for example), or while the project file does not load and nothing in the lane runs.

## 0.11.3 — 2026-11-06

### Patch changes

- [91c4f2b](https://github.com/devlanes-run/devlanes/commit/91c4f2b60a70695afc248f0b045a499292336a43) Added a `devlanes doctor` warning when Windows refuses to let devlanes start programs.

## 0.11.2 — 2026-11-02

### Patch changes

- [fef7c86](https://github.com/devlanes-run/devlanes/commit/fef7c863cf1a266174c8bed86a23767e0a30dee4) Changed a bare `devlanes -v` to print the version, like `-V`, instead of opening the TUI.

## 0.11.1 — 2026-11-01

### Patch changes

- [22d6f20](https://github.com/devlanes-run/devlanes/commit/22d6f2063df1696a4511831d010c63b4a5a0fa6a) Removed `devlanes update --notes`.
  
  Read the release notes on the release page, which `devlanes update` links to.

## 0.11.0 — 2026-11-01

### Minor changes

- [9d7557c](https://github.com/devlanes-run/devlanes/commit/9d7557cf5829274623142c4393958b76b536d5a2) Changed lanes to export their project as `DEVLANES_LANE_PROJECT` instead of `DEVLANES_PROJECT`, so a lane's variables no longer pin devlanes commands in another project's folder or checkout to that project.
  
  `DEVLANES_PROJECT` is now only a setting you export yourself, and it wins over the current folder like `-p`. Scripts that read the lane's project from its environment use `DEVLANES_LANE_PROJECT`.

## 0.10.0 — 2026-11-01

### Minor changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Removed `devlanes certs trust` and `certs untrust`; use `devlanes setup`, which works out what this machine needs and asks once.
  
  Run `devlanes setup` instead of `certs trust --hosts --resolver`, and `devlanes setup --remove` instead of `certs untrust --hosts --resolver`.
  The TUI offers to run it when HTTPS or hostnames need it.

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed every devlanes update asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.
  
  `devlanes doctor` and the hosts status now report an old helper as `helper_revision_outdated` instead of `helper_version_mismatch`.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added each box's and each connected client's devlanes version to the TUI's Boxes view, `devlanes remote ls` and `devlanes daemon clients`.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added `devlanes update` and the TUI's `U` key, which install the latest release the way devlanes was installed, restart the daemon and bring its lanes back.
  
  `u` no longer hides the release notice.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added `devlanes uninstall`, which asks once, then stops the daemon, removes its login service, undoes `devlanes setup` and removes the binary, and asks again before deleting `~/.devlanes`; `install.sh --uninstall` and `install.ps1 -Uninstall` run it.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added a What's new dialog the first time the TUI opens after an update, and `devlanes update --notes`.

## 0.9.0 — 2026-10-29

### Patch changes

- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane that several boxes run.
  
  A switch moves the lane's hostnames and ports to the other box at once and closes open connections to the old box. The same lane on two boxes is now mirrored from one of them, even when their hostnames differ. To work with both at once, run the lane under another lane number or project on one box. In the TUI, box lanes now join the lanes row of the local project with the same name, and lanes this machine does not mirror are grey. See [several boxes with the same project](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/remote-multiple-boxes.md).

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

