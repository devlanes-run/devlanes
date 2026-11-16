---
title: Connect to a box
description: Pair your machine with a box once, connect to it over SSH and open its lanes in your own browser.
kind: how-to
---

When a box runs your project, connecting to it puts the box's lanes on your machine under the hostnames and ports the box gave them.

::example{slug="remote-ssh-box"}

The box has to be ready first, as [Prepare a box](prepare-a-box.md) describes.

## Reach the box over SSH

Your daemon runs the system `ssh`, so give the box an entry in your SSH config and keep its key in an SSH agent, so that reconnects need no prompt.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName 203.0.113.10
  User you
```

## Pair and connect

The box serves only machines it has paired with. Get a one-time code on the box, then connect with it under the name your SSH config gives the box.

```console title="on the box"
$ devlanes daemon pair
```

```console title="on your machine"
$ devlanes remote connect my-vps --code K7QM-3XWD --background
```

Your machine keeps a token for later connections, so the code is needed once. Without `--background` the command stays in the terminal until you end it, and the connection carries on either way.

## Open the box's lanes

Check the connection, then open the box's URLs in your browser as you would on the box.

```console title="on your machine"
$ devlanes remote ls
$ devlanes url ls
```

A lane on your own machine with the same hostnames keeps them, even when it is stopped, and `devlanes remote ls` then lists the box's lane as skipped. Hand the hostnames to the box's lane, and take them back later with `--local`.

```console title="on your machine"
$ devlanes remote use my-vps myapp@0
$ devlanes remote use --local myapp@0
```

## Reconnect and disconnect

Your daemon reconnects by itself after sleep, a network change or a restart. When a reconnect needs you, such as for a passphrase, run the connect again in a terminal. When the box asks for a new pairing, get a new code and connect with it as above. Disconnect a box you no longer use.

```console title="on your machine"
$ devlanes remote connect my-vps
$ devlanes remote disconnect my-vps
```

## Revoke a machine

On the box, list the machines paired with it and revoke one by its name. Remove its SSH key from the box as well; [Security](../../concepts/security.md) explains why.

```console title="on the box"
$ devlanes daemon clients
$ devlanes daemon revoke laptop
```

## If no machine should connect to this one

Set [`remote.allow_connections`](/docs/reference/config/user/remote/#remote-allow_connections) to false in its user config.

## If the box should serve without pairing

Set [`remote.require_pairing`](/docs/reference/config/user/remote/#remote-require_pairing) to false in the box's user config, so each machine that SSH lets in connects without a code. [Security](../../concepts/security.md) covers what that gives up.

## Related

- [Connect to a box without SSH](connect-by-command.md)
- [Connect over Tailscale](tailscale.md)
- [Connect through Cloudflare](cloudflare.md)
- [Work with several boxes](several-boxes.md)
