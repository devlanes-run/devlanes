---
title: Report a bug
description: Collect the versions, checks and logs a devlanes bug report needs, and file it.
kind: how-to
---

Use this when devlanes does something you did not expect and you want someone to reproduce it.

## Check the machine

```console
$ devlanes doctor
```

The doctor checks the daemon, Docker, the proxy, the CA and hostname lookups, prints a fix under each line that is off, and names the version and how devlanes was installed. When a service is the problem, `devlanes debug why` with the service's name gives the reasons.

## Make a debug bundle

Add the command that goes wrong after the two dashes. The bundle runs it again with full diagnostics, so a start really starts the service.

```console
$ devlanes debug bundle -- start api
```

It writes an archive with the versions, the doctor's checks, the status of the lanes, the end of the daemon's log and the output of that command, and prints where the archive is. Values of variables named like tokens, keys or passwords are replaced, but a secret under another name stays, so read the files before you attach them.

## File the issue

Open an issue at [github.com/devlanes-run/devlanes/issues](https://github.com/devlanes-run/devlanes/issues) with your OS, the command you ran, what you expected and what you got, the lines of the project file involved with secrets removed, and the bundle.

## If the daemon's log needs more detail

Restart the daemon with a debug level, then do what goes wrong again and make the bundle. The restart stops your services and starts them again.

::::tabs{sync="os"}
:::tab[macOS]
```console
$ DEVLANES_LOG=debug devlanes daemon restart
```
:::
:::tab[Linux]
```console
$ DEVLANES_LOG=debug devlanes daemon restart
```
:::
:::tab[Windows]
```console
PS> $env:DEVLANES_LOG = "debug"; devlanes daemon restart
```
:::
::::

## If it is a security problem

Do not open a public issue. Follow the [devlanes-run security policy](https://github.com/devlanes-run/.github/blob/main/SECURITY.md).

## Related

- [Troubleshooting](../troubleshooting.md)
- [The daemon](../concepts/daemon.md)
- [Files and folders](../reference/files.md)
