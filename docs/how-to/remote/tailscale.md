---
title: Connect over Tailscale
description: Connect to a box on your tailnet with OpenSSH keys or Tailscale SSH, without exposing any port of it.
kind: how-to
---

A box on your tailnet connects like any other SSH host, and Tailscale only carries the connection or signs you in. Ports, hostnames and synced folders all travel inside the one SSH connection, so nothing needs Tailscale Serve, Funnel or a port bound to the box's tailnet address.

::example{slug="remote-ssh-box"}

## Name the box

Give the box an entry in your SSH config with its full MagicDNS name, or its Tailscale address. The short name depends on the search domain Tailscale adds to your DNS settings, and the full name does not.

```text title="~/.ssh/config"
Host my-vps
  HostName my-vps.example.invalid
  User you
```

A device keeps its Tailscale address on every network, so after sleep or a change of Wi-Fi your daemon reconnects to the same name.

## Allow SSH in the tailnet policy

With the box's own SSH server and your key, the policy only has to let your devices reach port 22 on the box. The default policy allows traffic between your devices; with your own, one grant does it.

```json
{
  "grants": [
    { "src": ["autogroup:member"], "dst": ["tag:devbox"], "ip": ["tcp:22"] }
  ]
}
```

Keep the key in an SSH agent so that reconnects need no prompt, as [Connect to a box](connect.md) describes.

## Pair and connect

Pairing works as on any network: a code from `devlanes daemon pair` on the box, then `devlanes remote connect my-vps` with it on your machine. [Connect to a box](connect.md) has the steps. Anyone with a shell on the box as you can hand out a code, so keep the tailnet's SSH access to your own devices.

## If the box uses Tailscale SSH

Tailscale SSH signs you in by your tailnet identity, with no key or agent. Turn it on, on the box.

```console title="on the box"
$ tailscale set --ssh
```

SSH sessions to the box's tailnet address that are open at that moment hang, and your daemon reconnects once its own session times out. The policy needs an `ssh` rule as well as the grant, and the user in your SSH config has to be one of its `users`.

```json
{
  "ssh": [
    {
      "action": "accept",
      "src": ["autogroup:member"],
      "dst": ["tag:devbox"],
      "users": ["you"]
    }
  ]
}
```

On Linux, Tailscale SSH runs commands in a login shell, so the box reads your login files for devlanes' commands too. The daemon a connection starts takes its path from them. Keep them from printing anything.

## If the rule asks for a check

A rule with the `check` action wants you to sign in again in a browser. Tailscale sends that link as an SSH banner on standard error, which your daemon does not show, so the connect waits for a sign-in nobody sees. The default policy of a new tailnet uses `check` for your own devices, and it wins over an `accept` rule that matches the same connection.

Change the rule for this box to `accept`, or pass the check yourself and connect within its period.

```console title="on your machine"
$ ssh my-vps true
```

The command prints the sign-in link. Open it, then run `devlanes remote connect my-vps`. A reconnect after the check period ends waits again until you repeat both.

## If you run Headscale

Headscale runs the same Tailscale clients, and OpenSSH over the tailnet works the same. It accepts `check` from release 0.29, where you pass the check as above. Earlier releases end the session when the check period runs out, which your daemon sees as a dropped connection, so use `accept` with them.

## Related

- [Connect to a box](connect.md)
- [Prepare a box](prepare-a-box.md)
- [Remote connection states](../../reference/remote-states.md)
- [Security](../../concepts/security.md)
