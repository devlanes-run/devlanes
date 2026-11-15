---
title: Troubleshooting
description: Messages and symptoms you may see, each with its likely cause and the page that fixes it.
kind: reference
---

Each row starts from what you see: a message, a warning of `devlanes doctor` or a behaviour. Text in capitals, such as `PORT` or `LANE`, stands for the value devlanes prints there. `devlanes debug why UNIT` explains a unit that is not running or not ready, and `devlanes doctor` checks the machine.

## The daemon

| What you see | Likely cause | Page |
|---|---|---|
| Lanes are `idle` after you closed a terminal, and `status` shows `needs start from a shell: devlanes start --lane LANE` | The daemon ended with the terminal, or restarted, and a new daemon started without the lanes' shell environment | [Bring lanes back after a reboot](how-to/lanes/after-a-reboot.md) |
| `doctor`: `this terminal ends the processes it started when it closes, so a daemon started here stops with it` | On Windows, the terminal's job object kills what it started and refuses breakaway | [Windows](reference/windows.md#terminals-and-the-daemon) |
| `the daemon restarted and lost this lane's shell environment` | The lane has run no start from a shell since the daemon started | [Bring lanes back after a reboot](how-to/lanes/after-a-reboot.md) |
| `doctor`: `not running (...)` on its daemon line | No daemon runs; a command that needs one starts it | [The daemon](concepts/daemon.md) |
| `the running daemon is devlanes VERSION ... and cannot serve this CLI` | The daemon runs another release than the CLI | [Update and uninstall](how-to/update.md) |
| `doctor`: `the daemon runs VERSION, this CLI is VERSION` | The CLI was updated and the daemon was not restarted | [Update and uninstall](how-to/update.md) |
| `the running daemon is older and does not know this command` | The same, for a single command | [Update and uninstall](how-to/update.md) |
| `auto-spawn refused in sandbox (...)` | The command runs inside an agent sandbox, where the CLI does not start a daemon | [Use devlanes from an agent sandbox](how-to/agents/sandboxes.md) |
| `doctor`: `daemon runs inside a sandbox (...)` or `this shell runs inside a sandbox (...)` | The same | [Use devlanes from an agent sandbox](how-to/agents/sandboxes.md) |
| `daemon is unreachable: ...` | The daemon does not answer on its socket or pipe | [The daemon](concepts/daemon.md) |
| `the daemon has not finished starting: step "STEP" made no progress for Ns` | A starting daemon is stuck on one step; `devlanes daemon logs` shows it | [Report a bug](how-to/report-a-bug.md) |
| `the daemon is shutting down; retry once it has stopped, or start a new one now` | A command reached a daemon that is stopping | [The daemon](concepts/daemon.md) |
| `devlanes refuses PATH: other accounts can access it`, or `another user owns it`, `it is a symlink`, `it is not a directory` | The devlanes home is shared, owned by another user, a link, or not a folder | [Files and folders](reference/files.md) |
| `doctor`: `user config PATH:LINE: MESSAGE` | The user config does not load | [Files and folders](reference/files.md) |
| `doctor`: `state schema N is newer than this devlanes (M)` | An older devlanes reads a home a newer one wrote | [Update and uninstall](how-to/update.md) |
| `doctor`: `state database schema is unreadable` | The state database is damaged | [Report a bug](how-to/report-a-bug.md) |

## Starting services

