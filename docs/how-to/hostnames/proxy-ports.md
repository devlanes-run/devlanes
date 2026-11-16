---
title: Choose the proxy's ports
description: Get the proxy onto ports 443 and 80, move it next to another reverse proxy, or let other devices reach your lanes.
kind: how-to
---

Use this guide when lane URLs carry a port such as 8443, or when another web server on your machine wants ports 443 and 80.

## Check which ports the proxy has

The proxy asks for 443 and 80. When it cannot have one, because another program holds it or the system does not allow it, it uses 8443 or 8080 and lane URLs carry that port. The doctor's proxy group shows the ports and why:

```console
$ devlanes doctor
```

## On Linux, let the daemon bind 443 and 80

Linux keeps ports below 1024 for root. Give the devlanes binary the right to bind them, then restart the daemon. The path is the one the doctor prints after "runs from"; `~/.local/bin/devlanes` is where the install script puts it.

```console
$ sudo setcap cap_net_bind_service=+ep "$(readlink -f ~/.local/bin/devlanes)"
$ devlanes daemon restart
```

An update replaces the binary and drops that right, so run both commands again after each update.

## Move the proxy to other ports

Caddy, nginx or Traefik on the same machine competes with devlanes for 443 and 80, and on Linux the one that starts second does not get them. To leave them to the other program, give the proxy ports of its own:

```yaml devlanes-user
# ~/.devlanes/config.yaml
proxy:
  https_port: 9443
  http_port: 9080
```

```console
$ devlanes daemon restart
```

Lane URLs then read https://myapp.localhost:9443. [`proxy.https_port`](/docs/reference/config/user/proxy/#proxy-https_port) and [`proxy.http_port`](/docs/reference/config/user/proxy/#proxy-http_port) have the details.

## If another device should reach a lane

Let the proxy accept connections from the network, then restart the daemon:

```yaml devlanes-user
# ~/.devlanes/config.yaml
proxy:
  lan: true
```

```console
$ devlanes daemon restart
```

The device also has to resolve the lane's hostnames to your machine and trust the devlanes CA, a certificate authority devlanes creates on your machine. Names under localhost always mean the device itself, so use a domain of your own from [Fix programs that cannot resolve *.localhost](resolve-localhost.md). Every device on the network can then reach your lanes; [Security](../../concepts/security.md) has what that exposes.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Install devlanes](../../install.md)
- [Run devlanes in WSL](../wsl.md)
