---
title: Connect to a box with a ticket
description: Pair your machine with a box by pasting a one-time ticket, and connect to it directly, without SSH.
kind: how-to
---

A ticket from the box pairs your machine with it and connects the two directly, peer to peer, with no SSH account or open port on the box. Use it for a box you can open a terminal on, such as a second computer, a VPS or a cloud workstation.

The box has to be ready first, as [Prepare a box](prepare-a-box.md) describes.

## Keep the box's daemon running

Your machine reaches the box only while the box's daemon runs, so install it as a service on the box. On Linux, turn on linger first, as [Prepare a box](prepare-a-box.md) shows.

```console title="on the box"
$ devlanes daemon install
```

## Pair and connect

Print a ticket on the box, then connect with it on your machine.

```console title="on the box"
$ devlanes daemon pair
```

```console title="on your machine"
$ devlanes remote connect dlp1...
```

Paste the whole ticket in place of `dlp1...`. It works once, within 10 minutes. Your machine keeps the box under the box's own name and prints that name. Your daemon reconnects after sleep, a network change or a restart, and a later connect takes the name and no ticket:

```console title="on your machine"
$ devlanes remote connect my-vps
```

To keep the box under another name, add `--name my-vps` to the first connect. With SSH access to the box, `ssh my-vps '~/.devlanes/bin/devlanes daemon pair --ticket'` prints the ticket alone.

## Open the box's lanes

```console title="on your machine"
$ devlanes remote ls
$ devlanes url ls
```

[Connect to a box](connect.md) covers choosing between a box's lane and your own, which works the same for every box.

## Disconnect and forget

`devlanes remote disconnect` keeps the pairing, so a later connect needs no ticket. `--forget` deletes it from your machine, and on the box `devlanes daemon revoke` turns your machine away for good.

```console title="on your machine"
$ devlanes remote disconnect my-vps --forget
```

```console title="on the box"
$ devlanes daemon clients
$ devlanes daemon revoke laptop
```

## If the connection does not form

devlanes connects only over a direct path between the two machines and never sends lane traffic through a relay. When the networks in between allow no direct path, the connect fails and says so: connect over SSH, as [Connect to a box](connect.md) shows, or over [Tailscale](tailscale.md).

## If both machines are on one network

Set [`remote.relays`](/docs/reference/config/user/remote/#remote-relays) to an empty list on both machines and run `devlanes daemon restart` on each, and they reach each other at the addresses in the ticket without contacting a relay. Those addresses change when the box's daemon restarts, so pair again with a new ticket after that.

```yaml devlanes-user
remote:
  relays: []
```

A box on a relay you host yourself, from the [devlanes relay](https://github.com/devlanes-run/relay) repository, is reached by machines that list the same relay there. [Security](../../concepts/security.md) says what a relay and a ticket reveal.

## Related

- [Connect to a box](connect.md)
- [Connect over Tailscale](tailscale.md)
- [Remote connection states](../../reference/remote-states.md)
- [Security](../../concepts/security.md)
