---
title: Give a server you run yourself a hostname
description: Put an HTTPS hostname in front of a server you start yourself, through the devlanes proxy, without devlanes running it.
kind: how-to
---

When you already run a server yourself, such as a docs site or pgAdmin, devlanes can give it an HTTPS hostname without starting or watching it.

This needs devlanes installed with HTTPS set up, as [Install devlanes](../../install.md) describes, and a look at [A service, a port and a hostname](../../learn/first-service.md) helps.

## Declare the server

devlanes reads a project file, `devlanes.yaml`, in the folder you run it from, a Git repository or a plain folder. For a server devlanes does not start, the file names the project and the address where your server listens:

```yaml devlanes
# devlanes.yaml
project: myapp

infra:
  external:
    docs:
      address: 127.0.0.1:4800
      hosts: "${{ lane.domain }}"
```

The hostname is the project name followed by `.localhost`, so this file gives `myapp.localhost`.

## Start the lane

Run one start in the folder:

```console title="~/dev/myapp"
$ devlanes start --no-ui
```

Nothing needs starting, but the folder becomes a lane of the project and the proxy routes the hostname to port 4800. Open `https://myapp.localhost`: while your server runs, it answers; while it is down, the proxy shows an error page saying nothing listens.

The proxy lives in the daemon, and a daemon that a command started exits a while after the last command. To keep the hostname answering, run the daemon as a login service, as [The daemon](../../concepts/daemon.md) describes.

## If only you run the server

Put the entry in the local file, `devlanes.local.yaml` next to the project file, which stays out of Git. [Keep personal changes out of Git](../projects/local-overrides.md) covers it.

## If the server rejects the hostname

Some dev servers refuse a request whose Host header is not their own address. The mapping form of a hostname makes the proxy send the local address instead:

```yaml devlanes
infra:
  external:
    docs:
      address: 127.0.0.1:4800
      hosts:
        host: "${{ lane.domain }}"
        host_header: upstream
```

## Related

- [A service, a port and a hostname](../../learn/first-service.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [The daemon](../../concepts/daemon.md)
- [Choose the proxy's ports](../hostnames/proxy-ports.md)
