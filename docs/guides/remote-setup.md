# Remote setup

A box runs its own devlanes and its own lanes. Your machine connects over SSH and gets the box's hostnames and ports locally, nothing else: lanes are started and stopped on the box, and the connection never carries your environment or secrets there. [Remote boxes](../remote.md) describes mirroring once connected; this page is about getting both machines ready.

## On the box

devlanes on the box is an ordinary install. It needs:

- Linux with glibc 2.35 or newer (Debian 12, Ubuntu 22.04 and later, Fedora), x86_64 or arm64; or macOS; or Windows 11 24H2 or later (see [Windows boxes](#windows-boxes)). Alpine and other musl systems have no release build.
- `git`, and Docker if the project has `infra`.
- The `devlanes` binary on the `PATH` that a non-interactive ssh session gets, or at `~/.devlanes/bin/devlanes`. `remote connect` finds it with `sh -c 'command -v devlanes'`, which does not read `.bashrc` or `.zshrc`.

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

Hidden units stay on the box: their ports are not mirrored and the box refuses streams to them. Everything else that is running or starting is mirrored.

### Windows boxes

A Windows machine is a box through Windows' own OpenSSH Server, an optional feature. In an elevated PowerShell on the box:

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Set-Service sshd -StartupType Automatic
Start-Service sshd
Get-NetFirewallRule -Name OpenSSH-Server-In-TCP   # the feature adds this rule for port 22
```

If that rule is missing, add it with `New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22`. Only the ssh port needs a rule: mirrored ports and synced files travel inside the ssh connection. Install devlanes as the user you log in as, as in the [Windows guide](windows.md#install). `install.ps1` puts `%LOCALAPPDATA%\Programs\devlanes` on your user `PATH`, which sshd reads from the registry for every session, so `remote connect` finds it; it also looks for `%USERPROFILE%\.devlanes\bin\devlanes.exe` (`-Dir $env:USERPROFILE\.devlanes\bin`). Log in with a standard account rather than an administrator: an administrator's ssh session is elevated, so its daemon would run every unit as administrator, which `devlanes doctor` warns about.

sshd runs commands with `cmd.exe` unless the `DefaultShell` registry value names another shell. `remote connect` works with cmd.exe, Windows PowerShell, PowerShell 7 and Git Bash: when its `sh` probe finds no `sh`, it asks PowerShell for devlanes and the shell, and quotes the tunnel command for that shell. A Windows client therefore authenticates twice the first time it connects to a Windows box, and once from then on. With cmd.exe as the shell, the path to devlanes must not hold `%`, which cmd.exe would expand.

The daemon a connection starts leaves the ssh session and keeps running after it ends. Without anyone signed in on the box's desktop it runs in the background session, so a unit that opens a window shows nothing, and `devlanes certs trust` there fails as it does over any ssh session; the client serves the box's hostnames with its own CA, so the box needs no trust. `devlanes daemon install` starts the daemon when you sign in on the desktop, not when the machine boots. A box nobody signs in to gets its daemon from the first connection or the first `devlanes` command over ssh, or from a Task Scheduler task that runs `devlanes.exe daemon run` as you at startup. When an ssh server other than Windows' own keeps the daemon inside the session, `remote ls` warns `the box's daemon was started inside this connection's ssh session`; start the daemon in one of those ways instead.

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
devlanes remote ls                                  # connected, reconnecting or needs_auth (with the error), failed
devlanes url ls                                     # the box's hostnames next to your local ones
devlanes status --json | jq .clients                # on the box: who is connected
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
| `devlanes on hetzner is C:\…\devlanes.exe, a path cmd.exe cannot run as a command` | the box's shell is cmd.exe and the path holds `%` | install devlanes in another folder |
| `ssh to hetzner failed (255): Permission denied` | no agent or key for the daemon's ssh | load the key, `devlanes daemon restart`, connect again |
| `needs_auth` in `remote ls`, a sign-in needed page in the browser | a reconnect asked for a password, passphrase or host key with no `remote connect` attached, or ssh rejected credentials that worked before | `devlanes remote connect hetzner` in a terminal; load the key into an agent so later reconnects need no prompt |
| `box hetzner needs you to sign in again: … Host key verification failed` | the box's host key changed | check the new key, `ssh-keygen -R hetzner`, then `devlanes remote connect hetzner` |
| a box connects again after a daemon restart | you connected it and have not run `remote disconnect` for it since | `devlanes remote disconnect hetzner` when you no longer want it |
| `no ssh client found` | Windows without OpenSSH Client | add the optional feature, or put `ssh.exe` on `PATH` |

## Limits

TCP only: UDP ports are not mirrored. The connection is client to box only; the box never dials you. Commands are not forwarded (`devlanes --host`), so manage lanes in an ssh shell or an editor's remote terminal on the box.
