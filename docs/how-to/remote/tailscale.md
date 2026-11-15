---
title: Connect over Tailscale
description: Connect to a box on your tailnet with OpenSSH keys or Tailscale SSH, without exposing any port of it.
kind: how-to
---

Your machine reaches a box on your tailnet as it reaches any SSH host, and Tailscale only carries the connection or signs you in. Ports, hostnames and synced folders all travel inside the one SSH connection, so nothing needs Tailscale Serve, Funnel or a port bound to the box's tailnet address.

::example{slug="remote-ssh-box"}

## Name the box

Give the box an entry in your SSH config with its full MagicDNS name, the machine name followed by your tailnet's DNS name, which ends in `ts.net`. The short machine name works through a search domain Tailscale adds to your DNS settings, and the full name does not depend on it.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName my-vps.example.ts.net
  User you
```

A device keeps its Tailscale address when it switches networks, so after sleep or a change of Wi-Fi your daemon reconnects to the same machine.

## Allow SSH in the tailnet policy

With the box's own SSH server and your key, the policy only has to let your devices reach port 22 on the box. The default policy allows traffic between all devices of the tailnet. With a policy of your own, a grant to your own devices does it.

```json title="tailnet policy file"
{
  "grants": [
    { "src": ["autogroup:member"], "dst": ["autogroup:self"], "ip": ["tcp:22"] }
  ]
}
```

Keep the key in an SSH agent so that reconnects need no prompt. Then pair and connect as [Connect to a box](connect.md) describes, and keep the tailnet's SSH access to your own devices, for the reason [Security](../../concepts/security.md) gives.

## If the box uses Tailscale SSH

Tailscale SSH signs you in by your tailnet identity, with no key or agent. Turn it on in the box's Tailscale client.

```console title="on the box"
$ tailscale set --ssh
```

SSH connections to the box's Tailscale address that are open at that moment hang, and your daemon reconnects once its own session times out. Restarting or upgrading Tailscale on the box ends Tailscale SSH sessions too, and your daemon reconnects after that as well.

The policy needs an `ssh` rule besides the grant, and the user in your SSH config has to be one of the rule's users. With `accept`, connects and reconnects need nobody at the keyboard.

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

A rule with the `check` action wants you to sign in again before a connection, and Tailscale gives the sign-in link while SSH connects. Your daemon does not show what SSH prints there, so the connect waits for a sign-in nobody sees and then retries. A new tailnet's default SSH rule checks connections to your own devices, and a checking rule that matches a connection wins over an `accept` rule that matches it too.

Change the rule for this box to `accept`, or pass the check yourself in a terminal, which prints the link.

```console title="on your machine"
$ ssh my-vps true
```

Then connect within the check period. Once it ends, reconnects fail until you pass the check again, and connecting by hand afterwards connects at once instead of at the next try.

```console title="on your machine"
$ devlanes remote connect my-vps
```

## Related

- [Connect to a box](connect.md)
- [Prepare a box](prepare-a-box.md)
- [Remote connection states](../../reference/remote-states.md)
- [Security](../../concepts/security.md)
