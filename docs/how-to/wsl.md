---
title: Run devlanes in WSL
description: Run devlanes inside a WSL distribution and open its lanes in a browser on Windows.
kind: how-to
publish: false
---

Use this guide when your repositories live in a WSL 2 distribution and your browser runs on Windows. devlanes then runs in the distribution as it does on Linux, and Windows reaches its proxy over loopback.

## Install devlanes in the distribution

Install the Linux build inside a glibc distribution such as Ubuntu or Debian, with the script from [Install devlanes](../install.md). The same build notices that it runs in WSL, and the doctor then adds a wsl group with a fix under each problem it finds:

```console
$ devlanes doctor
```

Keep the repositories on the distribution's own filesystem, such as `~/src`, and not under `/mnt/c`. Files on a Windows drive are slower to read from the distribution, and file watchers miss the edits Windows programs make there, so a start in such a worktree warns. Editors on Windows still reach the Linux folder through WSL's network share or VS Code's WSL extension.

## Keep the daemon running

The daemon starts on your first command, inside the distribution. WSL stops a distribution some seconds after its last terminal closes, and the daemon and every lane stop with it. To keep the distribution running, set this in your Windows user folder:

```ini title="%UserProfile%\.wslconfig"
[general]
instanceIdleTimeout=-1
```

To start the daemon with the distribution, through systemd, install it as a user service:

```console
$ devlanes daemon install
```

If the distribution runs without systemd, the doctor says so. Turn it on:

```ini title="/etc/wsl.conf"
[boot]
systemd=true
```

WSL reads both files when it starts, so after changing either, shut it down from Windows and open the distribution again:

```console
PS> wsl.exe --shutdown
```

## Open lanes from Windows

Browsers on Windows resolve names under localhost to loopback, and WSL 2 forwards connections to Windows loopback into the distribution, so a lane URL opened on Windows reaches the proxy. `devlanes url open` opens the Windows default browser.

A user in the distribution cannot bind ports 443 and 80, so the proxy falls back to 8443 and 8080 and lane URLs carry the port, such as https://myapp.localhost:8443. [Choose the proxy's ports](hostnames/proxy-ports.md) shows how to let the daemon bind them. Keep the names under localhost: Windows programs read neither the distribution's hosts file nor its resolver, so neither fix from [Fix programs that cannot resolve *.localhost](hostnames/resolve-localhost.md) reaches them.

## Trust the CA on Windows

devlanes signs lane certificates with the devlanes CA, a certificate authority devlanes creates on your machine, which here means inside the distribution. Browsers on Windows read the Windows store, not the distribution's, so add the Windows store to the stores the setup command fills, in the distribution's user config:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  trust: [system, windows]
```

Then run the setup command in the distribution. It changes the Windows store through Windows' own tools, and Windows shows a security warning on the desktop that you confirm with Yes.

```console
$ devlanes setup
```

The doctor's tls group then lists the windows store, and its wsl group warns while the list leaves that store out. [Trust the devlanes CA](hostnames/trust-the-ca.md) covers runtimes and other tools.

## If you use Docker Desktop

Turn on its WSL integration for the distribution under Settings, Resources, WSL integration. That puts Docker into the distribution, and containers work as on Linux.

## If Windows interop is off

devlanes then cannot open URLs in Windows or add the CA to the Windows store, and the doctor says so. Turn interop back on, then shut WSL down from Windows as above:

```ini title="/etc/wsl.conf"
[interop]
enabled=true
```

## If you would rather run devlanes on Windows

Install devlanes on Windows as well and connect to the distribution as a box: the daemon on Windows reaches the one in the distribution through `wsl.exe` and mirrors its lanes. The hostnames then open on Windows with certificates from the CA on Windows, and the lanes keep the ports they have in the distribution. Both sides need the same release.

```yaml devlanes-user
# %USERPROFILE%\.devlanes\config.yaml
remote:
  boxes:
    ubuntu:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/you/.local/bin/devlanes, tunnel, --stdio]
```

Windows mirrors each port of the box onto its own loopback, and also listens on the HTTPS port of the box's proxy, 8443 in a distribution. WSL by default forwards every port a program in the distribution listens on to Windows loopback as well. A port that is already taken on Windows is not mirrored, and `devlanes remote ls` warns about it. WSL's `localhostForwarding` setting turns that forwarding off, and WSL ignores the setting in its mirrored networking mode.

```ini title="%UserProfile%\.wslconfig"
[wsl2]
localhostForwarding=false
```

[Connect to a box without SSH](remote/connect-by-command.md) has the pairing step.

## Related

- [Install devlanes](../install.md)
- [Choose the proxy's ports](hostnames/proxy-ports.md)
- [Trust the devlanes CA](hostnames/trust-the-ca.md)
- [Windows](../reference/windows.md)
