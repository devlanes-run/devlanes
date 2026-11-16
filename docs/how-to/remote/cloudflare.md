---
title: Connect through Cloudflare
description: Reach a box's SSH server through Cloudflare Tunnel and Access, with no port of the box open to the internet.
kind: how-to
---

A box behind Cloudflare needs one route, to its SSH server, because everything devlanes mirrors travels inside the SSH connection.

::example{slug="remote-ssh-box"}

## Route SSH through a tunnel on the box

Install cloudflared on the box, create a tunnel, and give it a published application route on a hostname of your zone with the SSH service type and `localhost:22`. Protect the hostname with a self-hosted Access application, and run the tunnel as a service.

```yaml title="/etc/cloudflared/config.yml"
tunnel: <tunnel-uuid>
credentials-file: /etc/cloudflared/<tunnel-uuid>.json
ingress:
  - hostname: ssh.example.invalid
    service: ssh://localhost:22
  - service: http_status:404
```

## Add the box to your SSH config

Reach the hostname through cloudflared, written with its full path, since your daemon's path may not include it.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName ssh.example.invalid
  User you
  ProxyCommand /usr/local/bin/cloudflared access ssh --hostname %h
```

## Sign in, then connect

cloudflared signs you in through the browser, which your daemon cannot show you, so sign in from a terminal first. Then pair and connect as [Connect to a box](connect.md) describes. When the Access session expires, a reconnect fails until you run both commands again.

```console title="on your machine"
$ ssh my-vps true
$ devlanes remote connect my-vps
```

## If your machine runs Windows

Write the full path to `cloudflared.exe` in the proxy command, in double quotes when it holds a space.

## If you use the Cloudflare One Client

The box is then reachable at a private address through the client, and the SSH entry needs no proxy command.

## Related

- [Connect to a box](connect.md)
- [Prepare a box](prepare-a-box.md)
- [Security](../../concepts/security.md)
