# Remote setup

A box runs its own devlanes and its own lanes. Your machine connects over SSH and gets the box's hostnames and ports locally, nothing else: lanes are started and stopped on the box, and the connection never carries your environment or secrets there. [Remote boxes](../remote.md) describes mirroring once connected; this page is about getting both machines ready.

## On the box

devlanes on the box is an ordinary install. It needs:

- Linux with glibc 2.35 or newer (Debian 12, Ubuntu 22.04 and later, Fedora), x86_64 or arm64; or macOS; or Windows 11 24H2 or later (see [Windows boxes](#windows-boxes)). Alpine and other musl systems have no release build.
- `git`, and Docker if the project has `infra`.
- The `devlanes` binary on the `PATH` that a non-interactive ssh session gets, or at `~/.devlanes/bin/devlanes`. `remote connect` finds it with `sh -c 'command -v devlanes'`, which does not read `.bashrc` or `.zshrc`. On a Windows box it asks PowerShell (`Get-Command`), with the `PATH` sshd builds from the registry.

Install the same release as on the client, from an ssh shell on the box. `install.sh` can put it in `~/.devlanes/bin`, where `remote connect` always looks; replace `X.Y.Z` with the version `devlanes --version` prints on the client:

```sh
curl -fsSL https://devlanes.run/install.sh | sh -s -- --version X.Y.Z --dir ~/.devlanes/bin
```

Without `--dir` it installs to `~/.local/bin`, which a non-interactive ssh session may not have on its `PATH`; check with `ssh hetzner 'command -v devlanes'` from the client. Homebrew, npm and cargo binstall work too (the [README](../../README.md) lists them). When their directory is not on that `PATH`, as with Homebrew's `/opt/homebrew/bin` on macOS, link the binary into `~/.devlanes/bin` from an interactive shell on the box:

```sh
mkdir -p ~/.devlanes/bin && ln -sf "$(command -v devlanes)" ~/.devlanes/bin/devlanes
```

`remote connect` never installs or upgrades devlanes on a box. Without a binary the first connection fails with `devlanes is not installed on hetzner` and stops retrying.

Then, on the box:

```sh
ssh hetzner
git clone git@github.com:acme/acme.git && cd acme
devlanes start                      # or devlanes start --no-ui
devlanes certs trust                # optional: for curl and browsers running on the box itself
```

Services on the box must listen on loopback or a wildcard address. The box dials `127.0.0.1:<port>` for the mirror, as the proxy does locally.

### Keep the box daemon alive

The daemon starts on the first command and detaches from the ssh session, so it survives logging out. What kills it is systemd's cleanup of your user session on Linux. Enable linger once:

```sh
sudo loginctl enable-linger "$USER"
```

Without it, `devlanes remote ls` on the client warns `linger off` and the lanes vanish when your last ssh session ends. `devlanes daemon install` on the box adds a systemd user unit that restarts the daemon at boot, which is useful on top of linger but does not replace it: a user unit is stopped with the session too when linger is off.

On a macOS box there is no linger. The daemon keeps running after the ssh session closes. `devlanes daemon install` writes a LaunchAgent for login; it needs a GUI login session to load, so run it in a session where you are logged in on the console as well.

A client that connects while no daemon runs on the box starts one there. To keep the box daemon under your own control, set this in the box's `~/.devlanes/config.yaml`:

```yaml
remote:
  start_daemon: false
```

A client that connects while the daemon is stopped then shows `the devlanes daemon is not running on hetzner` in `remote ls` and keeps retrying. It connects once you run `devlanes daemon start` on the box. A daemon started on demand like that exits about 30 seconds after the last client or TUI leaves while nothing runs, so run `devlanes daemon install` to keep it running, or set `daemon.exit_when_idle: false` in the same file. An invalid value under `remote` counts as `false`. The setting is read at each connection, so it needs no restart.

### Pairing clients

The box serves only machines it paired with. Before a machine's first connection, get a one-time code on the box:

```sh
devlanes daemon pair
# pairing code: K7QM-3XWD (works once, for 10 minutes)
# on the machine you connect from, run: devlanes remote connect <box> --code K7QM-3XWD
```

The client enters it once ([first connection](#first-connection)) and connects with a token from then on, one per box name it connects by. `devlanes daemon clients` lists the paired machines and the ones connected now; `devlanes daemon revoke <name>` removes one and cuts its connection. The daemon page of the TUI on the box (`D`) shows the same list, and `x` there revokes the selected client. A wrong code voids every code you handed out, and five wrong codes within 10 minutes pause pairing for up to 10 minutes. To let every machine that reaches the box over SSH connect without a code, set this in the box's `~/.devlanes/config.yaml`:

```yaml
remote:
  require_pairing: false
```

A problem anywhere under `remote` turns `require_pairing` on (and `start_daemon` off). The setting is read at each connection, so it needs no restart. [Pairing](../remote.md#pairing) has the details.

### PATH for Docker and tools

The box daemon inherits the environment of whatever started it. Over ssh that is the non-interactive `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin` on macOS, the sshd default on Linux), and Compose, `psql`, `aws` and the rest are looked up there. Homebrew's `/opt/homebrew/bin` is not in it. Either put additions in a file the non-interactive shell reads (`~/.zshenv` for zsh, `~/.ssh/environment` with `PermitUserEnvironment`, or `/etc/environment`), or start the daemon from an interactive shell once (`devlanes daemon restart`), or install it as a service with the right `PATH` in the unit.

### Hidden units

```yaml
services:
  admin:
    run: ./admin
    hidden: true
infra:
  hidden: [postgres]
```

Hidden units stay on the box: clients do not see them, their ports are not mirrored, the box refuses streams to them and their `sync` folders are not synced. Everything else that is running or starting is mirrored.

### Ports outside the config

A server you run on the box outside devlanes, such as `python -m http.server 8000` in an ssh shell, is not mirrored until the box's config declares it. Add it to `devlanes.local.yaml` next to the project's `devlanes.yaml` on the box:

```yaml
infra:
  external:
    files:
      address: 127.0.0.1:8000
      hosts: "files.${{ lane.domain }}"
```

The box's lanes then list the unit `files`, and clients mirror port 8000 and `https://files.<lane domain>` while a lane of the project runs. [External ports](../remote.md#external-ports) has the details.

### URLs to give out

On the box, `devlanes url print web` prints the box's URL and, in a terminal, `on <client>: <url>` for each connected client, the URL that opens there, or why the unit does not open there (a port taken on that machine, a lane it skips). Agents working on the box read the same from `devlanes url print web --json` under `clients`; the bundled devlanes skill tells them to give you the client's URL. When no client is connected, an agent runs `devlanes daemon pair` on the box and asks you to run `devlanes remote connect <box>` on your machine with the code it got; a machine that is paired already needs no code.

### Windows boxes

A Windows machine is a box through Windows' own OpenSSH Server, an optional feature. In an elevated PowerShell on the box:

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Set-Service sshd -StartupType Automatic
Start-Service sshd
Get-NetFirewallRule -Name OpenSSH-Server-In-TCP   # the feature adds this rule for port 22
```

If that rule is missing, add it with `New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22`. Only the ssh port needs a rule: mirrored ports and synced files travel inside the ssh connection. Install devlanes as the user you log in as, as in the [Windows guide](windows.md#install). `install.ps1` puts `%LOCALAPPDATA%\Programs\devlanes` on your user `PATH`, which sshd reads from the registry for every session, so `remote connect` finds it; it also looks for `%USERPROFILE%\.devlanes\bin\devlanes.exe` (`-Dir $env:USERPROFILE\.devlanes\bin`). Log in with a standard account rather than an administrator: an administrator's ssh session is elevated, so its daemon would run every unit as administrator, which `devlanes doctor` warns about. For key authentication, sshd reads a standard account's keys from `%USERPROFILE%\.ssh\authorized_keys` and every administrator's from `C:\ProgramData\ssh\administrators_authorized_keys`.

sshd runs commands with `cmd.exe` unless the `DefaultShell` value under `HKLM:\SOFTWARE\OpenSSH` names another shell. `remote connect` works with cmd.exe, Windows PowerShell, PowerShell 7 and Git Bash: when its `sh` probe finds no `sh`, it asks PowerShell for devlanes and the shell, and quotes the tunnel command for that shell. A Windows client therefore authenticates twice on its first connection to a Windows box after its daemon starts, and once from then on. With cmd.exe as the shell, the path to devlanes must not hold `%` or `!`, which cmd.exe would expand. On a locked-down box where PowerShell runs in Constrained Language Mode, or security software blocks `powershell -EncodedCommand`, that probe gets no answer, and `remote connect` fails with `unexpected probe output from the box`, followed by what PowerShell printed on stderr. Such a box can still be reached as a [box reached by a command](remote-command.md) that names devlanes itself, such as `[ssh, acme-box, C:\Users\acme\.devlanes\bin\devlanes.exe, tunnel, --stdio]`.

The daemon a connection starts leaves the ssh session and keeps running after it ends. It runs in the ssh server's background session, not on the desktop, even when you are signed in there, so a unit that opens a window shows nothing, and `devlanes certs trust` there fails as it does over any ssh session; the client serves the box's hostnames with its own CA, so the box needs no trust. A daemon already started on the desktop (for example by `devlanes daemon install`) is used instead. With key authentication the daemon started over ssh has no network credentials, so units cannot reach SMB shares or use credentials from the Windows Credential Manager; when that matters, `devlanes daemon restart` from the desktop moves the daemon there. `devlanes daemon install` starts the daemon when you sign in on the desktop, not when the machine boots. A box nobody signs in to gets its daemon from the first connection or the first `devlanes` command over ssh, or from a Task Scheduler task that runs `devlanes.exe daemon run` as you at startup. When an ssh server other than Windows' own keeps the daemon inside the session, `remote ls` warns `the box's daemon was started inside this connection's ssh session`; start the daemon in one of those ways instead.

A project with `infra` needs Docker Desktop running, which starts only when you sign in on the desktop; over ssh `docker` reaches it while it runs.

Synced folders come from a Windows box with `/` in their paths and the names as the box stores them. Windows keeps only a read-only attribute, so the box sends new files as `0o644`, or `0o444` when read-only, and leaves the exec bits of files the client already holds alone.

## On the client

The client runs macOS, Linux or Windows (see [Windows clients](#windows-clients)).

- A `Host` entry in `~/.ssh/config`. devlanes runs the system `ssh`, so `ProxyJump`, `ProxyCommand`, certificates, `IdentityFile` and `User` apply. The name you give `remote connect` is that `Host`. Boxes on a tailnet or behind Cloudflare Zero Trust have their own pages: [Tailscale](remote-tailscale.md) and [Cloudflare](remote-cloudflare.md). A container, pod, WSL distribution or codespace without SSH is reached by a command instead; see [boxes reached by a command](remote-command.md).
- Your key in an agent. The ssh master is a child of the local daemon and uses the agent the daemon was started with; a daemon spawned from your shell inherits that shell's `SSH_AUTH_SOCK`. If the agent socket changes (new login, 1Password restart), `devlanes daemon restart` picks up the new one.
- `devlanes certs trust`, once. The box's hostnames are served by your local proxy with your local CA.
- A different `ports.range` than the box in `~/.devlanes/config.yaml`, so lanes created on both machines before they first connected do not overlap. Overlapping ports are reported, not mirrored, and the local allocator avoids the box's ports from then on. While the ranges overlap, every connection warns about it. With several boxes, give each box its own range as well.

```sh
devlanes remote connect hetzner             # stays attached: shows state, relays ssh prompts
devlanes remote connect hetzner --background
devlanes remote ls                          # state, mirrored ports, skipped lanes, warnings
devlanes remote disconnect hetzner
```

Passphrase, one-time-password and host-key prompts appear in the terminal running `remote connect`, prefixed with the box name; type `yes` to a host-key question even though the input is hidden.

### First connection

The first connection to a box pairs this machine with it. Run `devlanes daemon pair` on the box ([pairing clients](#pairing-clients)), then either pass the code or type it when asked:

```sh
devlanes remote connect hetzner --code K7QM-3XWD
devlanes remote connect hetzner
# [hetzner] the box serves only paired clients; pairing code from `devlanes daemon pair` on the box: K7QM-3XWD
```

Case and the dash do not matter. `--background` asks for the code the same way before it returns; without a terminal, as for an agent, pass `--code`. The box gives this machine a token, kept in its state database, that every later connection presents, also the ones the daemon makes on its own after sleep or a restart. Without a code, as in a script with no terminal, or with a wrong one, the connection waits in `needs_pairing` and `remote connect` says why; run it again with a new code. The same happens when the box revoked this machine's pairing: its ports stay bound and its hostnames answer with a [pairing needed](../errors/box-pairing-needed.md) page until you pair again.

The connection belongs to your local daemon, not to the terminal: Ctrl-C detaches, the daemon reconnects with backoff after sleep or network loss and keeps the mirrored ports bound meanwhile. It also survives a restart of the local daemon (`devlanes daemon restart`, a reboot with `devlanes daemon install`, an upgrade): the new daemon connects every box that connected since you last ran `remote disconnect` for it, in the background. It keeps retrying such a box even when the box is gone for good or its name no longer resolves; only `remote disconnect` ends that.

### Unattended reconnects

Reconnects after sleep, after a network change and after a daemon restart usually run with no terminal attached. Keys in an agent, or a key without a passphrase, let them run without a prompt. When a reconnect needs you anyway, because ssh asks for a password, a passphrase or a new host key, or rejects credentials that worked before, the box waits instead of retrying:

```sh
devlanes remote ls
# items:
#   state: needs_auth
#   error: ssh to hetzner failed (255): … Permission denied (publickey).
#   host: hetzner
#   fix: devlanes remote connect hetzner
devlanes remote connect hetzner     # shows SSH prompts in this terminal, then reconnects
```

While it waits and the daemon keeps running, the box's ports stay bound and its hostnames answer with a [sign-in needed](../errors/box-sign-in-needed.md) page; after a daemon restart they appear once the box connects. `status --json` lists it under `hosts` with the same `fix`. A box that never connected is different: on the first `remote connect`, rejected credentials fail it for good, unless ssh asked something nobody answered.

### Slow links

devlanes compresses synced files itself: the box sends their contents with zstd wherever that makes them smaller. Mirrored connections (HTTP, database and other TCP traffic) cross ssh as they are. On a slow or high-latency link, turn on ssh's own compression in the box's `Host` block of `~/.ssh/config`:

```text
Host hetzner
  HostName 203.0.113.10
  Compression yes
```

devlanes doesn't set it for you, because options on the `ssh` command line would override what your `~/.ssh/config` says for that host. Compression costs CPU on both machines and gains nothing on a fast link, so leave it off there. It applies from the next ssh connection to the box: run `devlanes remote disconnect hetzner`, then `remote connect` again.

### Several boxes

Connect to each box by its own name. Give every machine its own `ports.range`, for example `20000-24999` on the client, `25000-29999` on one box and `30000-32767` on the other, then `devlanes daemon restart` on each; lanes that already have ports keep them until `devlanes lane clean`. When two boxes run the same project, the box you connected first keeps the shared hostnames, and the other box's lane is listed under `skipped_lanes` in `remote ls` until the first one stops serving them or disconnects. When the ranges of two connected boxes overlap, `remote ls` shows a warning on both while they do.

### Box proxy on another port

A box whose proxy could not bind 443 (a VPS where unprivileged users cannot bind low ports) serves HTTPS on 8443 and puts that port in its URLs. The client then listens on 8443 too, on loopback only, and serves that box's hostnames there, so `https://web.acme.localhost:8443` opens unchanged. Nothing to configure. When the client cannot bind that port, because something else holds it or it is a low port the client's user may not bind, `remote ls` says so; the box's hostnames still open on the client's own HTTPS port.

### Windows clients

devlanes uses Windows' own OpenSSH client (`C:\Windows\System32\OpenSSH\ssh.exe`, the "OpenSSH Client" optional feature), or the first `ssh.exe` on `PATH` when that is not installed. It reads `%USERPROFILE%\.ssh\config` and `known_hosts` like any `ssh` you run yourself.

Windows' OpenSSH has no ControlMaster, so every connection and every reconnect authenticates again. Keep the key in the ssh-agent service so reconnects need no prompt. Once, in an elevated PowerShell:

```powershell
Get-Service ssh-agent | Set-Service -StartupType Automatic
Start-Service ssh-agent
```

Then, as yourself, `ssh-add $env:USERPROFILE\.ssh\id_ed25519`. The agent keeps the key across logins, and the daemon finds it without `SSH_AUTH_SOCK`. Passphrase and host-key prompts still appear in the terminal running `remote connect` (PowerShell, cmd or Windows Terminal; Git Bash's mintty has no console to prompt in).

Synced folders land under `%USERPROFILE%\.devlanes\sync`. Windows keeps only a read-only attribute, so exec bits are not synced. Files whose names Windows cannot hold (`con.txt`, `a:b`, a name ending in a dot or space) and the second of two names that differ only in case (`README.md` and `Readme.md`) are skipped and listed in `remote sync`. A box port inside a Windows reserved port range (`netsh int ipv4 show excludedportrange protocol=tcp`) cannot be mirrored and is reported with that hint.

## Checking

```sh
devlanes remote ls                                  # connected, reconnecting, needs_auth or needs_pairing (with the error), failed
devlanes daemon clients                             # on the box: paired machines and the ones connected now
devlanes url ls                                     # the box's hostnames next to your local ones
devlanes status --json | jq .clients                # on the box: who is connected and what each mirrors
devlanes url print web                              # on the box: the URL each connected client opens
curl -sS https://api.acme.localhost/healthz          # from the client, once the box lane is ready
```

| Message | Cause | Fix |
| --- | --- | --- |
| `devlanes is not installed on hetzner` | not on the ssh `PATH` or in `~/.devlanes/bin` (`%USERPROFILE%\.devlanes\bin` on Windows) | install it on the box, or move the binary there, then connect again |
| `the devlanes daemon is not running on hetzner` | `remote.start_daemon: false` on the box and its daemon is stopped | `devlanes daemon start` on the box, or `devlanes daemon install` to keep it running; the client keeps retrying |
| `hetzner: a connection to port 20012 (acme@0:api) failed: the unit is not running on the box` | an `open_failed` warning; `remote ls` shows the same as the port's `last_error` | start the unit on the box |
| `… failed: nothing on the box accepted it: connection refused` | the unit runs but does not listen on the box's loopback | make it listen on `127.0.0.1`, `::1` or a wildcard address |
| `… failed: the box did not answer in time` or `the link dropped while it opened` | a slow or dropped link | try again; `remote ls` shows the link state |
| `… failed: box hetzner is not connected: …` | the link stayed down for 10 seconds | the error after the colon says why |
| `the box runs devlanes 0.4.1 and this machine 0.4.3` | different releases, same protocol | works; upgrade the box when convenient |
| `the box runs … (protocol 5) but this client speaks protocol 4` | releases too far apart | install the same release on both machines, then `devlanes daemon restart` on the box |
| `` `devlanes tunnel --stdio` did not answer within 20s; the box may run an older devlanes release (protocol below 4) `` | this client is newer than the box: the box's devlanes predates protocol 4, whose tunnel announces itself first | install the same release on both machines |
| a mux or handshake error right after one machine was upgraded | this client predates protocol 4 and the box is newer: the client cannot read the line the box's tunnel starts with | install the same release on both machines |
| ``the box daemon runs an older devlanes than `devlanes tunnel` `` | the box's binary was upgraded but its daemon still runs the old one | `devlanes daemon restart` on the box |
| ``the box closed the connection before `devlanes tunnel --stdio` started; the box printed: …`` | the login shell exited or ran something else; its output follows | fix the shell's startup files, or the `ForceCommand` or `RemoteCommand` for this host |
| ``the box printed more than 64 KiB before `devlanes tunnel --stdio` started`` | a profile file prints a lot on every login | make it quiet for non-interactive sessions |
| `port 5432 is in use on this machine by postgres (pid 812); not mirrored` | a local process holds the port | stop it, or mark the box container `hidden` |
| `lane acme@1 is not mirrored; this machine already serves acme-1.localhost` | a local lane of the same project uses that hostname | stop the local lane, or give one side another `project` name |
| `lane acme@0 is not mirrored; box hetzner, connected earlier, already serves acme.localhost` | two connected boxes serve the same hostname | disconnect the other box or stop that lane there; the lane is mirrored once the name is free |
| `port 20012 is in use on this machine by box hetzner (acme@0:api); not mirrored` | another connected box's mirror holds the port | give the boxes different `ports.range` values |
| `the box's ports.range 20000-32767 overlaps that of box hetzner (20000-32767), so …` | two connected boxes hand out ports from the same pool | set another `ports.range` in `~/.devlanes/config.yaml` on one box, then `devlanes daemon restart` there |
| `the box's URLs use HTTPS port 8443, which this machine cannot serve: in use by …; open its hostnames on port 443 instead` | another process holds the box's proxy port on the client | stop that process; the client tries the port again every 30 seconds, or at once on `devlanes remote connect` for the box. Until then use the client's own HTTPS port |
| `the box's URLs use HTTPS port 443, which this machine cannot serve: permission denied (low ports may need privileges); …` | the box's proxy runs on a low port that the client's user may not bind (on macOS the extra port binds loopback only, which needs root below 1024) | on Linux, allow low ports as [Ports 443 and 80](https.md#ports-443-and-80) describes, then `devlanes daemon restart`; otherwise use the client's own HTTPS port |
| `linger off` | Linux box without `loginctl enable-linger` | run it on the box |
| `the box's daemon was started inside this connection's ssh session and stops when it ends` | a Windows box whose ssh server does not let the daemon leave the session | start the daemon outside ssh, as [Windows boxes](#windows-boxes) describes |
| `devlanes on hetzner is C:\…\devlanes.exe, a path cmd.exe cannot run as a command` | the box's shell is cmd.exe and the path holds `%` or `!` | install devlanes in another folder |
| `ssh to hetzner failed (255): Permission denied` | no agent or key for the daemon's ssh | load the key, `devlanes daemon restart`, connect again |
| `needs_auth` in `remote ls`, a sign-in needed page in the browser | a reconnect asked for a password, passphrase or host key with no `remote connect` attached, or ssh rejected credentials that worked before | `devlanes remote connect hetzner` in a terminal; load the key into an agent so later reconnects need no prompt |
| `box hetzner: the box serves only paired clients; get a code with devlanes daemon pair on the box` | the first connection, with no code given or typed | `devlanes daemon pair` on the box, then `devlanes remote connect hetzner --code <code>` |
| `box hetzner: the box no longer accepts this machine's pairing (it was revoked, …)` | `devlanes daemon revoke` or `devlanes daemon reset-state` on the box, or the box dropped this pairing as the oldest of 8 | pair again with a new code from `devlanes daemon pair` |
| `… the box refused the pairing code: the pairing code is wrong, expired or already used …` | a typo, a code older than 10 minutes or one used before; every outstanding code is void now | a new code from `devlanes daemon pair` |
| `… the box refused the pairing code: 5 wrong pairing codes in the last 10 minutes; the box takes codes again in 7 minutes` | five wrong codes within 10 minutes; `daemon pair` on the box refuses with the same wait | wait, then use a new code; the box logs the refused attempts with their client id and SSH address |
| `box hetzner needs you to sign in again: … Host key verification failed` | the box's host key changed | check the new key, `ssh-keygen -R hetzner`, then `devlanes remote connect hetzner` |
| a box connects again after a daemon restart | you connected it and have not run `remote disconnect` for it since | `devlanes remote disconnect hetzner` when you no longer want it |
| `no ssh client found` | Windows without OpenSSH Client | add the optional feature, or put `ssh.exe` on `PATH` |

## Limits

TCP only: UDP ports are not mirrored. The connection is client to box only; the box never dials you. Commands are not forwarded (`devlanes --host`), so manage lanes in an ssh shell or an editor's remote terminal on the box.
