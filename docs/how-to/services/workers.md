---
title: Run workers and replicas
description: Run queue consumers and other services without a port, stop them cleanly and run several copies of one.
kind: how-to
---

A queue consumer, a scheduler or another background process runs as a service like any other, with a few settings because nothing sends it requests.

::example{slug="workflows-queue-workers"}

## Wait for a line the worker prints

A service without hostnames counts as ready as soon as it starts, which tells you nothing about a worker that cannot reach its queue. Have the worker print one line once it is connected, and make that line its ready check:

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

The worker has a port and a URL although it listens on nothing. devlanes gives every service a port from the lane's block; a worker can leave it unused.

## Let it finish its job on stop

devlanes stops a service with SIGTERM and kills what is left after 10 seconds. A worker that finishes its job in hand on another signal, or needs longer, says so:

```yaml devlanes
services:
  worker:
    stop:
      signal: SIGINT
      timeout: 15s
```

The signal goes to the whole process tree. A worker that catches it can log what it does before it exits:

```console title="on Windows 11"
$ devlanes logs worker.1 worker.2 --tail 8
worker.2#15 worker.2 got SIGINT, finishing job 9 first
worker.1#15 worker.1 got SIGINT, finishing job 10 first
worker.1#17 worker.1 finished job 10 in 2000 ms
worker.2#17 worker.2 finished job 9 in 2001 ms
worker.1#18 worker.1 stopped cleanly, jobs:main keeps the waiting jobs for the next start
worker.2#18 worker.2 stopped cleanly, jobs:main keeps the waiting jobs for the next start
```

Windows has no such signals. There SIGINT and SIGTERM both go out as Ctrl-C, which Node raises as its SIGINT event, and [Windows](../../reference/windows.md) covers the other signals.

## Keep its lane awake

A lane that gets no requests is suspended after a while, and a suspended worker stops taking jobs. One setting keeps the lane running for as long as the worker runs:

```yaml devlanes
services:
  worker:
    keep_awake: true
```

[Throttling and freezing](../../concepts/freezing.md) says what else counts as activity.

## Run several copies

The replica count starts that many copies of the service, named `worker.1`, `worker.2` and so on, each with its own port. The replica index tells each copy which one it is, here in a variable and in the line its ready check waits for:

```yaml devlanes
services:
  worker:
    run_script:
      file: apps/worker/package.json
      name: dev
    replicas: 2
    env:
      WORKER_ID: ${{ replica.index }}
    ready:
      log: worker ${{ replica.index }} ready
```

Commands that take the service's name act on every copy, so `devlanes stop worker` stops both. Name one copy to act on it alone, as the logs command above does. In expressions, `services.worker` means the first copy.

The count is a plain number, the same in every lane, since each copy reserves ports in every lane's block.

## If the job ends by itself

A command that exits when its work is done, such as a cleanup or a one-off import, is a task. As a service it would show as failed the moment it exits. [Run installs, migrations and seeds](tasks.md) covers tasks.

## Related

- [Wait until a service is ready](ready-checks.md)
- [Run many lanes on one machine](../lanes/many-lanes.md)
- [Throttling and freezing](../../concepts/freezing.md)
