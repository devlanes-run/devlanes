---
title: Windows
description: What you need to know or do differently when you run devlanes on Windows.
kind: reference
---

devlanes runs natively on Windows 11 24H2 or later. [Install devlanes](../install.md) covers installing, and [Run devlanes in WSL](../how-to/wsl.md) the Linux build inside WSL.

- **Git for Windows is required.** Its `sh.exe` runs `run:` strings and `ready.cmd` checks, so `run:` strings stay POSIX shell on every OS. Install it before the daemon starts, or run `devlanes daemon restart` afterwards. A `run:` list needs no shell.
- **Some terminals end the daemon when they close.** `devlanes doctor` warns about such a terminal; start the daemon from another one, such as Windows Terminal, or run `devlanes daemon install` so it starts at each logon.
- **Do not start the daemon from an elevated terminal.** It would run every unit as administrator. `devlanes doctor` warns, and `devlanes daemon restart` from a normal terminal fixes it.
- **Stop signals become Ctrl-C and Ctrl-Break.** `SIGTERM` and `SIGINT` send Ctrl-C, `SIGHUP`, `SIGQUIT`, `SIGUSR1` and `SIGUSR2` send Ctrl-Break, and `SIGKILL` ends the unit's processes at once.
- **Units in a terminal stop with the daemon.** A unit with `pty: true`, the default, ends when the daemon ends.
- **Variable names are not case-sensitive.** devlanes treats `Path` and `PATH` as one variable and shows it as `PATH`.
- **`HOME` is not set in PowerShell or cmd.** A [`services.<name>.sync`](/docs/reference/config/project/services/) entry without `to`, or with a `to` under `~/`, then fails; set `to` to a full path, or start the lane from Git Bash.
- **A folder in use cannot be deleted.** Stop the lane before you remove or move its worktree. `devlanes lane clean` leaves a data folder that a program still holds, and `devlanes lane gc` deletes it later.
- **Some ports are reserved.** Hyper-V, WSL and Docker reserve port ranges; new lanes skip them, and `devlanes doctor` warns when a lane's ports fall into one later.
- **`curl.exe` needs `--ssl-revoke-best-effort`** for lane hostnames, since the devlanes CA publishes no revocation list.
- **An antivirus may block `devlanes.exe`.** Mark it as trusted when the daemon fails to start with a message that says so.
- **PowerShell completions:** `devlanes completions powershell`.
