---
title: Run workers and replicas
description: Run queue consumers and other services without a port, stop them cleanly and run several copies of one.
kind: how-to
---

A queue consumer, a scheduler or another background process runs as a service like any other, with a few settings because nothing sends it requests.

::example{slug="workflows-queue-workers"}

## Configure the worker

```yaml devlanes
services:
  worker:
    run: [node, worker.mjs]
    cwd: worker
    ready:
      log: "worker ready"
    stop:
      signal: SIGINT
      timeout: 15s
    keep_awake: true
```

Have the worker print one line once it is connected, and the ready check waits for that line. The stop settings give it the signal it finishes its current job on, and time to do so. Keeping the lane awake stops devlanes from freezing a lane that gets no requests while the worker runs; [Throttling and freezing](../../concepts/freezing.md) explains freezing.

## Run several copies

```yaml devlanes
services:
  worker:
    replicas: 2
    env:
      WORKER_ID: ${{ replica.index }}
    ready:
      log: worker ${{ replica.index }} ready
```

The copies run as worker.1 and worker.2, each with its own port, and the replica index tells each one which it is. The service's name stands for all its copies, so this stops both:

```console title="~/dev/myapp"
$ devlanes stop worker
```

Name one copy, such as worker.2, to act on it alone.

## If the job ends by itself

A command that exits when its work is done, such as a cleanup, is a task; as a service it would show as failed. [Run installs, migrations and seeds](tasks.md) covers tasks.

## Related

- [Wait until a service is ready](ready-checks.md)
- [Run many lanes on one machine](../lanes/many-lanes.md)
- [Windows](../../reference/windows.md)
