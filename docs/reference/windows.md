---
title: Windows
description: Behaviour of devlanes that differs on Windows, in shells, stop signals, processes, terminals, ports, paths and HTTPS.
kind: reference
---

devlanes runs natively on Windows 11 24H2 (build 26100) or later, on x64 and arm64. Each entry below is a behaviour that differs from macOS and Linux. [Install devlanes](../install.md) covers installing, and [Run devlanes in WSL](../how-to/wsl.md) the Linux build inside WSL.

## Shells and commands in units

| Subject | On Windows |
|---|---|
| `run:` strings and `ready.cmd` checks | Run by Git for Windows' `usr\bin\sh.exe`. The daemon finds it once when it starts, from the `git` on its `PATH`: from `git --exec-path`, else from where `git.exe` sits. MSYS2's and Cygwin's own Git are not used. |
| No Git for Windows | A unit with a `run:` string, and a `ready.cmd` check, fail with `shell_missing`. `devlanes doctor` reports `Git for Windows was not found, so units with run strings and ready.cmd checks cannot start` and suggests `install Git for Windows (https://gitforwindows.org), then devlanes daemon restart`. A `run:` list needs no shell. |
| Git for Windows installed after the daemon started | Found after `devlanes daemon restart`. |
| `PATH` of a unit run by `sh.exe` | Git's `usr\bin` is added at the end. |
| Path conversion | `sh.exe` runs with `MSYS_NO_PATHCONV=1` and `MSYS2_ARG_CONV_EXCL=*`, so arguments and variables reach Windows programs as written. Inside a `run:` string, `$PWD` and `pwd` give a POSIX path such as `/c/dev/myapp`; `pwd -W` gives `C:/dev/myapp`. |
| Backslashes in a `run:` string | `sh.exe` reads `\` as an escape outside quotes. Paths such as `${{ lane.path }}` and `${{ project.primary_path }}` hold `\`. |
| A bare program name | Looked up on the unit's `PATH` with the `PATHEXT` extensions `.COM`, `.EXE`, `.BAT` and `.CMD`, in `PATHEXT` order. Other extensions are skipped. |
| `.bat` and `.cmd` files | Run through `cmd.exe`. An argument with a line break fails the unit with `unsafe_batch_argument`. |
| npm scripts | A [`services.<name>.run_script`](/docs/reference/config/project/services/) entry for a script of an npm project runs its `pre`, main and `post` steps without npm, with npm's package variables and `node_modules\.bin` on `PATH`. Its launch note, which `devlanes env show UNIT --explain` prints, says `on Windows npm scripts run through SHELL without npm`. Other package managers run their own `run`. |
| Working directory | A unit whose working directory is 260 characters or longer fails with `cwd_too_long`. |
| Glob characters | `*`, `?`, `[...]`, `{a,b}` and `~` in an argument of a program built on the MSYS2 or Cygwin runtime are not expanded. |
| Names that differ only in letter case | Units, tasks and setup or teardown entries whose names differ only in case, such as `web` and `Web`, are refused when the project file loads: `units FIRST and SECOND would share one log folder on Windows, which ignores case; rename one`. |

## Commands you type

| Subject | On Windows |
|---|---|
| Shells | The CLI runs in PowerShell, cmd and Git Bash. |
| Git Bash | Git Bash rewrites arguments that look like POSIX paths before it starts a Windows program, so `/c` can arrive as `C:/`. Commands devlanes prints as hints use POSIX shell quoting. |
| `devlanes env export` | Prints POSIX `export` lines by default. `--shell pwsh` prints `$env:NAME = 'value'` lines in ASCII, with other text decoded from Base64. Its help notes that Windows PowerShell 5.1 and PowerShell before 7.5 remove a variable assigned an empty value. |
| `devlanes env shell` | Starts the program in `SHELL` when it is set, else `%ComSpec%`, else `cmd.exe`. |
| Variable names | Folded to upper case wherever devlanes captures, merges or looks up an environment: `Path` and `PATH` are one variable, `devlanes env show` lists `PATH`, and `${{ env.Path }}` and `${{ env.PATH }}` read the same value. |
| Completions | `devlanes completions powershell` prints PowerShell completions. |

## Stop signals

Windows has no POSIX signals. [`services.<name>.stop`](/docs/reference/config/project/services/) maps to console events:

| `stop.signal` | `pty: true` | `pty: false` | `pty: false`, MSYS2 or Cygwin program in the unit |
|---|---|---|---|
| `SIGTERM`, `SIGINT` | Ctrl-C, typed into the unit's console | Ctrl-C event | Ctrl-Break event |
| `SIGHUP`, `SIGQUIT`, `SIGUSR1`, `SIGUSR2` | Ctrl-Break event | Ctrl-Break event | Ctrl-Break event |
| `SIGKILL` | The unit's processes are terminated | The unit's processes are terminated | The unit's processes are terminated |

| Subject | On Windows |
|---|---|
| Launch note | A unit whose `stop.signal` is sent as Ctrl-Break gets the note `on Windows stop.signal NAME is sent as Ctrl-Break`, which `devlanes env show UNIT --explain` prints. |
| Windows of a unit | A stop also posts `WM_CLOSE` to the unit's visible windows. |
| After `stop.timeout` | What still runs is terminated. |
| Exit reported after a console event | `killed by signal 2` when the event ended the program. A program that handles the event and exits reports its own exit code. A unit started through Git's `sh` reports what `sh` reports, and just and Git's `bin\bash.exe` pass that on as code `512`. |
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
| A terminal whose job refuses breakaway and kills its processes when it closes | The daemon starts inside the job and ends when the terminal closes. `devlanes doctor` warns `this terminal ends the processes it started when it closes, so a daemon started here stops with it` and suggests `start the daemon from another terminal`. |
| An SSH session whose job kills on close and refuses breakaway, on a Windows box | The box daemon ends with the session. `devlanes remote ls` on the connected machine warns ``the box daemon stops when this SSH session ends. Run `devlanes daemon install` on the box``. Win32-OpenSSH's session job allows breakaway. |

Signing out or shutting down ends the daemon and its units without running `lane.stop` tasks.

## Starting at logon

| Subject | On Windows |
|---|---|
| `devlanes daemon install` | Writes a `devlanes` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` that starts the daemon at each logon, and starts nothing now. It prints `logon Run entry installed: LOCATION; the daemon starts at each logon`. |
| `DEVLANES_HOME` | The Run value passes no environment, so `daemon install` refuses while `DEVLANES_HOME` points away from `%USERPROFILE%\.devlanes`: `the logon start cannot pass DEVLANES_HOME, so its daemon uses DEFAULT, not HOME; ...`. |
| Task Manager | The entry shows in Task Manager's Startup apps. Turned off there, `devlanes doctor` warns `the logon Run entry at PATH is turned off in Task Manager's Startup apps`, and `devlanes daemon install` turns it back on and prints `turned it back on in Task Manager's Startup apps`. |
| Another binary | When the value starts another devlanes, `devlanes doctor` warns `the logon Run entry at PATH runs another devlanes: COMMAND` and suggests `devlanes daemon install`. |

