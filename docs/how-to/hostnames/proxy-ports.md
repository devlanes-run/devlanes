---
title: Choose the proxy's ports
description: Get the proxy onto ports 443 and 80, move it next to another reverse proxy, or let other devices reach your lanes.
kind: how-to
---

Use this guide when lane URLs carry a port such as 8443, when another web server on your machine wants ports 443 and 80, or when a phone or a second computer should open a lane.

## Check which ports the proxy has

The proxy asks for 443 and 80. When it cannot have one, because another program holds it or the system does not allow it, it takes 8443 for HTTPS or 8080 for plain http, and lane URLs then carry that port. The doctor command shows the ports in use:

```console title="on Windows 11"
$ devlanes doctor
proxy
  ✓ http on port 80
  ✓ https on port 443
```

A port the proxy fell back from gets a warning line with the reason, and names the program holding it when it can find one.

## On Linux, let the daemon bind 443 and 80

Linux keeps ports below 1024 for root, so a daemon running as you falls back unless you allow it. Either give the devlanes binary the capability to bind them, or lower the first unprivileged port for the whole machine:

```console
$ sudo setcap cap_net_bind_service=+ep "$(readlink -f "$(command -v devlanes)")"
```

```console
$ sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80
```

An update replaces the binary and drops the capability, so run setcap again after each one. The sysctl lets every program of every user bind ports from 80 up, and it lasts until a reboot unless you put it in a file under `/etc/sysctl.d`. Then run `devlanes daemon restart` so the proxy binds again.

macOS lets a user's program bind 443 and 80, and so does Windows. On Windows, port 80 is often held by the http.sys service, and Hyper-V, WSL or Docker may reserve ranges of ports; [Windows](../../reference/windows.md) covers both.

## Move the proxy to other ports

To keep 443 and 80 for another program, give the proxy ports of its own in the user config, then run `devlanes daemon restart`:

```yaml devlanes-user
# ~/.devlanes/config.yaml
proxy:
  https_port: 9443
  http_port: 9080
```

Lane URLs then read https://myapp.localhost:9443. A port you choose falls back to 8443 and 8080 like the default ones when it is taken. [`proxy`](/docs/reference/config/user/proxy/) lists the settings.

## If another reverse proxy runs on the machine

On macOS and Linux only one program can hold port 443. Whichever of devlanes and Caddy, nginx or Traefik starts second does not get it: the daemon falls back to 8443, and the other proxy usually fails to start. Move one of them, either the daemon as above or the other program.

On Windows both start, because the daemon listens on loopback only and the other proxy on every address. Requests to localhost names then reach devlanes and never the other proxy, and the doctor warns that the port is shared.

## If another device should reach a lane

The proxy accepts connections from this machine only. Allow the network in the user config and run `devlanes daemon restart`:

```yaml devlanes-user
# ~/.devlanes/config.yaml
proxy:
  lan: true
```

Windows Defender Firewall then asks once whether devlanes may accept connections from the network. The other device also has to find your machine under the lane's hostname and trust the certificate:

- A name under localhost always means the device itself, so switch to a top-level domain of your own, as [Fix programs that cannot resolve *.localhost](resolve-localhost.md) shows, and make the device resolve those names to your machine's address through its hosts file or a DNS server on your network.
- Install `~/.devlanes/ca/ca.pem` on the device as a trusted authority, as [Trust the devlanes CA](trust-the-ca.md) does for your own machine.

Every device on the network can then reach your lanes. [Security](../../concepts/security.md) has what that exposes. Lanes on a box need none of this, since your machine mirrors them; [Lanes on another machine](../../learn/remote.md) shows how.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Install devlanes](../../install.md)
- [Run devlanes in WSL](../wsl.md)
