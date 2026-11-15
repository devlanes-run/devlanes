---
title: Connect to a box
description: Pair your machine with a box once, connect to it over SSH and open its lanes in your own browser.
kind: how-to
---

When a box runs your project, connecting to it puts the box's lanes on your machine under the hostnames and ports the box gave them. The first connection pairs the two machines, and later ones need nothing from you.

::example{slug="remote-ssh-box"}

The box has to be ready first, as [Prepare a box](prepare-a-box.md) describes. A coding agent working on the box can do the box's part of these steps and ask you for yours, once it has the skill from [Set up a coding agent](../agents/set-up.md).

## Reach the box over SSH

Your daemon runs the system `ssh`, so the box connects under the name your SSH config gives it, with your jump hosts, keys and certificates.

```text title="~/.ssh/config"
Host my-vps
  HostName 203.0.113.10
  User you
```

Keep the key in an SSH agent so that reconnects need no prompt. The daemon uses the agent it was started with; after the agent changes, as on a new login, run `devlanes daemon restart`. Your machine serves the box's hostnames with its own certificates, so the box needs no HTTPS setup of its own.

## Pair and connect

The box serves only machines it has paired with. A pairing starts with a one-time code from the box, which your machine trades for a token it keeps for later connections.

:::steps
1. On the box, run `devlanes daemon pair`. It prints the code, how long it stays valid and the command to run on your machine.
2. On your machine, run `devlanes remote connect my-vps --code` followed by that code.
:::

Without `--code`, the connect asks for the code. It stays attached to show what SSH asks, such as a passphrase or a new host key, each prompt marked with the box's name. Ctrl-C only detaches the terminal, since the connection belongs to your daemon, and `--background` returns once the box is connected. How long a code lives and how many wrong ones the box takes are in [Remote connection states](../../reference/remote-states.md).

## See what your machine mirrors

`devlanes remote ls` lists the box as connected, with each port your machine now listens on for it. A port that a program on your machine already holds is listed with that program, and the box's lane is not reachable on that port here. Warnings, such as releases that differ, appear under the box.

`devlanes url ls` in the project's folder on your machine lists the box's hostnames next to your local ones, and they open in your browser as they do on the box. [Remote mirroring](../../concepts/remote-mirroring.md) explains why they keep their names and ports.

## Reconnect without a terminal

Your daemon reconnects by itself after sleep or a network change, and after it restarts. It keeps a box until you run `devlanes remote disconnect my-vps`, also when the box is gone for good.

A reconnect that needs you waits instead of retrying. SSH may ask for a password or a new host key, or the box may no longer accept your pairing. The box's hostnames then show [DL205 Box sign-in needed](../../errors/box-sign-in-needed.md) or [DL206 Box pairing needed](../../errors/box-pairing-needed.md), and `devlanes remote ls` names the command to run. For a sign-in that is `devlanes remote connect my-vps` again in a terminal, which shows the prompt.

## Use the box's lane instead of a stopped local one

A lane on your machine with the same hostnames keeps them while it runs, and also while it is stopped. To hand the hostnames of a stopped one to the box, run `devlanes remote use my-vps myapp@0` on your machine, which chooses the box's lane.

`devlanes remote use --local myapp@0` gives the hostnames back. Starting the local lane takes them back as well, and drops the choice.

## Revoke a machine

On the box, `devlanes daemon clients` lists the machines paired with it and the ones connected now. `devlanes daemon revoke` with a name or id from that list deletes the machine's pairings and ends its connections. Its SSH access stays, so remove its key from the box as well; [Security](../../concepts/security.md) explains what pairing protects.

## If your machine runs Windows

The OpenSSH client of Windows has no shared connection, so each reconnect signs in again. Keep the key in the ssh-agent service of Windows so those sign-ins need no prompt; [Windows](../../reference/windows.md) has the details.

## Related

- [Connect to a box without SSH](connect-by-command.md)
- [Connect over Tailscale](tailscale.md)
- [Connect through Cloudflare](cloudflare.md)
- [Work with several boxes](several-boxes.md)