## Elevated terminals

| Subject | `devlanes doctor` warns | and suggests |
|---|---|---|
| The daemon runs elevated, and every unit it starts runs with administrator rights | `the daemon runs as administrator, and so does every unit it starts` | `devlanes daemon restart from a terminal that is not elevated` |
| This terminal is elevated, so a daemon it starts would run units that way | `this terminal runs as administrator; a daemon it starts runs every unit that way` | `run devlanes from a terminal that is not elevated` |

An elevated daemon still accepts commands from your unelevated terminals.

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
| Daemon endpoint | A named pipe, `\\.\pipe\devlanes-<hash>`, from the user and the home. There is no socket file. Only your user can open it, and your own processes that run at low integrity, as most sandboxes do, cannot. An elevated daemon still owns the pipe as your user and accepts your unelevated processes. |
| `HOME` for synced folders | A [`services.<name>.sync`](/docs/reference/config/project/services/) entry without `to`, or with a `to` that is `~` or starts with `~/`, reads `HOME` from the environment of the command that started the lane. The default folder is `HOME/devlanes/sync/PROJECT/LANE/FOLDER`. PowerShell and cmd do not set `HOME`, and the entry then fails with `HOME is not set in the client environment`. Git Bash sets it. |
| A folder a process holds | Windows cannot delete or rename a folder that a running process works in or holds a file open in. `devlanes lane clean` leaves such a lane data folder, and `devlanes lane gc` deletes it once nothing holds it. `git worktree remove` fails the same way while the lane runs in the worktree. |
| A running executable | Windows refuses to overwrite it. An update renames it to `devlanes.exe.old-<timestamp>`, and `devlanes doctor` deletes the old copy once nothing runs it. The daemon, the TUI and `devlanes update` keep a hard link to their executable in `<devlanes home>\run\exe` while they run. |

