---
title: Give a service several ports and hostnames
description: Give one service extra ports by name, route hostnames to them, and handle a program that only listens on a fixed port.
kind: how-to
---

When one process listens on more than one port, such as an app with a separate WebSocket or admin server, each port can get a name and a hostname of its own.

::example{slug="web-realtime-ws"}

## Name the ports and route hostnames to them

```yaml devlanes
services:
  web:
    run_script:
      file: package.json
      name: dev
    env:
      WS_PORT: ${{ self.ports.ws }}
      ADMIN_PORT: ${{ self.ports.admin }}
    hosts:
      - ${{ lane.domain }}
      - host: ws.${{ lane.domain }}
        port: ws
      - host: admin.${{ lane.domain }}
        port: admin
```

Naming a port in the service gives it one more port in each lane, and your program reads the number from the variable, as it reads `PORT` for the main one. A hostname that names a port goes to that port, and a plain one to the main port. In lane 0 that gives `myapp.localhost`, `ws.myapp.localhost` and `admin.myapp.localhost`; this lists them with their ports:

```console title="~/dev/myapp"
$ devlanes url ls
```

WebSockets and server-sent events pass through the proxy without a setting. Plain entries in the list are extra hostnames for the main port.

## If the program insists on a fixed port

Set the port to the one the program uses, so devlanes checks and routes that one:

```yaml devlanes
services:
  web:
    env:
      PORT: 3000
```

Only one lane at a time can then run the service. When the program takes its port as a flag, pass `${{ self.port }}` instead and each lane keeps its own.

## Related

- [Ports](../../concepts/ports.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Serve wildcard and tenant subdomains](../hostnames/wildcards.md)
