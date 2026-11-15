---
title: Connect through Cloudflare
description: Reach a box's SSH server through Cloudflare Tunnel and Access, with no port of the box open to the internet.
kind: how-to
---

A box behind Cloudflare needs one route, to its SSH server, and no public hostname per service, because ports, hostnames and synced folders all travel inside the SSH connection. You reach that route with cloudflared on your machine, or with the Cloudflare One Client.

::example{slug="remote-ssh-box"}

## Route SSH through a tunnel on the box

Install cloudflared on the box, create a tunnel, and give it a published route on a hostname of your zone with the SSH service type. A tunnel managed in a local file does the same with an ingress rule.

```yaml
# /etc/cloudflared/config.yml
tunnel: <tunnel-uuid>
credentials-file: /etc/cloudflared/<tunnel-uuid>.json
ingress:
  - hostname: ssh.example.invalid
    service: ssh://localhost:22
  - service: http_status:404
```

Protect the hostname with a self-hosted Access application whose policy lets in the people who connect, and run the tunnel as a service so it comes back after a reboot. The SSH server keeps its configuration, and TCP forwarding may stay off, since devlanes opens only session channels.

## Add the box to your SSH config

Your machine reaches the hostname through cloudflared as the proxy command of an SSH entry.

```text title="~/.ssh/config"
Host my-vps
  HostName ssh.example.invalid
  User you
  ProxyCommand /usr/local/bin/cloudflared access ssh --hostname %h
```

Write the full path to cloudflared. The SSH that devlanes runs is a child of your daemon and has the daemon's path, which may lack the folder cloudflared is in.

## Sign in to Access in a terminal

cloudflared needs an Access token for the hostname. Without one it prints a login link and waits, but your daemon does not show what cloudflared prints, so a connect would wait for a login nobody sees. Get the token in a terminal first.

```console title="on your machine"
$ ssh my-vps true
```

cloudflared keeps the token, and the runs your daemon starts reuse it. Then pair and connect as [Connect to a box](connect.md) describes.

## When the Access session expires

A connection that is up stays up, since Access only decides when a connection may start. A reconnect after sleep, a network change or a daemon restart needs a valid token. Without one, cloudflared starts a login again and the reconnect fails and retries. Run `ssh my-vps true` in a terminal again, then `devlanes remote connect my-vps`.

A longer session duration on the Access application makes this rarer. A machine that has to reconnect with nobody at the browser can use an Access service token instead.

## If your machine runs Windows

The OpenSSH client of Windows runs the proxy command too. Write the full path to `cloudflared.exe`, in double quotes when it holds a space. Each reconnect signs in again on Windows, so each needs a valid token.

## If you use the Cloudflare One Client

With the Cloudflare One Client enrolled, the box is reachable at a private address and SSH connects to it directly, with no proxy command. The tunnel on the box gets a private network route instead of a published one, and the client's split tunnels have to include that address. There is no login per connection, so reconnects need nobody while the client is connected.

```text title="~/.ssh/config"
Host my-vps
  HostName 10.0.0.12
  User you
```

## If you use short-lived certificates

SSH runs the certificate step from your SSH config before each connection, as it does in a terminal, and it needs a valid Access token as well. A certificate you renew some other way has to be renewed before it expires. A reconnect with an expired one waits for you, as [Connect to a box](connect.md) describes.

## Related

- [Connect to a box](connect.md)
- [Prepare a box](prepare-a-box.md)
- [Remote connection states](../../reference/remote-states.md)
- [Security](../../concepts/security.md)