[Files and folders](files.md) lists the files in the home.

## Ports

| Subject | On Windows |
|---|---|
| Reserved port ranges | Hyper-V, WSL and Docker Desktop reserve TCP port ranges that nothing can bind. `netsh int ipv4 show excludedportrange protocol=tcp` lists them. New lane port blocks skip them. |
| A lane whose ports became reserved | `devlanes doctor` warns `LANE has ports PORTS in a Windows reserved port range, which cannot be bound`, and suggests `devlanes lane clean LANE`, which gives it new ports on its next start. It warns the same for a proxy port and a port mirrored from a box. |
| Ports 443 and 80 | The proxy needs no privilege for them. Access denied on them means another program holds the port, port 80 often by http.sys (PID 4, System), or Windows reserved it. The proxy then falls back to 8443 or 8080 with a message that names `netsh http show servicestate` and `netsh int ipv4 show excludedportrange protocol=tcp`. |
| `proxy.lan` | The proxy listens on every network interface, and Windows Defender Firewall asks the first time whether `devlanes.exe` may accept connections from the network. Devices on the network reach it only where the firewall allows it. |

## Antivirus and application control

| Subject | On Windows |
|---|---|
| `netsh` refused | `devlanes doctor` warns `Windows refused to let devlanes start netsh, so it may not be allowed to start any program` and suggests `an antivirus or application control may restrict devlanes.exe; mark it as trusted there`. |
| Daemon refused | When Windows will not run `devlanes.exe`, starting the daemon fails with `Windows refused to start the daemon (PATH): ERROR. An antivirus or application control may restrict devlanes.exe; mark it as trusted there`. |

## Hostnames and HTTPS

| Subject | On Windows |
|---|---|
| `*.localhost` | Windows 11 answers every `*.localhost` name with loopback. devlanes never writes the hosts file on Windows, and `devlanes setup` skips that step. |
| Windows Server | Does not resolve `*.localhost`. `devlanes doctor` reports that the test lookup failed. |
| Resolver strategy | `devlanes setup` adds a Name Resolution Policy Table rule that sends the configured TLD to the daemon's DNS server, which listens on `127.53.0.1:53`. |
| Trust store | [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md) covers the Windows store. |
| `curl.exe` | Uses the Windows store and checks revocation. The devlanes CA publishes no revocation list, so `curl.exe` fails unless it gets `--ssl-revoke-best-effort`, which still verifies the chain. |

## Remote boxes

| Subject | On Windows |
|---|---|
| Your machine on Windows | Win32-OpenSSH has no connection sharing, so each connection and reconnect to a box signs in to SSH again. devlanes runs `<System32>\OpenSSH\ssh.exe`, else the first `ssh.exe` on `PATH`. Without either, connecting fails with `no ssh client found (...); add the Windows optional feature "OpenSSH Client" or put ssh.exe on PATH`. |
| A Windows box | Its SSH shell is cmd.exe or PowerShell, so the session runs as an encoded PowerShell command. Your machine finds devlanes there with PowerShell's `Get-Command` on that account's `PATH`, then in `%USERPROFILE%\.devlanes\bin`. |
| A devlanes path on a cmd.exe box | A path holding `%`, `!` or `"` cannot run from cmd.exe, and connecting fails with ``cmd.exe on BOX cannot run devlanes from PATH because the path holds `%`, `!` or `"`. Install it in another folder``. |
| An administrator's SSH session | Is elevated, so a box daemon it starts runs every unit with administrator rights, as under [Elevated terminals](#elevated-terminals). |
| The box daemon and the SSH session | The SSH row under [Terminals and the daemon](#terminals-and-the-daemon). |

## Coding agents

| Subject | On Windows |
|---|---|
| A cancelled MCP tool call | The server sends Ctrl-Break to the command's process group and terminates its job after the wait in [MCP tools](mcp-tools.md#progress-and-cancellation). |
| Environment of a lane started through MCP | The server's own, unless `DEVLANES_SHELL_ENV` is `pwsh` or `powershell` ([MCP tools](mcp-tools.md#environment-of-a-lane-started-through-mcp)). |
| A sandboxed agent shell | Cannot open the daemon's pipe when it runs at low integrity, as under Daemon endpoint in [Paths and files](#paths-and-files). |
