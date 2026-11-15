---
title: Connect through Cloudflare
description: Reach a box's SSH server through Cloudflare Tunnel and Access, with no port of the box open to the internet.
kind: how-to
---

A box behind Cloudflare needs one route, to its SSH server, and no public hostname per service, because ports, hostnames and synced folders all travel inside the SSH connection. You reach that route with cloudflared on your machine, or with the Cloudflare One Client.

::example{slug="remote-ssh-box"}

## Route SSH through a tunnel on the box

Install cloudflared on the box and create a tunnel. Give it a published application route on a hostname of your zone, with the SSH service type and `localhost:22`. A tunnel managed in a local file does the same with an ingress rule.

```yaml title="/etc/cloudflared/config.yml"
tunnel: <tunnel-uuid>
credentials-file: /etc/cloudflared/<tunnel-uuid>.json
ingress:
  - hostname: ssh.example.invalid
    service: ssh://localhost:22
  - service: http_status:404
```

Protect the hostname with a self-hosted Access application whose policy lets in the people who connect, and run the tunnel as a service so it comes back after a reboot.

The SSH server keeps its configuration. TCP forwarding may stay off, since devlanes opens only session channels.

## Add the box to your SSH config

Your machine reaches the hostname through cloudflared as the proxy command of an SSH entry.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName ssh.example.invalid
  User you
  ProxyCommand /usr/local/bin/cloudflared access ssh --hostname %h
```

Write the full path to cloudflared. The SSH that devlanes runs is a child of your daemon and has the daemon's path, which may lack the folder cloudflared is in.

## Sign in to Access in a terminal

Without a valid Access session, cloudflared starts a sign-in in the browser before the connection goes through. Your daemon does not show anything cloudflared prints, so a connect started from it would wait for a sign-in you do not see. Sign in from a terminal first.

```console title="on your machine"
$ ssh my-vps true
```

Then pair and connect as [Connect to a box](connect.md) describes.

## When the Access session expires

Cloudflare does not limit how long an SSH connection that is up lasts. The Access session decides only when a new connection may start, so a reconnect after sleep, a network change or a daemon restart needs one that is still valid. Without it the reconnect fails and your daemon retries. Sign in again in a terminal, then connect.

```console title="on your machine"
$ ssh my-vps true
$ devlanes remote connect my-vps
```

A longer session duration on the Access application makes this rarer. A machine that has to reconnect with nobody at the browser can use an Access service token instead.

## If your machine runs Windows

The OpenSSH client of Windows runs the proxy command too. Write the full path to `cloudflared.exe`, in double quotes when it holds a space.

```ssh-config title="%USERPROFILE%\.ssh\config"
Host my-vps
  HostName ssh.example.invalid
  User you
  ProxyCommand "C:\Program Files\cloudflared\cloudflared.exe" access ssh --hostname %h
```

## If you use the Cloudflare One Client

With the Cloudflare One Client enrolled, the box is reachable at a private address and SSH connects to it directly, with no proxy command. The tunnel on the box gets a private network route instead of a published one, and the box's address has to go through the client, as its split tunnel settings decide.

```ssh-config title="~/.ssh/config"
Host my-vps
  HostName 10.0.0.12
  User you
```

## If you use short-lived certificates

Your daemon runs the system SSH with your SSH config, so the certificates of Cloudflare's [Access for Infrastructure](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/ssh/ssh-infrastructure-access/) work as they do in a terminal. Set them up as that guide describes, and check them with a connection from a terminal first. An infrastructure application closes its SSH connections when its session duration runs out, and your daemon then reconnects.

## Related

- [Connect to a box](connect.md)
- [Prepare a box](prepare-a-box.md)
- [Remote connection states](../../reference/remote-states.md)
- [Security](../../concepts/security.md)