| What you see | Likely cause | Page |
|---|---|---|
| `UNIT couldn't start: PROGRAM: command not found on PATH` | The program is not on the `PATH` of the shell that started the lane | [Environment and expressions](concepts/environment.md) |
| `UNIT couldn't start: PROGRAM is not executable` | The file lacks execute permission | [Add devlanes to a project](how-to/projects/add-devlanes.md) |
| `UNIT couldn't start: working directory DIR does not exist` | `cwd` names a folder the worktree lacks | [Add devlanes to a project](how-to/projects/add-devlanes.md) |
| `UNIT wasn't ready in time` | The ready check never passed; the service keeps running and holds its port | [Wait until a service is ready](how-to/services/ready-checks.md) |
| `nothing accepts connections on port PORT; it listens on ...` | The service listens on another port than the one devlanes gave it | [A service, a port and a hostname](learn/first-service.md) |
| A service crashed, and `status` still shows it `ready` | A watcher such as `node --watch` keeps the process alive after the crash, and a ready check runs only during a start | [When a service fails](learn/failures.md) |
| `exited with code N` or `killed by signal N` | The process ended; `devlanes logs UNIT` shows why | [When a service fails](learn/failures.md) |
| `UNIT is still running from a previous daemon (pid N); start does not take it over` | An `orphaned` unit from an earlier daemon holds its place | [States and exit codes](reference/states-and-exit-codes.md) |
| `UNIT is already running; stop it first, or restart it` | A start named a running unit | [States and exit codes](reference/states-and-exit-codes.md) |
| `queued for a start slot`, or `queued: N of M starting`, and units stay `queued` | The machine-wide start gate paces starts | [Start order and readiness](concepts/start-order.md) |
| `task TASK failed with exit code N` | A task of the lane failed | [Run installs, migrations and seeds](how-to/services/tasks.md) |
| `infra setup failed: ...` | An `infra.setup` entry failed | [Share one database server between lanes](how-to/infra/shared-database.md) |
| `Docker isn't running or can't be reached` | The project has containers and Docker is down | [Give each lane its own container](how-to/infra/container-per-lane.md) |
| `doctor`: `Compose VERSION cannot apply per-lane overrides` | Docker Compose is too old | [Give each lane its own container](how-to/infra/container-per-lane.md) |
| `container NAME did not pass its CHECK check within TIME` | A container's health check did not pass | [Share one database server between lanes](how-to/infra/shared-database.md) |
| `no profile NAME; profiles: ...` | A start named a profile the project file lacks | [Share settings and choose which services start](how-to/projects/templates-and-profiles.md) |
| `not selected by profile NAME` or `autostart is off; it only starts when named` | The unit is outside the profiles the lane started with | [Start a service only on demand](how-to/services/on-demand.md) |
| `NAME is a configured service of LANE; give exec another --name, or run the configured service` | `exec` used the name of a configured unit | [A coding agent runs the lane](learn/coding-agents.md) |
| `name at least one unit` | A command needs a unit and got none | [Lane and unit references](reference/lane-and-unit-references.md) |
| `the start of LANE was cancelled by a stop, clean or reset of the lane` | Another command stopped the lane while it started | [Start order and readiness](concepts/start-order.md) |

## Ports

