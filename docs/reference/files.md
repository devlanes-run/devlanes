---
title: Files and folders
description: Where devlanes keeps its home, the user config, logs, the CA and the lane data folders, and the files of a project.
kind: reference
---

devlanes keeps its own files in one folder, the devlanes home. A project has files of its own in its repository.

## The devlanes home

| System | Path |
|---|---|
| macOS, Linux | `~/.devlanes` |
| Windows | `%USERPROFILE%\.devlanes` |
| With `DEVLANES_HOME` set | That path. |

Each home has its own daemon, lanes and port pool, and is private to your account.

| Path in the home | Contents |
|---|---|
| `config.yaml` | The user config. |
| `state.db` | The daemon's database: projects, lanes, port blocks, run history and the boxes this machine connected to. It holds no environment variables. |
| `logs/daemon.log` | The daemon's log. |
| `logs/<project>/<lane number>/<unit>/` | The output of each unit. |
| `ca/` | The CA. |
| `data/<project>/<lane id>/` | The lane data folders. |
| `sync/` | Folders synced from boxes. |
| `p2p-key` | This machine's key for pairing by ticket, readable by your account only. Do not share it. Deleting or replacing it gives this machine a new identity, so every box and machine paired by ticket refuses it until you pair again. Created the first time this machine asks for a ticket or connects by ticket. |
| `p2p-port` | The UDP ports a machine that has accepted pairings by ticket keeps across restarts. Deleting it lets the ports change, and machines that reach this one without a relay then need a new ticket. |

## Logs

A unit's output is kept until it is older than `logs.retention.days` (default 7) or all stored output passes `logs.retention.max_size` (default 5 GiB); the oldest goes first. `devlanes logs` reads it. `devlanes lane clean` deletes a lane's logs with the lane.

## CA files

| File | Contents |
|---|---|
| `ca/ca.pem` | The root certificate. It can only sign names for local development. |
| `ca/ca-key.pem` | The root's private key, readable by your account only. |
| `ca/bundle.pem` | The system's root certificates and `ca.pem` in one file, for programs that replace their trust store instead of adding to it. |
| `ca/truststore.p12` | The certificates of `bundle.pem` as a PKCS#12 trust store for Java, with the password `changeit`. |

## Lane data folder

Each lane has a folder of its own outside its worktree, at `data/<project>/<lane id>` in the devlanes home, or under `lanes.data_dir` from the user config. Expressions read it as `${{ lane.data_path }}`, and every unit gets it as `DEVLANES_LANE_DATA`. The lane id stays the same when the lane's number changes. devlanes creates the folder before a lane starts.

:::caution
Cleaning a lane, with `devlanes lane clean` or `devlanes lane gc`, deletes its data folder.
:::

## Files of a project

| File | Purpose |
|---|---|
| `devlanes.yaml` | The project file. |
| `devlanes.local.yaml` | The local file: one developer's changes, merged over the project file. |
| `<name>.yaml`, such as `devlanes.prod.yaml` | A second project file, named with `--file`. |
| `<name>.local.yaml` | The local file of `<name>.yaml`. |
| `.env` | Values for `${{ env.NAME }}` in expressions, next to the project file. devlanes does not put them in the environment of processes. |

A lane reads the local file and `.env` from its own worktree, and from the primary worktree when its worktree has none. devlanes does not add the local file to `.gitignore`.
