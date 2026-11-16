---
title: Report a bug
description: Collect the versions, checks and logs a devlanes bug report needs, and file it.
kind: how-to
---

When devlanes does something you did not expect, a report with what you ran, what came back and what the daemon logged lets someone else reproduce it. A few commands collect most of that.

## Check the machine

The doctor command checks the daemon, Docker, the proxy, the CA and hostname lookups, and prints a fix under each line that is off. Its release lines are what a report needs first: the version, the build and how devlanes was installed.

```console title="on Windows 11"
$ devlanes doctor
release
  ✓ devlanes 0.13.0 (release build v0.13.0), installed with install.ps1
  ✓ runs from C:\Users\you\AppData\Local\Programs\devlanes\devlanes.exe
  ✓ latest release 0.13.0, read from github, updates.mode: notify, checked 1h ago
```

When a service is the problem, `devlanes debug why` with its name gives the reasons it is not running or not ready, and `devlanes debug errors` lists what failed, how often it crashed and its recent error lines. Put their output in the report too.

## Make a debug bundle

The bundle writes an archive for the report and prints where it is. Add the command that goes wrong after two dashes, and the bundle runs it again, with the most detailed diagnostics, and adds what it printed. That command really runs: a start starts the service, and a clean with `--yes` cleans the lane.

```console
$ devlanes debug bundle -- start api
```

The archive holds the client and daemon versions, the doctor checks, a check of the project file, the status of the lanes, the end of the daemon's log and the output of the command you added. Without a command after the two dashes, the bundle only reads from a running daemon and never starts one, so it also works when the daemon does not come up.

## Read the archive before you attach it

Before anything is written, devlanes replaces the values of variables whose names look secret, such as names with token, key or password in them, along with passwords in URLs and your home directory. Project, branch and host names stay. A secret in a variable whose name does not say so, or in a command's arguments, stays as well, so open the files and remove what should not be public.

## Turn up the daemon's log

`devlanes daemon logs` prints the end of the daemon's own log. For more detail in it, restart the daemon with a debug level, then do what goes wrong again. The restart stops each running service and starts it again, so it asks first when services run.

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

For a single command, `-v` prints what the client and the daemon did, step by step, with timestamps, on stderr. `-vv` prints more.

## File the issue

Open an issue at [github.com/devlanes-run/devlanes/issues](https://github.com/devlanes-run/devlanes/issues) with:

- your OS and its version, and the release lines of the doctor, with your user name replaced
- the command you ran, what you expected and what you got, with its output
- the lines of the project file the problem touches, with secrets removed
- the debug bundle

## If it is a security problem

Do not open a public issue. Follow the [devlanes-run security policy](https://github.com/devlanes-run/.github/blob/main/SECURITY.md), which says where to send it and what to include.

## Related

- [Troubleshooting](../troubleshooting.md)
- [The daemon](../concepts/daemon.md)
- [Files and folders](../reference/files.md)
