---
title: Run installs, migrations and seeds
description: Run one-off commands when a lane is created, when it starts, once its services are ready, at stop and at clean, or by hand.
kind: how-to
---

A command that runs and ends, such as an install, a migration or a seed, is a task: devlanes runs it at the lane event you choose, or when you name it.

::example{slug="infra-postgres-shared"}

## Add the tasks

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

The install runs on a new lane's first start, until it succeeds once, and again after you change its entry. The migration runs each time the lane comes up, before its services, so the migration tool has to skip what it already applied. The seed runs once the services are ready, for work that goes through the running app. [Start order and readiness](../../concepts/start-order.md) has the whole order.

## Run a task by hand

A task without an event runs only when you name it, and any task can be run that way:

```console title="~/dev/myapp"
$ devlanes run migrate
```

## If a task is destructive

Give it a question, which the run asks before it starts; scripts and agents pass `--yes` instead:

```yaml devlanes
tasks:
  reset-db:
    steps: [infra.teardown.database, infra.setup.database, migrate, seed]
    confirm: Drop the database of ${{ lane.name }} and seed it again?
```

The steps run in order, here the entries that drop and create the lane's database from [Share one database server between lanes](../infra/shared-database.md), then two tasks.

## If something must run at stop or clean

Tasks on `lane.stop` run when you stop the whole lane or the daemon, and tasks on `lane.clean` when you clean the lane. A logout or a shutdown of the machine skips stop tasks, so a dump that has to happen goes in a task you run yourself.

## Related

- [A database for each lane](../../learn/databases.md)
- [Share one database server between lanes](../infra/shared-database.md)
- [Start order and readiness](../../concepts/start-order.md)
