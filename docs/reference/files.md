---
title: Files and folders
description: The devlanes home and what is in it, the lane data folders, logs, the CA, the daemon's socket, the files devlanes writes elsewhere, and the files of a project.
kind: reference
---

devlanes keeps its own files in one folder, the devlanes home. A few commands write outside it, and a project has files of its own in its repository.

## The devlanes home

| System | Path |
|---|---|
| macOS, Linux | `~/.devlanes` |
| Windows | `%USERPROFILE%\.devlanes` |
| With `DEVLANES_HOME` set | That path. |

Each home has its own daemon, lanes and port pool. The home and its `run` folder are private to your account: on macOS and Linux new ones get mode 0700, and the daemon sets an existing one of yours to 0700 and logs that it did. On Windows they get an access list for your account, SYSTEM and Administrators. The daemon refuses to start when the home is a symbolic link, not a folder, or another account's folder.

| Group | Path in the home | Contents |
|---|---|---|
| State | `config.yaml` | The user config. |
| State | `state.db`, `state.db-wal`, `state.db-shm` | The daemon's database ([The state database](#the-state-database)). |
| State | `state.db.legacy-<stamp>`, `data/.legacy-<stamp>/` | The database and the lane data folders that `devlanes daemon reset-state` set aside. devlanes does not read them again. |
| State | `client-id` | This machine's id, which boxes know it by. |
| State | `notices.json` | When each update notice was last shown, the ones dismissed in the TUI, and the last release whose highlights were shown. |
| State | `resolver-file` | The path of the resolver file or rule `devlanes setup` wrote, so `devlanes setup --remove` finds it after `hostnames.tld` changes. |
| Logs | `logs/<project>/<lane number>/<unit>/` | The output of each unit ([Logs](#logs)). |
| Logs | `logs/daemon.log`, `logs/daemon.log.1` | The daemon's log and the one before it. |
| Logs | `logs/uninstall.log` | The output of the package manager that `devlanes uninstall` ran. |
| Logs | `logs/update.log` | The output of package managers that `devlanes update` ran. |
| CA | `ca/` | The CA ([CA files](#ca-files)). |
| Data | `data/<project>/<lane id>/` | The lane data folders ([Lane data folder](#lane-data-folder)). |
| Run | `run/daemon.lock` | Held by the running daemon, so a second one does not start for the same home. |
| Run | `run/daemon.replacing` | While `devlanes daemon restart` runs: the process id of the daemon waiting to take over. |
| Run | `run/daemon.sock` | The daemon's socket on macOS and Linux, unless it is elsewhere ([Socket and pipe](#socket-and-pipe)). |
| Run | `run/daemon.socket` | On macOS and Linux, when the socket is elsewhere: the socket's path. |
| Run | `run/exe/<pid>.exe` | On Windows, a second link to the `devlanes.exe` of each running devlanes process, so a package manager can replace the file. |
| Remote | `run/ssh/` | On macOS and Linux, the shared SSH connections to boxes, `m-<hash>`, and their logs, `m-<hash>.log`. |
| Remote | `sync/<box>/<project>/<project>@<index>/<folder>/` | Folders synced from a box's lanes. |
| Update | `update/` | Releases that `devlanes update` downloaded, and its lock. |
| Update | `update-check.json` | The result of the last release check. |

## The state database

`state.db` holds, as data:

| What | Kept |
|---|---|
| Projects: name, repository, primary branch, project file | Until its last lane is cleaned. |
| Worktrees of each project, with their path and branch | As long as the project. |
| Lanes: number, name, worktree, the profiles and services each lane ran last, and whether it should run | Until the lane is cleaned. |
| Port blocks of the lanes and the place of each service's port in a block | As long as the lane and the project. |
| The `lane.create` tasks and setup entries that succeeded in each lane, with the declaration they ran | As long as the lane. |
| Runs of units and tasks: start and end time, exit code or signal, process ids, who started them | `history.retention`, default 7 days. |
| An index of each run's log files | As long as the log files. |
| CPU and memory per minute, summaries per run, memory peaks during starts | `metrics.retention`, default 7 days. |
| Boxes this machine connected to, the token for each, and the box chosen for a lane with `devlanes remote use` | `history.remote_hosts_retention` after the last connection, default 90 days. |
| On a box: the machines paired with it, by a hash of their token | Until `devlanes daemon revoke`. |
| Lane data folders waiting to be deleted | Until they are deleted. |

It holds no environment variables. When the file grows over `state.max_size` (default 512 MiB), an hourly cleanup deletes the oldest samples, then the oldest history, before their retention ends.

## Lane data folder

| | |
|---|---|
| Path | `<data root>/<project>/<lane id>`. The data root is `data` in the devlanes home, or `lanes.data_dir` in the user config. |
| Lane id | A number the daemon gives the lane when it creates it. It differs from the lane number and stays the same when the lane number changes, as when the lane's branch becomes the primary one. |
| Read as | `${{ lane.data_path }}` in expressions and `DEVLANES_LANE_DATA` in every unit. On Windows both are written with `/`, such as `C:/Users/you/.devlanes/data/myapp/3`. |
| `lanes.data_dir` | An absolute path, or one that starts with `~/`; on Windows also `~\` or a drive letter. |
| Created | When the lane gets its number, and again before a unit, an `exec` command or the lane's containers start. |
| Deleted | When the lane is cleaned: by `devlanes lane clean` after the lane's `lane.clean` tasks and teardown ran, and by `devlanes lane gc` or `lanes.removed_worktrees: clean` for a lane whose worktree is gone. The project's folder goes when no lane folder is left in it. |
| Not deleted at once | A folder that cannot be deleted, such as one a program on Windows still has open, is retried by `devlanes lane gc` and by the daemon's hourly maintenance. |

Two lanes that run from one worktree, one of them added with `devlanes lane new --name`, have separate data folders.

devlanes writes these folders inside a lane's data folder:

| Folder | Written by |
|---|---|
| `compose` | The Compose override for the lane's `per_lane` services. |
| `tauri` | The `tauri` preset: a config overlay for each service and replica. |
| `wrangler-registry` | Wrangler and Miniflare, which every unit points at it ([Environment variables](environment-variables.md#for-every-unit)). |

## Logs

| Log | Where | How long it is kept |
|---|---|---|
| A unit's output | `logs/<project>/<lane number>/<unit>/` in the devlanes home, in files of up to 16 MiB or one hour of output. | Until `logs.retention.days` (default 7) or `logs.retention.max_size` (default 5 GiB, across every project and lane) is passed. A cleanup every 10 minutes deletes the oldest files first. It keeps the newest file of each stopped unit and up to 1 MiB of a running unit's current file. |
| The daemon's log | `logs/daemon.log` in the devlanes home. Its level is `info`, or what `DEVLANES_LOG` sets. | When the daemon starts and the file is over 10 MiB, it becomes `daemon.log.1`, replacing the one before. |
| A container's output | Not stored. `devlanes infra logs` reads it from Docker. | As Docker keeps it. |

A unit's log folder holds:

| File | Contents |
|---|---|
| `<number>.log` | The file being written. |
| `<number>.log.zst` | A closed file, compressed. |
| `<number>.meta.json` | The lines, times and offsets a closed file holds, so a query finds a line without reading the file. |
| `.writer.lock` | Held by the one process that writes the folder. |

`devlanes lane clean` deletes the logs of the lane it names. A lane cleaned because its worktree is gone, by `devlanes lane gc` or by `lanes.removed_worktrees: clean`, keeps its logs until the retention limits remove them.

## CA files

In `ca/` in the devlanes home:

| File | Contents |
|---|---|
| `.lock` | Held while the CA is loaded or created. |
| `bundle.pem` | The system's root certificates and `ca.pem` in one file, for programs that replace their trust store instead of adding to it. |
| `ca-key.pem` | The root's private key. It has to be readable by your account only; the daemon refuses a key others can read. |
| `ca.pem` | The root certificate. It is valid for 10 years and can only sign names for local development. |

With `tls.ca: mkcert`, devlanes reads `rootCA.pem` and `rootCA-key.pem` from mkcert's folder instead and does not write `ca.pem` or `ca-key.pem`.

## Socket and pipe

Clients reach the daemon through a Unix socket or a named pipe.

| System | Endpoint |
|---|---|
| macOS | `run/daemon.sock` in the devlanes home. |
| Linux | `$XDG_RUNTIME_DIR/devlanes/daemon-<hash>.sock` when `XDG_RUNTIME_DIR` is an absolute path to a folder of yours with mode 0700, else `run/daemon.sock` in the devlanes home. |
| macOS, Linux, path of 104 bytes or more | `devlanes-<uid>/daemon-<hash>.sock` in the temporary folder (`TMPDIR`), else in `/tmp`. |
| Windows | The named pipe `\\.\pipe\devlanes-<hash>`. |

`<hash>` is computed from the devlanes home, and on Windows also from your account, so each home has its own endpoint.

## Files outside the home

| What | Path | Written by |
|---|---|---|
| `devlanes` on macOS and Linux | `~/.local/bin/devlanes` and `devlanes-hosts`, or `DEVLANES_INSTALL_DIR` | `install.sh` |
| `devlanes` on Windows | `%LOCALAPPDATA%\Programs\devlanes\devlanes.exe`, or `DEVLANES_INSTALL_DIR` | `install.ps1` |
| `devlanes` on a box | `~/.devlanes/bin/devlanes`, or `%USERPROFILE%\.devlanes\bin\devlanes.exe`, whatever `DEVLANES_HOME` says. `devlanes remote connect` looks there when devlanes is not on the `PATH` of the SSH session. | You, or `install.sh` with `--dir` |
| Login service on macOS | `~/Library/LaunchAgents/run.devlanes.daemon.plist` | `devlanes daemon install` |
| Login service on Linux | `~/.config/systemd/user/devlanes.service` | `devlanes daemon install` |
| Login service on Windows | The value `devlanes` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | `devlanes daemon install` |
| The CA in the system trust store | Depends on the system ([Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)) | `devlanes setup` |
| Hosts helper, on macOS and Linux | `/usr/local/libexec/devlanes-hosts` | `devlanes setup` |
| Hosts helper's sudo rule, on macOS and Linux | `/etc/sudoers.d/devlanes` | `devlanes setup` |
| Hostnames in the hosts file, on macOS and Linux | The lines between `# devlanes begin` and `# devlanes end` in `/etc/hosts` | The hosts helper, when the daemon asks it |
| Resolver file on macOS | `/etc/resolver/<tld>` | `devlanes setup`, with `hostnames.resolver: resolver` |
| Resolver file on Linux, systemd 258 or later | `/etc/systemd/dns-delegate.d/devlanes-<tld>.dns-delegate` | `devlanes setup`, with `hostnames.resolver: resolver` |
| Resolver rule on Windows | The registry key `devlanes-<tld>` under `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig` | `devlanes setup`, with `hostnames.resolver: resolver` |
| Synced build output | `~/devlanes/sync/<project>/<lane name>/<folder>`, or a service's `sync.to`. `~` is the `HOME` of the shell that started the lane. | The daemon, for a service with `sync` |
| Lane data | `lanes.data_dir` | The daemon, when that setting is set |
| The daemon's socket | [Socket and pipe](#socket-and-pipe) | The daemon |

`devlanes setup --remove` undoes what `devlanes setup` wrote. `devlanes uninstall` removes the command, the login service and what `setup` wrote, and deletes the home and the lane data when you agree.

## Files of a project

| File | Purpose |
|---|---|
| `devlanes.yaml` | The project file. Commands find it as [Picking the project](lane-and-unit-references.md#picking-the-project) lists. |
| `<name>.yaml`, such as `devlanes.prod.yaml` | A second project file, named with `--file`. Its name cannot end in `.local.yaml` or `.local.yml`. |
| `devlanes.local.yaml` | The local file of `devlanes.yaml`: one developer's changes, merged over the project file. |
| `<name>.local.yaml`, such as `devlanes.prod.local.yaml` | The local file of `<name>.yaml`. |
| `.env` | Values for `${{ env.NAME }}` in expressions, next to the project file. devlanes does not put them in the environment of processes. |

Each lane reads the local file and `.env` from its own worktree. When its worktree has none, as a new worktree has no files that Git ignores, the lane reads the primary worktree's. devlanes does not add the local file to `.gitignore`.

A `.env` file is read line by line:

| Line or value | Read as |
|---|---|
| Empty, or starting with `#` | Skipped. |
| `NAME=value`, `export NAME=value` | The variable `NAME`. Spaces around the name and the value are dropped. |
| A value in double quotes | The text inside, with `\n` as a line break and `\"` as a quote. |
| A value in single quotes | The text inside, as it is. |
| A value without quotes | The text up to ` #`, which starts a comment. |

Nothing in a value is expanded: `$OTHER` stays as it is.
