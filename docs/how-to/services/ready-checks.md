---
title: Wait until a service is ready
description: Give each service a ready check so a start returns only once the service works.
kind: how-to
---

A start waits for each service's ready check, so a check that tests the real app turns a broken service into a failed start instead of a URL that answers with an error.

::example{slug="basics-ready-checks"}

## Pick a check

Choose the kind by what the service offers:

- A health route: `http`, which passes when the route answers with a 2xx status.
- A port and no route: `tcp`, which passes once the port accepts a connection.
- No port, such as a worker: `log`, which passes when a line of the service's output matches.
- Something only a tool can tell: `cmd`, a command that passes once it exits 0.
- Nothing to probe: `started`.

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
```

Without a ready check, a service with hostnames waits for its port, and one without counts as ready once it has started. The fields of each kind are under [`services.<name>.ready`](/docs/reference/config/project/services/#services-name-ready).

## Give a slow service more time

A service that compiles before it listens needs a longer timeout on its check, and the start needs a longer wait of its own:

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
      timeout: 5m
```

```console title="~/dev/myapp"
$ devlanes start --timeout 5m --no-ui
```

When a check fails, the start says what the check asked and what came back. The service keeps running, and devlanes does not restart it; restart it after the fix. [When a service fails](../../learn/failures.md) follows such a failure to its cause.

## If the app answers only on its own hostname

The check sends the Host header `127.0.0.1:<port>`. An app that refuses any other host needs the check to send its hostname:

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
        host: ${{ self.hosts[0] }}
```

## Related

- [Start order and readiness](../../concepts/start-order.md)
- [Run workers and replicas](workers.md)
- [When a service fails](../../learn/failures.md)
