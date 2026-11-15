---
title: Give a service several ports and hostnames
description: Give one service extra ports by name, route hostnames to them, and handle a program that only listens on a fixed port.
kind: how-to
---

When one process listens on more than one port, such as an app with a separate WebSocket or admin server, each port can get a name of its own and a hostname that reaches it.

::example{slug="web-realtime-ws"}

## Name a second port

Mention a port by name anywhere in the service and devlanes allocates it, next to the service's main port and with the same offset in every lane. The service learns the number from a variable you pass it:

```yaml devlanes
services:
  web:
    run_script:
      file: package.json
      name: dev
    env:
      WS_PORT: ${{ self.ports.ws }}
      ADMIN_PORT: ${{ self.ports.admin }}
```

The main port stays in `PORT`. Another service reads a named port as `${{ services.web.ports.ws }}`, but only the service that owns the port allocates it.

## Route a hostname to it

A hostname that names a port goes to that port; a plain one goes to the main port. Both kinds sit in one list:

```yaml devlanes
services:
  web:
    hosts:
      - ${{ lane.domain }}
      - host: ws.${{ lane.domain }}
        port: ws
      - host: admin.${{ lane.domain }}
        port: admin
```

In lane 0 the main port answers on `myapp.localhost` and the ws port on `ws.myapp.localhost`, and the admin port follows the same pattern.

`devlanes url ls` lists each hostname of the project with the port it reaches. WebSockets and server-sent events pass through the proxy on any of these hostnames without a setting.

## Wait for the right port

The ready check goes to the main port. When the program opens its ports one after another, open the main port last, or point a port check at the named one:

```yaml devlanes
services:
  web:
    ready:
      tcp: ws
```

## Give one service several hostnames

Plain entries in the same list are several hostnames for the main port, such as an app that serves a second brand on its own name. The first one without a wildcard becomes the service's public URL, the one other services read from it.

```yaml devlanes
services:
  web:
    hosts:
      - ${{ lane.domain }}
      - shop.${{ lane.domain }}
```

Keep every name under the lane's domain, so each lane gets its own and two lanes never claim the same hostname. [Serve wildcard and tenant subdomains](../hostnames/wildcards.md) covers names with a wildcard.

## If the program insists on a fixed port

A program that cannot be told its port listens where it wants, while the ready check and the proxy look for it on the port devlanes allocated. Set the port to the one the program uses, so devlanes checks and routes that one:

```yaml devlanes
services:
  web:
    env:
      PORT: 3000
```

The number is then the same in every lane, so only one lane at a time can run the service; a start in a second lane fails on the taken port. When the program takes its port as a flag instead, pass `${{ self.port }}` in the command and each lane keeps its own.

## Related

- [Ports](../../concepts/ports.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Serve wildcard and tenant subdomains](../hostnames/wildcards.md)
