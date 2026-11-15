---
title: Connect to a box without SSH
description: Reach a container, a Kubernetes pod, a WSL distribution or a codespace through the command that runs programs inside it.
kind: how-to
---

A container, a pod, a WSL distribution or a codespace can be a box without an SSH server of its own. Each has a command that runs a program inside it with its input and output passed through, and that command takes the place of SSH.

::example{slug="remote-docker-box"}

## Prepare the inside of the box

devlanes is installed inside the box, the same release as on your machine, and the project is started there once, as [Prepare a box](prepare-a-box.md) describes. Your machine does not search the box for devlanes over a command, so it has to be on the path the command gets, or named by its full path in the command. The box daemon belongs to the user the command runs as. It outlives each connection, but stops with its container, pod or distribution.

## Name the box and its command

The box gets a name under [`remote.boxes`](/docs/reference/config/user/remote/) in your user config, and the command that runs `devlanes tunnel --stdio` inside it. The command is a list with one item per argument. No shell runs it, so nothing in it is expanded, such as a tilde or a variable.

::::tabs
:::tab[Docker]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    my-vps:
      command: [docker, exec, -i, my-vps, devlanes, tunnel, --stdio]
```

The container must already run. Keep `-i` and leave out Docker's terminal flag, since a terminal would corrupt the stream. For a Compose service, Compose's exec command does the same with `-T`, which turns its terminal off.
:::
:::tab[Kubernetes]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    my-vps:
      command: [kubectl, exec, -i, myapp-0, -c, app, --, devlanes, tunnel, --stdio]
```

The pod name is fixed in the list, so a StatefulSet's pod fits it. For a Deployment, write `deploy/` and the Deployment's name to reach its first pod.
:::
:::tab[WSL]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    my-vps:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/you/.devlanes/bin/devlanes, tunnel, --stdio]
```

`--exec` skips the distribution's shell, so the path to devlanes is absolute. WSL forwards ports bound inside the distribution to Windows, where they can block the same ports from being mirrored; [Run devlanes in WSL](../wsl.md) covers that and keeping the distribution running.
:::
:::tab[Codespace]
```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  boxes:
    my-vps:
      command: [gh, codespace, ssh, -c, myapp-x9q7, --, devlanes, tunnel, --stdio]
```

The GitHub CLI starts a stopped codespace first, which can take a minute. The codespace needs an SSH server, and the GitHub CLI has to be signed in before your daemon runs it.
:::
::::

`devlanes config set` writes the same entry from the command line. Your daemon reads the list each time you connect, so a change needs no restart, and a connected box keeps its old command until you disconnect it.

## Sign in to the tool first

The command runs under your daemon, with no terminal and the daemon's environment, and nothing passes its prompts to you. Sign in to the tool before connecting, as with `gh auth login` or your Kubernetes credentials. When the daemon lacks a variable the tool needs, such as the one that names your Kubernetes config, run `devlanes daemon restart` from a shell that has it.

## Pair and connect

Pairing works as over SSH, and [Connect to a box](connect.md) explains it. The box's half runs through the same tool, here for the Docker box.

:::steps
1. On your machine, run `docker exec my-vps devlanes daemon pair`. With no terminal it prints the code alone.
2. Run `devlanes remote connect my-vps --code` followed by that code.
3. Run `devlanes remote ls`. It lists the box with `command` as its transport.
:::

Each reconnect runs the command again, so a reconnect after sleep is a new exec into the box.

## If the container is thrown away often

A container that only you reach can skip pairing with [`remote.require_pairing`](/docs/reference/config/user/remote/) set to false in the user config inside its image. Then each new container connects without a code. [Security](../../concepts/security.md) covers what that gives up.

## If the command is a batch file on Windows

Your machine looks up a program without a folder on the path with `.exe` or `.com` added, so a batch file is not found that way. Run it through the Windows command prompt instead.

```yaml devlanes-user
remote:
  boxes:
    my-vps:
      command: [cmd, /c, my-vps.cmd]
```

## Related

- [Connect to a box](connect.md)
- [Run devlanes in WSL](../wsl.md)
- [Lanes on another machine](../../learn/remote.md)
- [Remote connection states](../../reference/remote-states.md)
