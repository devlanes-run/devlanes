---
title: Connect to a box through a command
description: Reach a container, a Kubernetes pod, a WSL distribution or a codespace through the command that runs programs inside it.
kind: how-to
---

A container, a pod, a WSL distribution or a codespace can be a box without an SSH server, through the command that runs a program inside it.

::example{slug="remote-docker-box"}

## Prepare the inside of the box

Install devlanes inside the box, give it its own ports and start the project there, as [Prepare a box](prepare-a-box.md) shows. Linger and the service do not apply to a container. Put devlanes on the path the command gets, or name it by its full path in the command.

## Name the box and its command

Give the box a name under [`remote.boxes.<name>.command`](/docs/reference/config/user/remote/#remote-boxes-name-command), with a command that runs `devlanes tunnel --stdio` inside it, one list item per argument. No shell runs it.

::::tabs
:::tab[Docker]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    devbox:
      command: [docker, exec, -i, devbox, devlanes, tunnel, --stdio]
```

Keep `-i` and leave out Docker's terminal flag.
:::
:::tab[Kubernetes]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    devbox:
      command: [kubectl, exec, -i, devbox-0, -c, app, --, devlanes, tunnel, --stdio]
```
:::
:::tab[WSL]
```yaml devlanes-user
# %USERPROFILE%\.devlanes\config.yaml
remote:
  boxes:
    devbox:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/you/.local/bin/devlanes, tunnel, --stdio]
```

`--exec` skips the distribution's shell, so the path to devlanes is absolute.
:::
:::tab[Codespace]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    devbox:
      command: [gh, codespace, ssh, -c, myapp-x9q7, --, devlanes, tunnel, --stdio]
```
:::
::::

## Sign in to the tool first

The command runs under your daemon with no terminal, so nothing can answer a prompt. Sign in to the tool beforehand, as for the GitHub CLI.

```console title="on your machine"
$ gh auth login
```

## Pair and connect

Pairing works as [Connect to a box](connect.md) describes, with the box's half run through the same tool.

```console title="on your machine"
$ docker exec devbox devlanes daemon pair
$ devlanes remote connect devbox --code K7QM-3XWD --background
```

## If a tool needs a variable from your shell

The daemon keeps only some of your shell's variables, listed under [Variables a new daemon keeps](../../reference/environment-variables.md#variables-a-new-daemon-keeps). Sign in to a tool in a way it saves to a file or the system's credential store.

## If WSL holds the box's ports on Windows

WSL forwards the distribution's ports to Windows, where they block mirroring. Turn that off and restart WSL.

```text title="%USERPROFILE%\.wslconfig"
[wsl2]
localhostForwarding=false
```

```console title="on your machine"
PS> wsl.exe --shutdown
```

## Related

- [Connect to a box](connect.md)
- [Run devlanes in WSL](../wsl.md)
- [Lanes on another machine](../../learn/remote.md)
- [Remote connection states](../../reference/remote-states.md)