| What you see | Likely cause | Page |
|---|---|---|
| `port PORT is taken by pid N` | Another process holds a port of the lane | [Ports](concepts/ports.md) |
| `port PORT is taken by LANE:UNIT` | Another lane's unit holds the port | [Ports](concepts/ports.md) |
| `the port pool has no free block of N ports for another lane` | Every block of `ports.range` belongs to a lane | [Remove lanes](how-to/lanes/clean-up.md) |
| `lane is unassigned: lanes.max is N` | The project reached its lane limit | [Remove lanes](how-to/lanes/clean-up.md) |
| `the lane's port block of N ports cannot hold every port the config asks for` | The services need more ports than `lanes.block_size` gives | [Give a service several ports and hostnames](how-to/services/ports-and-hostnames.md) |
| `config changed ports; devlanes reload --lane LANE restarts the lane with them` | An edit needs ports the running lane's block lacks | [Config files and reload](concepts/config-and-reload.md) |
| `port PORT is mirrored from BOX for UNIT` | A local service needs a port that a box's mirror holds | [Choose what a box shares](how-to/remote/what-a-box-shares.md) |
| `doctor`: `port PORT is held by PROGRAM (pid N); the LISTENER proxy will fall back to another port` | Another program holds 443 or 80 | [Choose the proxy's ports](how-to/hostnames/proxy-ports.md) |
| `doctor`: `LANE has ports PORTS in a Windows reserved port range, which cannot be bound` | Windows reserved the range after the lane got its ports | [Windows](reference/windows.md#ports) |

## Hostnames and HTTPS

| What you see | Likely cause | Page |
|---|---|---|
| Over HTTPS the connection fails with no page, such as `curl: (35)` or `ERR_SSL_PROTOCOL_ERROR`, for a name no lane routes | The proxy refuses the TLS handshake for hostnames it does not route | [Hostnames and HTTPS](concepts/hostnames-and-https.md) |
| `ERR_CERT_AUTHORITY_INVALID` or another certificate error | This browser or tool does not trust the devlanes CA | [Trust the devlanes CA](how-to/hostnames/trust-the-ca.md) |
| `curl.exe` fails on Windows with a revocation error | The devlanes CA publishes no revocation list | [Windows](reference/windows.md#hostnames-and-https) |
| `doctor`: `CA not trusted by STORE` | `devlanes setup` has not run, or the store lost the CA | [Trust the devlanes CA](how-to/hostnames/trust-the-ca.md) |
| `HOST is outside the CA's names; HTTPS will fail` | A hostname lies outside the names the CA may sign | [Serve wildcard and tenant subdomains](how-to/hostnames/wildcards.md) |
| `HOST is routed by LANE and OTHER; LANE keeps it, so give one of them another hostname` | Two lanes claim the same hostname | [Give a service several ports and hostnames](how-to/services/ports-and-hostnames.md) |
| `hostname HOST is invalid` | A `hosts` entry is not a valid name | [Give a service several ports and hostnames](how-to/services/ports-and-hostnames.md) |
| A program cannot resolve a `*.localhost` name, such as Node's `ENOTFOUND` | Its resolver does not send `*.localhost` to loopback | [Fix programs that cannot resolve *.localhost](how-to/hostnames/resolve-localhost.md) |
| `doctor`: `a test lookup of a *.localhost name failed at daemon start, and devlanes does not write the hosts file on Windows` | Windows Server does not resolve `*.localhost` | [Fix programs that cannot resolve *.localhost](how-to/hostnames/resolve-localhost.md) |
| `doctor`: `.TLD lookups do not reach the devlanes DNS server` | The resolver for the configured TLD is missing | [Fix programs that cannot resolve *.localhost](how-to/hostnames/resolve-localhost.md) |
| `doctor`: `Group Policy sets NRPT rules on this machine, so Windows ignores the devlanes rule` | Group Policy overrides local name resolution rules | [Fix programs that cannot resolve *.localhost](how-to/hostnames/resolve-localhost.md) |
| A URL opens on port 8443 or 8080 | The proxy could not bind 443 or 80 | [Choose the proxy's ports](how-to/hostnames/proxy-ports.md) |
| A lane's cookies or CORS calls reach another lane | Origins or cookie domains are fixed in the app | [Set CORS and cookies for each lane](how-to/hostnames/cors-and-cookies.md) |

## Proxy error pages

| What you see | Likely cause | Page |
|---|---|---|
| [DL101 Unknown host](errors/unknown-host.md) | No lane routes the hostname | [Give a service several ports and hostnames](how-to/services/ports-and-hostnames.md) |
| [DL102 Invalid host](errors/invalid-host.md) | The request has no valid `Host` header | [DL102 Invalid host](errors/invalid-host.md) |
| [DL103 CONNECT not supported](errors/connect-unsupported.md) | A client sent a CONNECT request | [DL103 CONNECT not supported](errors/connect-unsupported.md) |
| [DL104 Proxy loop](errors/proxy-loop.md) | The service sends the request back to its own hostname | [Run Vite and Next.js](how-to/projects/vite-and-next.md) |
| [DL201 Service not running](errors/service-idle.md) | The service or its lane is stopped | [Start a service only on demand](how-to/services/on-demand.md) |
| [DL202 Service starting](errors/service-starting.md) | The service did not get ready in time | [Wait until a service is ready](how-to/services/ready-checks.md) |
| [DL203 Service failed](errors/service-failed.md) | The service exited or failed its ready check | [When a service fails](learn/failures.md) |
| [DL204 Box reconnecting](errors/box-reconnecting.md) | The connection to the box is down | [Remote connection states](reference/remote-states.md) |
| [DL205 Box sign-in needed](errors/box-sign-in-needed.md) | Reconnecting to the box needs you to sign in | [Connect to a box](how-to/remote/connect.md) |
| [DL206 Box pairing needed](errors/box-pairing-needed.md) | The box no longer accepts this machine's pairing | [Connect to a box](how-to/remote/connect.md) |
| [DL301 Service not listening](errors/upstream-unreachable.md) | Nothing listens on the service's port | [A service, a port and a hostname](learn/first-service.md) |
| [DL302 Upstream request failed](errors/upstream-failed.md) | The service closed the connection or did not answer with HTTP/1.1 | [DL302 Upstream request failed](errors/upstream-failed.md) |
| [DL303 Unexpected protocol upgrade](errors/unexpected-upgrade.md) | The service upgraded a request that did not ask for it | [DL303 Unexpected protocol upgrade](errors/unexpected-upgrade.md) |

## Project files

| What you see | Likely cause | Page |
|---|---|---|
| `no devlanes.yaml in DIR or its parents` | The command ran outside a project | [Add devlanes to a project](how-to/projects/add-devlanes.md) |
| An error about `devlanes.yaml` with the hint `devlanes config check` | The project file does not load | [Config files and reload](concepts/config-and-reload.md) |
| `FILE and OTHER both name project NAME; change project: in one of them` | Two project files of one repository name one project | [Split a monorepo into projects](how-to/projects/monorepo.md) |
| `project NAME is already used by OTHER; set another project: in this file` | Another repository uses the project name | [Lanes and projects](concepts/lanes-and-projects.md) |
| `LANE's project file now names project NAME` | The worktree checked out a file that names another project | [Remove lanes](how-to/lanes/clean-up.md) |
| `units UNIT and OTHER would share one log folder on Windows, which ignores case; rename one` | Two unit names differ only in letter case | [Windows](reference/windows.md#shells-and-commands-in-units) |
| `PATH is on the Windows drive DRIVE: file access is slow and file watching misses edits made from Windows` | Inside WSL, the worktree sits on a Windows drive | [Run devlanes in WSL](how-to/wsl.md) |

## Remote boxes

| What you see | Likely cause | Page |
|---|---|---|
| `needs_auth` in `remote ls`, or `box BOX needs you to sign in again` | A reconnect needs a password, a passphrase or a host key answer | [Remote connection states](reference/remote-states.md#connection-states) |
| `needs_pairing` in `remote ls`, or `box BOX needs a new pairing code` | The box has no pairing for this machine | [Connect to a box](how-to/remote/connect.md) |
| `the pairing code is wrong, expired or already used` | A typo, a code older than its lifetime, or a used code | [Connect to a box](how-to/remote/connect.md) |
| `pairing is paused for TIME after 5 wrong codes` | Too many wrong codes | [Remote connection states](reference/remote-states.md#pairing) |
| `this machine does not accept connections because remote.allow_connections is false` | `devlanes daemon pair` ran on a machine that turns clients away | [Prepare a box](how-to/remote/prepare-a-box.md) |
| `the box runs devlanes VERSION and this machine VERSION. Install the same release on both.` | The machines run different releases | [Update and uninstall](how-to/update.md) |
| `... but this client speaks protocol N. Install the same release on both machines, then run devlanes daemon restart on the box` | The releases speak different protocols, and the connection fails | [Update and uninstall](how-to/update.md) |
| `the box stops your processes at logout` | Linger is off for the Linux box's user | [Prepare a box](how-to/remote/prepare-a-box.md) |
| `the box daemon stops when this SSH session ends` | The Windows box's daemon runs inside the SSH session's job | [Prepare a box](how-to/remote/prepare-a-box.md) |
| `port PORT is not mirrored because ...` | The port is taken on this machine | [Remote connection states](reference/remote-states.md#warnings) |
| `not mirrored because ... is in use` on a lane in `remote ls` | Another lane serves the same hostnames | [Work with several boxes](how-to/remote/several-boxes.md) |
| `this machine cannot use the box's HTTPS port PORT (...)` | The box's proxy port is taken or not allowed here | [Remote connection states](reference/remote-states.md#warnings) |
| A port's `last_error` in `remote ls` | Connections to the port do not reach the box's service | [Remote connection states](reference/remote-states.md#mirrored-ports) |
| A box connects again after a daemon restart | The box stays wanted until `devlanes remote disconnect` | [Connect to a box](how-to/remote/connect.md) |

## Coding agents

| What you see | Likely cause | Page |
|---|---|---|
| `...; rerun with --yes` | The command asks first and no terminal can answer | [MCP tools](reference/mcp-tools.md#consent) |
| `devlanes COMMAND does not run here: ...` from the MCP server | The command needs a terminal | [MCP tools](reference/mcp-tools.md#refused-commands) |
| A tool call through MCP returns what is ready so far | The server's timeout for the host passed first | [MCP tools](reference/mcp-tools.md#timeouts) |
| A service started through MCP fails with `command not found on PATH` | The lane got the MCP server's environment, without your shell profile | [MCP tools](reference/mcp-tools.md#environment-of-a-lane-started-through-mcp) |
