---
title: Connect to a box
description: Pair your machine with a box once, connect to it over SSH and open its lanes in your own browser.
kind: how-to
---

When a box runs your project, connecting to it puts the box's lanes on your machine under the hostnames and ports the box gave them. The first connection pairs the two machines, and later ones use the token your machine keeps.

::example{slug="remote-ssh-box"}

The box has to be ready first, as [Prepare a box](prepare-a-box.md) describes. A coding agent working on the box can do the box's part of these steps and ask you for yours, once it has the skill from [Set up a coding agent](../agents/set-up.md).

## Reach the box over SSH

Your daemon runs the system `ssh`, so it reaches the box under the name your SSH config gives it, with your jump hosts, keys and certificates.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName 203.0.113.10
  User you
```

Keep the key in an SSH agent, so that the connections your daemon makes on its own need no prompt. The daemon uses the agent it was started with, so after the agent changes, as on a new login, restart it.

```console title="on your machine"
$ devlanes daemon restart
```

## Pair and connect

The box serves only machines it has paired with. A pairing starts with a one-time code from the box, which your machine trades for a token it keeps for later connections.

```console title="on the box"
$ devlanes daemon pair
```

Connect with that code, under the name your machine reaches the box by.

```console title="on your machine"
$ devlanes remote connect my-vps --code K7QM-3XWD
```

Without `--code`, the command asks for the code. It stays in the terminal to show what SSH asks, such as a passphrase or a new host key, and `--background` returns once the box is connected instead. How long a code lives and how many wrong ones the box takes are in [Remote connection states](../../reference/remote-states.md).

## See what your machine mirrors

`devlanes remote ls` lists the boxes your machine knows and their connection state.

```console title="on your machine"
$ devlanes remote ls
```

Outside a project folder, `devlanes url ls` lists every hostname your machine routes, the box's among them. They open in your browser as they do on the box, and [Remote mirroring](../../concepts/remote-mirroring.md) explains why they keep their names and ports.

```console title="on your machine"
$ devlanes url ls
```

## Reconnect without a terminal

The connection belongs to your daemon. It reconnects by itself after sleep or a network change and after the daemon restarts, and it keeps a box until you disconnect it, also when the box is gone for good.

```console title="on your machine"
$ devlanes remote disconnect my-vps
```

On macOS and Linux, your machine keeps one SSH connection per box and runs every session over it, so it signs in again only when that connection has ended. On Windows each reconnect signs in again, so keep the key in the ssh-agent service of Windows.

A reconnect that needs you waits instead of retrying. When SSH asks for a password or a new host key, or refuses a key it accepted before, the box's hostnames show [DL205 Box sign-in needed](../../errors/box-sign-in-needed.md). Run the connect again in a terminal, which shows the prompt.

```console title="on your machine"
$ devlanes remote connect my-vps
```

When the box no longer accepts your pairing, because it revoked your machine or reset its state, its hostnames show [DL206 Box pairing needed](../../errors/box-pairing-needed.md). Get a new code from `devlanes daemon pair` on the box and connect with `--code` as above.

## Use the box's lane instead of a stopped local one

A lane on your machine with the same hostnames keeps them while it runs, and also while it is stopped. Choose the box's lane to hand it the hostnames of a stopped one.

```console title="on your machine"
$ devlanes remote use my-vps myapp@0
```

Give them back to the lane on your machine with `--local`. Starting the local lane takes them back as well, and drops the choice.

```console title="on your machine"
$ devlanes remote use --local myapp@0
```

## Revoke a machine

On the box, list the machines paired with it and the ones connected now.

```console title="on the box"
$ devlanes daemon clients
```

Revoke one by its name. When two machines share that name, the box refuses it, and the machine's id or the start of its id picks one. Revoking deletes the machine's pairings and ends its connections. Remove its SSH key from the box as well; [Security](../../concepts/security.md) explains why.

```console title="on the box"
$ devlanes daemon revoke laptop
```

## If no machine should connect to this one

Set [`remote.allow_connections`](/docs/reference/config/user/remote/) to false in the user config of a machine that should never serve as a box. Every machine that connects is turned away, and `devlanes daemon pair` gives no codes. Machines connected already stay until they disconnect.

```yaml devlanes-user
# ~/.devlanes/config.yaml
remote:
  allow_connections: false
```

## If the box should serve without pairing

A box that SSH alone should guard, such as a container that only you reach or copies of one image, can skip pairing with [`remote.require_pairing`](/docs/reference/config/user/remote/) set to false in its user config. Each machine that gets in then connects without a code. [Security](../../concepts/security.md) covers what that gives up.

## Related

- [Connect to a box without SSH](connect-by-command.md)
- [Connect over Tailscale](tailscale.md)
- [Connect through Cloudflare](cloudflare.md)
- [Work with several boxes](several-boxes.md)
