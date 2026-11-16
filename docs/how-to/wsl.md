---
title: Run devlanes in WSL
description: Run devlanes inside a WSL distribution and open its lanes in a browser on Windows.
kind: how-to
publish: false
---

Use this guide when your repositories live in a WSL 2 distribution and your browser runs on Windows. devlanes runs in the distribution as on Linux, and Windows reaches its lanes over localhost.

## Install devlanes in the distribution

Install the Linux build in the distribution with the script from [Install devlanes](../install.md), then check it. Inside WSL the doctor adds a wsl group with a fix for each problem.

```console
$ devlanes doctor
```

Keep your repositories on the distribution's own filesystem, such as `~/src`, not under `/mnt/c`, where file access is slow and edits from Windows go unnoticed.

## Keep the distribution running

WSL stops a distribution soon after its last terminal closes, and your lanes with it. To keep it running, add this to the file in your Windows user folder, then restart WSL from Windows:

```ini title="%UserProfile%\.wslconfig"
[general]
instanceIdleTimeout=-1
```

```console
PS> wsl.exe --shutdown
```

## Trust the CA on Windows

Browsers on Windows read the Windows store, so add it to the stores the setup command fills with the devlanes CA, a certificate authority devlanes creates on your machine:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  trust: [system, windows]
```

Run the setup command in the distribution and confirm the security warning Windows shows:

```console
$ devlanes setup
```

## Open a lane from Windows

Start a lane in the distribution and open its URL in your Windows browser. The proxy in the distribution cannot have ports 443 and 80, so lane URLs carry 8443, such as `https://myapp.localhost:8443`; [Choose the proxy's ports](hostnames/proxy-ports.md) changes that.

```console
$ devlanes start
$ devlanes url open
```

## If you use Docker Desktop

Turn on its WSL integration for the distribution, under Settings, Resources, WSL integration.

## If you would rather run devlanes on Windows

Install devlanes on Windows too and connect to the distribution as a box, a machine whose lanes devlanes mirrors to yours, so the lanes open on Windows. Both sides need the same release.

```yaml devlanes-user
# %USERPROFILE%\.devlanes\config.yaml
remote:
  boxes:
    ubuntu:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/you/.local/bin/devlanes, tunnel, --stdio]
```

[Connect to a box without SSH](remote/connect-by-command.md) has the pairing step.

## Related

- [Install devlanes](../install.md)
- [Choose the proxy's ports](hostnames/proxy-ports.md)
- [Trust the devlanes CA](hostnames/trust-the-ca.md)
- [Windows](../reference/windows.md)
