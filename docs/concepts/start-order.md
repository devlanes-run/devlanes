---
title: Start order and readiness
description: A start brings a lane up in fixed phases, from containers to services, waits until each service is ready or has failed, and takes turns with other starts on the machine.
kind: concept
---

When you start a lane, devlanes brings up its parts in a fixed order: the containers, then setup, then tasks, then the services, then the tasks that wait for services. The start returns once each service it started has passed its ready check or failed. Services do not wait for one another, so whatever has to be up first belongs in an earlier phase.

## A start runs in phases

Containers are the databases and other programs a project runs in Docker, such as one Postgres; [Glossary](../reference/glossary.md) has the other terms. Each phase begins once the one before it has ended. devlanes works out each service's command and environment only after the containers are up, because a service's variables can name a port that a container publishes once it runs.

![A start of myapp runs postgres in containers, then setup creates the database, then the install and migrate tasks one after the other, then api and web start together through the start gate with no order between them, then the seed task after services](start-order.svg)

These phases run when a start brings the lane up from nothing running. A start in a lane that already runs only adds the services the lane is missing, such as one you just added to the project file, and runs no tasks. A `lane.start` task you just added therefore waits for the next start that brings the lane up, such as a whole-lane `devlanes restart`.

## Setup prepares containers, tasks prepare the worktree

Setup entries are commands the project file lists for the lane's containers, such as creating the lane's own database in a shared Postgres. devlanes records each entry that succeeded and runs it again only after you edit it or after its container restarted, since a restarted container may have lost what the entry made. The entries of a lane run in parallel.

Tasks are one-off commands bound to a moment in the lane's life, and they run one at a time in the order the project file lists them. A `lane.create` task runs at each start until it has succeeded once for the lane, and again after you edit it: installing dependencies in a new worktree is the usual one. A `lane.start` task runs each time a start brings the lane up, which suits migrations that skip what is already applied.

```yaml devlanes
tasks:
  install:
    run: [npm, install]
    when: lane.create
  migrate:
    run_script:
      file: package.json
      name: db:migrate
    when: lane.start
  seed:
    run_script:
      file: package.json
      name: db:seed
    when: lane.create
    after: services
```

A task marked to run after services, such as a seed that goes through the API, waits until every service of the start is ready, and is skipped when one failed or the start ran out of time. A task that fails ends the start; one that runs before services ends it before any service is spawned. [Run installs, migrations and seeds](../how-to/services/tasks.md) sets these up.

## Ready means the ready check passed

A service with a ready check is ready when that check passes: a port that accepts connections, a route that answers, a line in the log or a command that succeeds. A service without one waits for its port to accept connections when it has a hostname, and otherwise counts as ready once its process has run half a second. [Wait until a service is ready](../how-to/services/ready-checks.md) shows how to choose a check, and [`services.<name>.ready`](/docs/reference/config/project/services/) lists the kinds and their timeouts.

The check runs during the start and not after it. A service that stops answering later still shows as ready until its process exits.

## A check that never passes leaves the process running

When the timeout passes, the service becomes failed with the reason the check last saw, and devlanes neither stops its process nor starts it again. In this lane the API's ready check asked for a path the API does not serve, so the API answered 404 until the timeout.

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

The API still uses memory because its process still runs and still holds port 20060, while the proxy answers its hostname with an error page. The other services went on starting, since one failure does not cancel the rest. With the process left up, its logs and its port are there for you to inspect. It stays failed until you start, restart or stop it, as [The daemon](daemon.md) explains.

## The start gate lets starts take turns

Each service start on the machine waits at the start gate, a queue shared by every lane and project, before it spawns. Replicas and restarts wait there too, and a service that waits shows as `queued`. It holds its turn until its ready check passes or fails, or, without a check, until its CPU use settles. The start gate also has a timeout of its own, after which the next service may go even if this one is not ready, so a service whose code waits for another service cannot hold up the queue.

By default the start gate lets as many services start at once as the machine has CPU cores, and lets fewer through while the machine is short of CPU or memory, as when many lanes start after a reboot. It only delays spawns: a process that runs is not slowed. The [`startup`](/docs/reference/config/user/startup/) settings change it, and `--all-at-once` skips it for one start.

## Services do not wait for each other

A service cannot name another service it depends on, and devlanes does not order services by what they need from each other. Ports are given out before any service spawns, so each service can receive the URL of another service in its lane through an expression even though that service is not listening yet; [Environment and expressions](environment.md) explains how.

For you, the waiting moves into your code. When the database is itself a service, it starts together with the API, and the API has to retry its first connection until the database answers; a web app that calls the API at startup has to do the same. Run the database as a container instead and it is up before any service starts.

## Related

- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [Run many lanes on one machine](../how-to/lanes/many-lanes.md)
- [`startup`](/docs/reference/config/user/startup/)
