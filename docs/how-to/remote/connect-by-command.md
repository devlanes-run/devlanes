---
title: Connect to a box without SSH
description: Reach a container, a Kubernetes pod, a WSL distribution or a codespace through the command that runs programs inside it.
kind: how-to
---

A container, a pod, a WSL distribution or a codespace can be a box without an SSH server of its own. Each has a command that runs a program inside it with its input and output passed through, and that command takes the place of SSH.

::example{slug="remote-docker-box"}

## Prepare the inside of the box

Install devlanes inside the box, the same release as on your machine, give the box its own ports and start the project there once, as [Prepare a box](prepare-a-box.md) shows. Linger and the login service do not apply to a container. Your machine does not search the box for devlanes over a command, so put it on the path the command gets, or name it by its full path in the command.

The box daemon belongs to the user the command runs as. It outlives each connection, but stops with its container, pod or distribution.

## Name the box and its command

Give the box a name in your user config, with the command that runs `devlanes tunnel --stdio` inside it, under [`remote.boxes.<name>.command`](/docs/reference/config/user/remote/). The command is a list with one item per argument. No shell runs it, so nothing in it is expanded, such as a tilde or a variable.

::::tabs
:::tab[Docker]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    devbox:
      command: [docker, exec, -i, devbox, devlanes, tunnel, --stdio]
```

The container must already run. Keep `-i` and leave out Docker's terminal flag, since a terminal would corrupt the stream. `docker compose exec -T` does the same for a Compose service.
:::
:::tab[Kubernetes]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    devbox:
      command: [kubectl, exec, -i, devbox-0, -c, app, --, devlanes, tunnel, --stdio]
```

The pod name is fixed in the list, so a StatefulSet's pod fits it. For a Deployment, write `deploy/` and the Deployment's name to reach one of its pods.
:::
:::tab[WSL]
```yaml devlanes-user
# %USERPROFILE%\.devlanes\config.yaml
remote:
  boxes:
    devbox:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/you/.local/bin/devlanes, tunnel, --stdio]
```

`--exec` skips the distribution's shell, so the path to devlanes is absolute. [Run devlanes in WSL](../wsl.md) covers keeping the distribution running.
:::
:::tab[Codespace]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    devbox:
      command: [gh, codespace, ssh, -c, myapp-x9q7, --, devlanes, tunnel, --stdio]
```

The name after `-c` is your codespace's, as the GitHub CLI lists it.
:::
::::

`devlanes config set` writes the same entry from the command line, as a quoted list. Your daemon reads the list each time you connect, so a change needs no restart.

```console title="on your machine"
$ devlanes config set remote.boxes.devbox.command '[docker, exec, -i, devbox, devlanes, tunnel, --stdio]'
```

## Sign in to the tool first

The command runs under your daemon, with no terminal, and nothing passes its prompts to you. Sign in to the tool before connecting, as you would for the GitHub CLI.

```console title="on your machine"
$ gh auth login
```

A new daemon keeps only some variables of the shell that started it, among them the path, the one naming your Kubernetes config and Docker's own, as [Variables a new daemon keeps](../../reference/environment-variables.md#variables-a-new-daemon-keeps) lists. A variable a tool needs for its sign-in does not reach the command otherwise, so sign in to the tool in a way it saves to a file or to the system's credential store, as the GitHub CLI does.

## Pair and connect

Pairing works as over SSH, as [Connect to a box](connect.md) explains, and the box's half runs through the same tool. Here for the Docker box, get the code first.

```console title="on your machine"
$ docker exec devbox devlanes daemon pair
```

Then connect with the code it gave and check the box.

```console title="on your machine"
$ devlanes remote connect devbox --code K7QM-3XWD
```

```console title="on your machine"
$ devlanes remote ls
```

Each reconnect runs the command again, so a reconnect after sleep is a new exec into the box. A container that only you reach can skip pairing, as [Connect to a box](connect.md#if-the-box-should-serve-without-pairing) describes.

## If WSL holds the box's ports on Windows

WSL makes ports bound in the distribution reachable on Windows at localhost, and a port held there is not mirrored. Turn that forwarding off in your WSL settings on Windows, so the ports are left to devlanes.

```text title="%USERPROFILE%\.wslconfig"
[wsl2]
localhostForwarding=false
```

WSL reads the file when its virtual machine starts, so shut it down before you connect again. In mirrored networking mode the setting has no effect.

```console title="on your machine"
PS> wsl.exe --shutdown
```

## If the command is a batch file on Windows

A program named without an extension is looked for with `.exe` or `.com` added, so the name of a batch file without its extension does not find it. Run a batch file through the Windows command prompt.

```yaml devlanes-user
remote:
  boxes:
    devbox:
      command: [cmd, /c, devbox.cmd]
```

## Related

- [Connect to a box](connect.md)
- [Run devlanes in WSL](../wsl.md)
- [Lanes on another machine](../../learn/remote.md)
- [Remote connection states](../../reference/remote-states.md)
