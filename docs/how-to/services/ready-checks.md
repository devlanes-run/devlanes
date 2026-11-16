---
title: Wait until a service is ready
description: Give each service a ready check so a start returns only once the service works.
kind: how-to
---

A start waits for each service's ready check, so a check that tests the real app turns a broken service into a failed start you can see, instead of a URL that answers with an error.

::example{slug="basics-ready-checks"}

## Pick a check for the service

devlanes repeats a ready check until it passes or its timeout runs out. Choose the kind by what the service offers:

- A health route: `http`, which passes when the route answers with a 2xx status.
- A port and no route: `tcp`, which passes once the port accepts a connection.
- No port, such as a worker: `log`, which passes when a line of the service's output matches.
- Something only a tool can tell: `cmd`, a check written as a command, which passes once the command exits 0.
- Nothing to probe: `started`.

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
```

Without a ready check, a service with hostnames waits for its port, and one without counts as ready once its process has run for a moment. A route that reads from the database tells you more than a port does, since an open port only proves the socket is there. The fields of each kind are under [`services.<name>.ready`](/docs/reference/config/project/services/#services-name-ready) in the config reference, and [Run workers and replicas](workers.md) shows the log check on a worker.

## Give a slow service more time

A service that compiles before it listens, such as a Rust binary or a Python environment that syncs first, needs a longer timeout:

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
      timeout: 5m
```

The start has a limit of its own, shorter than this check. For an API that takes longer than that to listen, a plain start gives up while the service goes on starting, and exits with code 124, the timeout code in [States and exit codes](../../reference/states-and-exit-codes.md):

```console title="~/dev/myapp"
$ devlanes start --no-ui
myapp@0 · main
! services  0 of 1 ready
  starting  api
! timed out before every unit was ready
```

Give the start a longer wait too, and it returns once the service is ready:

```console title="~/dev/myapp"
$ devlanes start --timeout 5m --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
```

## When the check never passes

A check that asks the wrong question fails after its timeout. The message shows the request the check made and what came back, and the hint names the field to change:

```console title="~/dev/myapp"
$ devlanes restart api --no-ui
myapp@0 · main
✗ services  0 of 1 ready
  ✗  api  wasn't ready in time; GET /health (Host: 127.0.0.1:20060) answered 404   api listening on http://127.0.0.1:20060

hint: use a `ready.http.path` that answers 2xx, set `ready.http.host` if the app routes by hostname, or set `ready.http.status: 404`
```

Here the path was wrong. The check does not follow redirects, so a route that redirects to a login page needs its status set to 302. The service keeps running and holds its port after a failed check, and devlanes does not restart it; a restart after the fix runs the check again.

## If the app answers only on its own hostname

The check sends its request to the service's port with the Host header `127.0.0.1:<port>`. An app that refuses any other host, as Django's allowed hosts do, needs the check to send its hostname instead:

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
        host: ${{ self.hosts[0] }}
```

When a check needs more than a path and a header, write it as a command check that runs `curl` or a script of your own.

## If the dev server runs under a file watcher

A watcher such as `node --watch` stays alive after the program crashes and waits for the next file change, so a check that only asks whether the process started reports it ready. A check of the route keeps failing and the start reports the service failed. [When a service fails](../../learn/failures.md) follows such a failure to its cause.

## Related

- [Start order and readiness](../../concepts/start-order.md)
- [Run workers and replicas](workers.md)
- [When a service fails](../../learn/failures.md)
