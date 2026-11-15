---
title: Wait until a service is ready
description: Give each service a ready check so a start returns only once the service works.
kind: how-to
---

A start waits for each service's ready check, so a check that tests the real app turns a broken service into a failed start you can see, instead of a URL that answers with an error.

::example{slug="basics-ready-checks"}

## Pick a check for the service

A ready check is retried until it passes or its timeout runs out, 60 seconds unless you set one. Choose the kind by what the service offers:

- A health route: `http`, which passes when the route answers with a 2xx status.
- A port and no route: `tcp`, which passes once the port accepts a connection.
- No port, such as a worker: `log`, which passes when a line of its output matches.
- A check that needs a tool: `cmd`, which passes once the command exits 0.
- Nothing to probe: `started`.

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
  worker:
    ready:
      log: "worker ready"
```

A service without a ready check waits for its port when it has hostnames, and counts as ready as soon as it starts when it has none. A route that reads from the database tells you more than a port does, since a port only proves the socket is open. The fields of each kind are under [`ready`](/docs/reference/config/project/services/) in the config reference.

## Give a slow service more time

A service that compiles before it listens, such as a Rust binary or a Python environment that syncs first, needs a longer timeout.

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
      timeout: 3m
```

The start itself waits three minutes by default. When the check takes longer, the start returns while the service goes on starting, and `devlanes wait api` follows it until it is ready or failed.

## Check the app behind a file watcher

A dev server under a file watcher, such as `node --watch`, stays alive after the program crashes and waits for the next file change. A service checked only for having started is then reported ready. An `http` check keeps failing instead, and the start reports the service failed with the watcher's last line:

```console title="~/dev/myapp"
$ devlanes restart api --no-ui
myapp@0 · main
✗ services  0 of 1 ready
  ✗  api  wasn't ready in time   Failed running 'server.mjs'. Waiting for file changes before restarting...

logs: devlanes logs myapp@0:api --tail 200
```

The error itself is earlier in the log the last line names. [When a service fails](../../learn/failures.md) follows this failure to its cause.

## When the check never passes

A check that asks the wrong question fails the same way, after its timeout. The message shows the request the check made and what came back, and the hint names the field to change.

```console title="~/dev/myapp"
$ devlanes restart api --no-ui
myapp@0 · main
✗ services  0 of 1 ready
  ✗  api  wasn't ready in time; GET /health (Host: 127.0.0.1:20060) answered 404   api listening on http://127.0.0.1:20060

hint: use a `ready.http.path` that answers 2xx, set `ready.http.host` if the app routes by hostname, or set `ready.http.status: 404`
```

Here the path was wrong. The check does not follow redirects, so a route that redirects to a login page needs its status set to 302. The process keeps running after a failed check, and devlanes does not restart it; a restart after the fix runs the check again.

## If the app answers only on its own hostname

The check sends the request to the service's port with the Host header `127.0.0.1:<port>`. An app that refuses any other host, as Django's allowed hosts do, needs the check to send its hostname instead:

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
        host: ${{ self.hosts[0] }}
```

When a check needs more than a path and a header, write it as a `cmd` that runs `curl` or a script of your own.

## Related

- [Start order and readiness](../../concepts/start-order.md)
- [Run workers and replicas](workers.md)
- [When a service fails](../../learn/failures.md)
