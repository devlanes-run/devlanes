---
title: Run workers and replicas
description: Run queue consumers and other services without a port, stop them cleanly and run several copies of one.
kind: how-to
---

A queue consumer, a scheduler or another background process runs as a service like any other, with a few settings because nothing sends it requests.

::example{slug="workflows-queue-workers"}

## Wait for a line the worker prints

A service without hostnames counts as ready once its process has run for a moment, which tells you nothing about a worker that cannot reach its queue. Have the worker print one line once it is connected, and make that line its ready check:

```yaml devlanes
services:
  worker:
    run: [node, worker.mjs]
    cwd: worker
    ready:
      log: "worker ready"
```

The check is a regular expression matched against the worker's output, with colours removed. Once the line appears, the worker reads as ready:

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  3/3       0.0%  92.4 MiB  https://myapp.localhost

  UNIT    STATE  PORT   CPU   MEM       URL
  api     ready  20060  0.0%  36.5 MiB  https://api.myapp.localhost
  web     ready  20061  0.0%  43.2 MiB  https://myapp.localhost
  worker  ready  20062  0.0%  12.8 MiB  http://127.0.0.1:20062
```

The worker has a port and a URL although it listens on nothing. devlanes gives each service a port from the lane's block unless the service sets its own, and a worker can leave it unused.

## Let it finish its job on stop

devlanes stops a service with SIGTERM and kills what is left after a short grace period. A worker that finishes its job in hand on another signal, or needs longer, says so:

```yaml devlanes
services:
  worker:
    stop:
      signal: SIGINT
      timeout: 15s
```

The signal goes to the whole process tree, so the worker gets it even when a package manager or a shell started it. Windows has no such signals. A stop there sends Ctrl-C or Ctrl-Break, and [Windows](../../reference/windows.md) says which.

## Keep its lane awake

devlanes freezes a lane that nobody uses for a while, and a frozen worker stops taking jobs. One setting keeps the lane running for as long as the worker runs:

```yaml devlanes
services:
  worker:
    keep_awake: true
```

[Throttling and freezing](../../concepts/freezing.md) says what counts as use.

## Run several copies

The replica count starts that many copies of the service, named `worker.1`, `worker.2` and so on, each with its own port. The replica index tells each copy which one it is, here in a variable and in the line its ready check waits for:

```yaml devlanes
services:
  worker:
    replicas: 2
    env:
      WORKER_ID: ${{ replica.index }}
    ready:
      log: worker ${{ replica.index }} ready
```

In the start, stop, restart and logs commands, the service's name stands for all its copies, so stopping the worker stops both. Name one copy, such as worker.2, to act on it alone. In expressions, the worker's URL and port are those of the first copy.

The count is a plain number, the same in every lane, since each copy reserves ports in every lane's block.

## If the job ends by itself

A command that exits when its work is done, such as a cleanup or a one-off import, is a task. As a service it would show as failed the moment it exits. [Run installs, migrations and seeds](tasks.md) covers tasks.

## Related

- [Wait until a service is ready](ready-checks.md)
- [Run many lanes on one machine](../lanes/many-lanes.md)
- [Throttling and freezing](../../concepts/freezing.md)
