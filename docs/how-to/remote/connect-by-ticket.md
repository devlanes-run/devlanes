---
title: Connect to a box with a ticket
description: Pair your machine with a box by pasting a one-time ticket, and connect to it directly, without SSH.
kind: how-to
---

A ticket from the box pairs your machine with it and connects the two directly, peer to peer, with no SSH account on the box. Use it for a box you can open a terminal on, such as a second computer, a VPS or a cloud workstation.

The box has to be ready first, its daemon included, as [Prepare a box](prepare-a-box.md) describes.

## Pair and connect

Print a ticket on the box, then connect with it on your machine, giving the box the name you want to use for it.

```console title="on the box"
$ devlanes daemon pair
```

If it prints a code and no ticket, run `devlanes daemon pair --ticket`, which waits longer for the ticket.

```console title="on your machine"
$ devlanes remote connect dlp1... --name my-vps
```

Paste the whole ticket in place of `dlp1...`. It works once, within 10 minutes. Without `--name`, your machine keeps the box under the box's own host name, and prints it. Your daemon reconnects after sleep, a network change or a restart, and a later connect takes the name and no ticket:

```console title="on your machine"
$ devlanes remote connect my-vps
```

[Connect to a box](connect.md) covers opening the box's lanes, which works the same for every box.

## Forget a box

`devlanes remote disconnect my-vps` keeps the pairing, so a later connect needs no ticket. `--forget` deletes it from your machine; folders synced from the box stay in the devlanes home. On the box, `devlanes daemon revoke` turns your machine away.

```console title="on your machine"
$ devlanes remote disconnect my-vps --forget
```

## If the connection does not form

devlanes connects only over a direct path between the two machines and does not keep a connection going through a relay. When the networks in between allow no direct path, the connect fails and says so: connect over SSH, as [Connect to a box](connect.md) shows, or over [Tailscale](tailscale.md).

## If you host your own relay

A box on a relay you host yourself, from the [devlanes relay](https://github.com/devlanes-run/relay) repository, is reached by machines that list the same relay in [`remote.relays`](/docs/reference/config/user/remote/#remote-relays).

## If both machines are on one network

Set [`remote.relays`](/docs/reference/config/user/remote/#remote-relays) to an empty list on both machines and run `devlanes daemon restart` on each, and they reach each other without a relay.

```yaml devlanes-user
remote:
  relays: []
```

Your machine then finds the box only at the addresses in its ticket, so pair again with a new ticket when the box's IP address changes, as on another network or with a new address from the router, and when the box's daemon could not keep its port because another program took it, which its log (`devlanes daemon logs` on the box) says. On a Windows box, allow devlanes through the firewall, or the box does not answer.

## Related

- [Connect to a box](connect.md)
- [Connect over Tailscale](tailscale.md)
- [Remote connection states](../../reference/remote-states.md)
- [Security](../../concepts/security.md)
