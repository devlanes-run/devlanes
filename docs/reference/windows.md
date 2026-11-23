---
title: Windows
description: What you need to know or do differently when you run devlanes on Windows.
kind: reference
---

devlanes runs natively on Windows 11 24H2 or later. [Install devlanes](../install.md) covers installing, and [Run devlanes in WSL](../how-to/wsl.md) the Linux build inside WSL.

| Subject | On Windows |
|---|---|
| Git for Windows | Required. Its `sh.exe` runs `run:` strings and `ready.cmd` checks, so `run:` strings stay POSIX shell on every OS. Install it before the daemon starts, or run `devlanes daemon restart` afterwards. A `run:` list needs no shell. |
| Terminals | Some terminals end every process they started when they close, the daemon included. `devlanes doctor` warns about such a terminal. Start the daemon from another one, or run `devlanes daemon install` so it starts at each logon. |
| Elevated terminals | A daemon started from an elevated terminal runs every unit as administrator. `devlanes doctor` warns, and `devlanes daemon restart` from a normal terminal fixes it. |
| Stop signals | `SIGTERM` and `SIGINT` send Ctrl-C. `SIGHUP`, `SIGQUIT`, `SIGUSR1` and `SIGUSR2` send Ctrl-Break. `SIGKILL` ends the unit's processes at once. |
| Units in a terminal | A unit with `pty: true`, the default, ends when the daemon ends. |
| Variable names | Not case-sensitive: Path and PATH are one variable. A process gets each name as it was written where its value comes from, so Gradle's `ORG_GRADLE_PROJECT_` properties keep their case. |
| `HOME` | PowerShell and cmd do not set it. A [`services.<name>.sync`](/docs/reference/config/project/services/) entry without `to`, or with a `to` under `~/`, then fails; give `to` a full path, or start the lane from Git Bash. |
| Folders in use | Windows cannot delete a folder a program holds open. Stop the lane before you remove or move its worktree. `devlanes lane clean` leaves a data folder still in use, and `devlanes lane gc` deletes it later. |
| Reserved ports | Hyper-V, WSL and Docker reserve port ranges. New lanes skip them, and `devlanes doctor` warns when a lane's ports fall into one later. |
| `curl.exe` | Needs `--ssl-revoke-best-effort` for lane hostnames, since the devlanes CA publishes no revocation list. |
| Antivirus | An antivirus or application control may block `devlanes.exe`. When the daemon fails to start with a message that says so, mark it as trusted there. |
| Completions | `devlanes completions powershell` prints PowerShell completions. |
