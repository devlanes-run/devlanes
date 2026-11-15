---
title: Start order and readiness
description: A start brings a lane up in fixed phases, waits until each service it started is ready or has failed, and takes turns with other starts on the machine.
kind: concept
---

A lane is made of containers, setup commands, tasks and services, and a start brings them up in that order. It returns once each service it started has passed its ready check or failed. The order comes from the kind of each unit, so services do not wait for one another, and a unit that fails stays failed until you act.

## A start runs in phases

Each phase begins once the one before it has ended. Containers come first because the rest of the lane may need them: setup creates a database inside one, and a service's variables can name a port the container only publishes once it runs. That is why devlanes works out every service's command and environment after the containers are up and before setup begins.

![A start runs containers, setup, tasks, services, then tasks after services, each phase once the one before it ended](start-order.svg)

Setup runs the first time a lane starts, with its entries side by side, and again for an entry that you edit or whose container restarted. Tasks bound to a lane event run one at a time, in the order the project file lists them: a `lane.create` task until it has succeeded once for the lane, a `lane.start` task on each whole start. A task that fails ends the start before any service is spawned. A start that only adds a service to a lane already running skips the tasks.

## Ready means the ready check passed

The services phase starts the selected services together, each as soon as the machine's start gate gives it a slot, and then waits. A service with a ready check is ready when that check passes: a port that accepts connections, a route that answers, a line in the log or a command that succeeds. A service without one counts as ready once its process is up. [Wait until a service is ready](../how-to/services/ready-checks.md) shows how to choose a check, and [`ready`](/docs/reference/config/project/services/) in the reference lists the kinds with their timeouts.

The check runs during the start and not after it. devlanes does not keep probing a service that passed, so a service that stops answering later still shows as ready until its process exits. Tasks marked to run after services wait for the end of this phase, and are skipped when a service failed or the start ran out of time.

## A check that never passes leaves the process running

When the timeout passes, the service becomes failed with the reason the check last saw. devlanes does not stop the process or start it again. In this lane the API's ready check asked for a path the API does not serve, so it answered 404 until the timeout.

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE   SERVICES  CPU   MEM       URL
  0     main    failed  2/3       0.0%  94.4 MiB  https://myapp.localhost

  UNIT    STATE   PORT   CPU   MEM       URL
  api     failed  20060  0.0%  38.5 MiB  https://api.myapp.localhost
  web     ready   20061  0.0%  43.1 MiB  https://myapp.localhost
  worker  ready   20062  0.0%  12.7 MiB  http://127.0.0.1:20062
  api: wasn't ready in time; GET /health (Host: 127.0.0.1:20060) answered 404
```

The API still has memory in use because its process still runs and still holds port 20060. The proxy answers its hostname with an error page instead of passing requests to a service that never proved it works. The other services of the start went on starting, since one failure does not cancel the rest. Leaving the process up means its logs and its port are there to inspect, and a restart, a stop or a new start is your decision.

## Starts across the machine take turns

Unless pacing is turned off, each service start on the machine, from any lane or project, replicas and restarts included, asks a shared gate for a slot before it spawns. A service that waits for one shows as `queued`. It keeps its slot until its ready check passes or fails, or until a slot timeout passes; a service without a check keeps it until its CPU use settles. The timeout exists so that services waiting on each other cannot hold up the queue.

The default gate allows as many starts at once as the machine has CPU cores, and halves that under CPU or memory pressure. The point is a machine that stays usable when many lanes start together, as after a reboot. The gate only delays spawns: a process that runs is not slowed or capped. The [`startup`](/docs/reference/config/user/startup/) settings change the pacing, and `--all-at-once` skips the gate for one start.

## Services do not wait for each other

A service cannot name another service it depends on. The ordering devlanes offers is by kind: containers, setup and tasks come before services, and tasks after services come after them. Within the services phase, services start together, held back only by the start gate.

What makes that workable is that ports are given out before any service spawns. Each service can receive the URL of another service in its lane through an expression, even though that service is not listening yet; [Environment and expressions](environment.md) explains how. The cost falls on a service whose code calls another at startup: it has to wait or retry until the other answers, as it would once deployed.

## A failed service stays failed

A service that exits, or whose ready check fails, stays failed with its reason until a start, a restart or a stop. devlanes does not restart it. The rule exists so the failure and its error stay in front of you, instead of a process that comes back and hides what broke. [When a service fails](../learn/failures.md) follows one such failure to its cause.

A dev server can still restart its own code, as a file watcher does after an edit. devlanes does not count that as a start, so the state it shows is the one the last start decided. Bringing a lane's saved services back after a reboot is a start you ask for, and [The daemon](daemon.md) covers it.

## Related

- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Run many lanes on one machine](../how-to/lanes/many-lanes.md)
- [`startup`](/docs/reference/config/user/startup/)
