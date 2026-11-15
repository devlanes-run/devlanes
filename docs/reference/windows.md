---
title: Windows
description: Everything devlanes does differently on Windows, from shells and stop signals to ports, terminals and HTTPS.
kind: reference
---

devlanes runs natively on Windows 11 24H2 (build 26100) or later, on x64 and arm64. Each entry below is a behaviour that differs from macOS and Linux. [Install devlanes](../install.md) covers installing, and [Run devlanes in WSL](../how-to/wsl.md) the Linux build inside WSL.

## Shells and commands in units

| Subject | On Windows |
|---|---|
| `run:` strings and `ready.cmd` checks | Run by Git for Windows' `usr\bin\sh.exe`. The daemon finds it once when it starts, from the `git` on its `PATH`: from `git --exec-path`, else from where `git.exe` sits. MSYS2's and Cygwin's own Git are not used. |
| No Git for Windows | A unit with a `run:` string, and a `ready.cmd` check, fail with `shell_missing`. `devlanes doctor` reports `Git for Windows was not found, so units with run strings and ready.cmd checks cannot start`. A `run:` list needs no shell. |
| Git for Windows installed after the daemon started | Found after `devlanes daemon restart`. |
| `PATH` of a unit run by `sh.exe` | Git's `usr\bin` is added at the end. |
| Path conversion | `sh.exe` runs with `MSYS_NO_PATHCONV=1` and `MSYS2_ARG_CONV_EXCL=*`, so arguments and variables reach Windows programs as written. Inside a `run:` string, `$PWD` and `pwd` give a POSIX path such as `/c/dev/myapp`; `pwd -W` gives `C:/dev/myapp`. |
| Backslashes in a `run:` string | `sh.exe` reads `\` as an escape outside quotes. Paths such as `${{ lane.path }}` and `${{ project.primary_path }}` hold `\`. |
| A bare program name | Looked up on the unit's `PATH` with the `PATHEXT` extensions `.COM`, `.EXE`, `.BAT` and `.CMD`, in `PATHEXT` order. Other extensions are skipped. |
| `.bat` and `.cmd` files | Run through `cmd.exe`. An argument with a line break fails the unit with `unsafe_batch_argument`. |
| Working directory | A unit whose working directory is 260 characters or longer fails with `cwd_too_long`. |
| Glob characters | `*`, `?`, `[...]`, `{a,b}` and `~` in an argument of a program built on the MSYS2 or Cygwin runtime are not expanded. |
| Unit names | Names that differ only in letter case, such as `web` and `Web`, are refused by `devlanes config check`, since they would share one log folder. |

## Commands you type

| Subject | On Windows |
|---|---|
| Shells | The CLI runs in PowerShell, cmd and Git Bash. |
| Git Bash | Git Bash rewrites arguments that look like POSIX paths before it starts a Windows program, so `/c` can arrive as `C:/`. Commands devlanes prints as hints use POSIX shell quoting. |
| `devlanes env export` | Prints POSIX `export` lines by default. `--shell pwsh` prints `$env:NAME = 'value'` lines in ASCII, with other text decoded from Base64. Windows PowerShell 5.1 and PowerShell before 7.5 remove a variable assigned an empty value. |
| `devlanes env shell` | Starts the program in `SHELL` when it is set, else `%ComSpec%`, else `cmd.exe`. |
| Variable names | Folded to upper case wherever devlanes captures, merges or looks up an environment: `Path` and `PATH` are one variable, `devlanes env show` lists `PATH`, and `${{ env.Path }}` and `${{ env.PATH }}` read the same value. |
| Completions | `devlanes completions powershell` prints PowerShell completions. |

## Stop signals

Windows has no POSIX signals. [`services.<name>.stop`](/docs/reference/config/project/services/) maps to console events:

| `stop.signal` | `pty: true` | `pty: false` | `pty: false`, a program on the MSYS2 or Cygwin runtime runs in the unit |
|---|---|---|---|
| `SIGTERM`, `SIGINT` | Ctrl-C, typed into the unit's console | Ctrl-C event | Ctrl-Break event |
| `SIGHUP`, `SIGQUIT`, `SIGUSR1`, `SIGUSR2` | Ctrl-Break event | Ctrl-Break event | Ctrl-Break event |
| `SIGKILL` | The unit's processes are terminated | The unit's processes are terminated | The unit's processes are terminated |

| Subject | On Windows |
|---|---|
| Windows of a unit | A stop also posts `WM_CLOSE` to the unit's visible windows. |
| After `stop.timeout` | What still runs is terminated. |
| Exit reported after a console event | `killed by signal 2`, for Ctrl-C and Ctrl-Break alike. |
| Exit reported after termination | `killed by signal 9`. |
| Units an earlier daemon left running | Stopped as `pty: false` units are. |

## Processes of a unit

| Subject | On Windows |
|---|---|
| Job object | Each unit runs in a job object its processes cannot leave. A program that starts a child outside its job fails with access denied. |
| When the main process exits | What is left in the unit's job is ended, for tasks too. |
| `pty: true` units | Run in a ConPTY pseudo console. Logs hold the lines as that console showed them, and `ready.log` patterns match those lines. |
| Console width | A `pty: true` unit's console is 300 columns wide while nobody types into it or zooms its pane. |
| Units that stop with the daemon | `pty: true` units end when the daemon ends, since their pseudo console belongs to the daemon. `pty: false` units keep running, and the next daemon finds them as orphaned. |

## Terminals and the daemon

The daemon leaves the terminal's process group and console when it starts. It also tries to leave the terminal's job object.

| Terminal | Effect on a daemon it starts |
|---|---|
| A terminal whose job lets processes break away | The daemon leaves the job and outlives the terminal. |
| A terminal whose job refuses breakaway and does not kill on close | The daemon starts inside the job and outlives the terminal. |
| A terminal whose job refuses breakaway and kills its processes when it closes | The daemon starts inside the job and ends when the terminal closes. `devlanes doctor` warns: `this terminal ends the processes it started when it closes, so a daemon started here stops with it`. |
| An SSH session whose job kills on close and refuses breakaway, on a Windows box | The box daemon ends with the session. `devlanes remote ls` on the connected machine warns that the box daemon stops when the SSH session ends. Win32-OpenSSH's session job allows breakaway. |

`devlanes daemon install` starts the daemon at logon instead, from a `devlanes` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Signing out or shutting down ends the daemon and its units without running `lane.stop` tasks.

## Elevated terminals

| Subject | On Windows |
|---|---|
| A daemon started from an elevated terminal | Runs every unit with administrator rights. `devlanes doctor` warns `the daemon runs as administrator, and so does every unit it starts`. |
| An elevated terminal | `devlanes doctor` warns `this terminal runs as administrator; a daemon it starts runs every unit that way`. |

## Throttling and freezing

| Subject | On Windows |
|---|---|
| Throttled lane | Its processes run in efficiency mode, as Task Manager sets it: idle priority with power throttling. |
| Frozen lane | Its processes are suspended. Their memory is not trimmed. |

[Throttling and freezing](../concepts/freezing.md) explains both.

## Paths and files

| Subject | On Windows |
|---|---|
| devlanes home | `%USERPROFILE%\.devlanes`, or `DEVLANES_HOME`. `HOME`, which Git Bash sets, is not read for the home. |
| Daemon endpoint | A named pipe, `\\.\pipe\devlanes-<hash>`, from the user and the home. There is no socket file. |
| `HOME` for synced folders | A [`services.<name>.sync`](/docs/reference/config/project/services/) entry without `to`, or with a `to` that starts with `~`, reads `HOME` from the environment of the command that started the lane. PowerShell and cmd do not set `HOME`, and the entry then fails with `HOME is not set in the client environment`. Git Bash sets it. |
| A running executable | Windows refuses to overwrite it. An update renames it to `devlanes.exe.old-<timestamp>`, and `devlanes doctor` deletes the old copy once nothing runs it. The daemon, the TUI and `devlanes update` keep a hard link to their executable in `<devlanes home>\run\exe` while they run. |

[Files and folders](files.md) lists the files in the home.

## Ports

| Subject | On Windows |
|---|---|
| Reserved port ranges | Hyper-V, WSL and Docker Desktop reserve TCP port ranges that nothing can bind. `netsh int ipv4 show excludedportrange protocol=tcp` lists them. New lane port blocks skip them. |
| A lane whose ports became reserved | `devlanes doctor` warns that the lane has ports in a Windows reserved port range, and that `devlanes lane clean` gives it new ports on its next start. It warns the same for a proxy port and a port mirrored from a box. |
| Port 80 | Often held by http.sys (PID 4, System), listed by `netsh http show servicestate`. The proxy then falls back to 8080, and its message names both commands. |
| Ports 443 and 80 | The proxy needs no privilege for them. |
| `proxy.lan` | Windows Defender Firewall asks the first time whether `devlanes.exe` may accept connections from the network. |
| Netsh refused | `devlanes doctor` warns when Windows refused to let devlanes start `netsh`, which an antivirus or application control causes. |

## Hostnames and HTTPS

| Subject | On Windows |
|---|---|
| `*.localhost` | Windows 11 answers every `*.localhost` name with loopback. devlanes never writes the hosts file on Windows, and `devlanes setup` skips that step. |
| Windows Server | Does not resolve `*.localhost`. `devlanes doctor` reports that the test lookup failed. |
| Resolver strategy | `devlanes setup` adds a Name Resolution Policy Table rule that sends the configured TLD to the daemon's DNS server, which listens on `127.53.0.1:53`. |
| Trust store | [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md) covers the Windows store. |
| `curl.exe` | Uses the Windows store and checks revocation. The devlanes CA publishes no revocation list, so `curl.exe` fails unless it gets `--ssl-revoke-best-effort`, which still verifies the chain. |

## Coding agents

| Subject | On Windows |
|---|---|
| A cancelled MCP tool call | The server sends Ctrl-Break to the command's process group and terminates its job after the wait in [MCP tools](mcp-tools.md#progress-and-cancellation). |
| Environment of a lane started through MCP | The server's own, unless `DEVLANES_SHELL_ENV` is `pwsh` or `powershell` ([MCP tools](mcp-tools.md#environment-of-a-lane-started-through-mcp)). |
