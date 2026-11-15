---
title: Give a server you run yourself a hostname
description: Put an HTTPS hostname in front of a server you start yourself, through the devlanes proxy, without devlanes running it.
kind: how-to
---

When you already run a server yourself, such as a docs site, pgAdmin or a tool from another project, devlanes can give it an HTTPS hostname without starting or watching it.

::example{slug="infra-plain-containers"}

This needs devlanes installed, with HTTPS set up as [Install devlanes](../../install.md) describes. devlanes reads a project file, `devlanes.yaml`, that names the project and what it runs; [A service, a port and a hostname](../../learn/first-service.md) introduces it.

## Declare the server

Put the server's address and the hostname it should get in the project file, at the root of the repository. For a server devlanes does not start, that is all the file needs:

```yaml devlanes
# devlanes.yaml
project: myapp

infra:
  external:
    docs:
      address: 127.0.0.1:4800
      hosts: "docs.${{ lane.domain }}"
```

The address is a loopback address and a port, `127.0.0.1` or `[::1]`, and the server has to listen there. The hostname is built from the lane's domain, which is `myapp.localhost` in the main worktree, so this one reads `docs.myapp.localhost`.

## Pick a port no lane uses

devlanes hands the services of its lanes ports from a range, 20000 to 32767 by default, so a port below it, as here, never goes to one of them. The declared port then belongs to the project. A port that devlanes or a lane already uses is refused when devlanes reads the file, with the entry and what holds the port.

## Start the lane and the server

Start the lane once in the folder, so devlanes knows the project. Then start the server as you usually do, listening on the declared address.

```console title="~/dev/myapp"
$ devlanes start
```

The proxy now answers `https://docs.myapp.localhost` and passes each request to port 4800, whether the server is up or not. devlanes never starts, stops or checks it: status lists the entry with its fixed port, and commands that would start, stop or read logs refuse it. On Linux the URL may carry the port 8443; [Choose the proxy's ports](../hostnames/proxy-ports.md) explains when.

## If only you run the server

A server the rest of the team does not run belongs in the local file, `devlanes.local.yaml` next to the project file, which stays out of Git. devlanes reads the change by itself and routes the new hostname. [Keep personal changes out of Git](../projects/local-overrides.md) covers the file.

## If the server rejects the hostname

Some dev servers answer only requests addressed to their own address and refuse any other Host header. The mapping form of a hostname makes the proxy send the local address instead:

```yaml devlanes
infra:
  external:
    docs:
      address: 127.0.0.1:4800
      hosts:
        host: "docs.${{ lane.domain }}"
        host_header: upstream
```

## If the server runs on a box

On a machine whose lanes you open from your own, declare the server on the box. [Choose what a box shares](../remote/what-a-box-shares.md) covers when your machine mirrors its port.

## Related

- [A service, a port and a hostname](../../learn/first-service.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Trust the devlanes CA](../hostnames/trust-the-ca.md)
