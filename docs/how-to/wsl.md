---
title: Run devlanes in WSL
description: Run devlanes inside a WSL distribution and open its lanes in a browser on Windows.
kind: how-to
---

Use this guide when your repositories live in a WSL distribution and your browser runs on Windows. devlanes then runs in the distribution as it does on Linux, and Windows reaches its proxy over loopback.

## Install devlanes in the distribution

Install the Linux build inside a glibc distribution such as Ubuntu or Debian, with the script from [Install devlanes](../install.md). The same build notices that it runs in WSL, and the doctor command then adds a group about the distribution with a fix under each problem it finds.

Keep the repositories on the distribution's own filesystem, such as `~/src`, and not under `/mnt/c`. Files on a Windows drive are slower to read from the distribution, and edits that Windows programs make there go unnoticed by file watchers, so a start in such a worktree warns. Editors on Windows still reach the Linux folder through WSL's network share or VS Code's WSL extension.

## Keep the daemon running

The daemon starts on your first command, inside the distribution. WSL stops a distribution some seconds after its last terminal closes, and the daemon and every lane stop with it. To keep the distribution running, set this in `%UserProfile%\.wslconfig` on Windows:

```ini title="%UserProfile%\.wslconfig"
[general]
instanceIdleTimeout=-1
```

`devlanes daemon install` starts the daemon with the distribution, through systemd. If the distribution runs without systemd, turn it on, then run `wsl.exe --shutdown` from Windows and open the distribution again:

```ini title="/etc/wsl.conf"
[boot]
systemd=true
```

## Open lanes from Windows

Browsers on Windows resolve names under localhost to loopback, and WSL forwards loopback connections from Windows into the distribution, so a lane URL opened on Windows reaches the proxy. `devlanes url open` opens the Windows default browser.

A user in the distribution cannot bind ports 443 and 80, so the proxy falls back to 8443 and 8080 and lane URLs carry the port, such as https://myapp.localhost:8443. [Choose the proxy's ports](hostnames/proxy-ports.md) shows how to let the daemon bind them. Keep the names under localhost: Windows programs read neither the distribution's hosts file nor its resolver, so neither fix from [Fix programs that cannot resolve *.localhost](hostnames/resolve-localhost.md) reaches them.

## Trust the CA on Windows

Browsers on Windows read the Windows certificate store, not the distribution's. Add the Windows store to the stores the setup command fills, in the distribution's user config:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  trust: [system, windows]
```

Then run `devlanes setup` in the distribution. It changes the Windows store through Windows' own tools, and Windows shows a security warning on the desktop that you confirm with Yes. The doctor then reports the authority as trusted by windows, and warns while the store is missing from the list. [Trust the devlanes CA](hostnames/trust-the-ca.md) covers runtimes and other tools.

## If you use Docker Desktop

Turn on its WSL integration for the distribution under Settings, Resources, WSL integration. That puts Docker into the distribution, and containers work as on Linux.

## If Windows interop is off

devlanes then cannot open URLs in Windows or add the authority to the Windows store, and the doctor says so. Turn interop back on, then shut WSL down from Windows as above:

```ini title="/etc/wsl.conf"
[interop]
enabled=true
```

## If you would rather run devlanes on Windows

Install devlanes on Windows as well and treat the distribution as a box: the daemon on Windows connects to the one in the distribution through `wsl.exe` and mirrors its lanes, so hostnames, ports and the authority all belong to Windows. Both sides need the same release.

```yaml devlanes-user
# %USERPROFILE%\.devlanes\config.yaml
remote:
  boxes:
    ubuntu:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/you/.local/bin/devlanes, tunnel, --stdio]
```

[Connect to a box without SSH](remote/connect-by-command.md) has the pairing step and the ports WSL forwards on its own.

## Related

- [Install devlanes](../install.md)
- [Choose the proxy's ports](hostnames/proxy-ports.md)
- [Trust the devlanes CA](hostnames/trust-the-ca.md)
- [Windows](../reference/windows.md)
