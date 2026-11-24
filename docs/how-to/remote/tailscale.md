---
title: Connect over Tailscale
description: Connect to a box on your tailnet with OpenSSH keys or Tailscale SSH, without exposing any port of it.
kind: how-to
---

Your machine reaches a box on your tailnet as it reaches any SSH host, and everything devlanes mirrors is sent inside that one SSH connection, so nothing needs Tailscale Serve or Funnel.

::example{slug="remote-ssh-box"}

## Name the box

Use the box's full MagicDNS name, which ends in your tailnet's `ts.net` domain.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName my-vps.example.ts.net
  User you
```

## Allow SSH in the tailnet policy

The default policy already allows traffic between your devices. With a policy of your own, let your devices reach port 22 on the box.

```json title="tailnet policy file"
{
  "grants": [
    { "src": ["autogroup:member"], "dst": ["autogroup:self"], "ip": ["tcp:22"] }
  ]
}
```

Keep your key in an SSH agent, then pair and connect as [Connect to a box](connect.md) describes.

## If the box uses Tailscale SSH

Tailscale SSH signs you in by your tailnet identity instead of a key. Turn it on in the box's Tailscale client, and add an `ssh` rule with the `accept` action, so connects and reconnects need nobody at the keyboard.

```console title="on the box"
$ tailscale set --ssh
```

```json title="tailnet policy file"
{
  "ssh": [
    {
      "action": "accept",
      "src": ["autogroup:member"],
      "dst": ["autogroup:self"],
      "users": ["you"]
    }
  ]
}
```

## If the rule asks for a check

A rule with the `check` action, which a new tailnet uses by default, asks for a browser sign-in your daemon cannot show you. Change it to `accept`, or pass the check in a terminal and connect within the check period.

```console title="on your machine"
$ ssh my-vps true
$ devlanes remote connect my-vps
```

## Related

- [Connect to a box](connect.md)
- [Prepare a box](prepare-a-box.md)
- [Security](../../concepts/security.md)
