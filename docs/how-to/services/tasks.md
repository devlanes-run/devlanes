---
title: Run installs, migrations and seeds
description: Run one-off commands when a lane is created, when it starts, once its services are ready, at stop and at clean, or by hand.
kind: how-to
---

A command that runs and ends, such as an install, a migration or a seed, is a task: devlanes runs it at the lane event you choose, or when you ask for it by name.

::example{slug="infra-postgres-shared"}

## Install dependencies in a new lane

A new worktree has no installed dependencies. A task on the lane's creation runs before the lane's first start reaches its services:

```yaml devlanes
tasks:
  install:
    run: [npm, install]
    when: lane.create
```

It runs at each start of the lane until it succeeds once, and again after you change its entry in the project file. A change to a file it reads, such as the lockfile, does not count.

## Migrate when the lane starts

A task on the lane's start runs each time the lane comes up, after its containers and before its services, so the migration tool has to skip what it already applied:

```yaml devlanes
tasks:
  migrate:
    run_script:
      file: package.json
      name: db:migrate
    when: lane.start
```

Starting one more service into a lane that already runs runs no tasks. Tasks of both events run in the order the file lists them, and one that fails before the services fails the start and prints the end of its log. [Start order and readiness](../../concepts/start-order.md) has the whole order.

## Seed once the services are ready

A seed that goes through the running API has to wait for it. Moving the task after the services runs it once every service the start brought up is ready:

```yaml devlanes
tasks:
  seed:
    run_script:
      file: package.json
      name: db:seed
    when: lane.create
    after: services
    env:
      API_URL: ${{ services.api.url }}
```

When a service fails, or the start stops waiting before every service is ready, these tasks are skipped.

## Run a task by hand

A task without an event runs only when you name it with `devlanes run`, and every other task can be run that way too. The task's output streams to your terminal, and the command exits with the task's exit code.

A task can also be a list of steps: other tasks and setup or teardown entries, run in order until one fails. Give a destructive one a question to ask first:

```yaml devlanes
tasks:
  reset-db:
    steps: [infra.teardown.database, infra.setup.database, migrate, seed]
    confirm: Drop the database of ${{ lane.name }} and seed it again?
```

Here `infra.teardown.database` and `infra.setup.database` name the entries that drop and create the lane's database, as [Share one database server between lanes](../infra/shared-database.md) sets them up.

On a terminal, the run asks the question and waits. Scripts and agents have no terminal to ask on, so the run stops unless they pass `--yes`.

## At stop and at clean

A task on the lane's stop runs when the whole lane stops, before its processes do: a stop without unit names, a clean, or `devlanes daemon stop` and `daemon restart`. A task on the lane's clean runs after the processes stop and before the teardown entries remove the lane's data from the containers.

```yaml devlanes
tasks:
  dump:
    run: ./scripts/dump-db.sh
    when: lane.stop
```

:::caution
A logout or a shutdown of the machine ends the daemon without running the stop tasks, so a dump that has to happen goes in a task you run yourself.
:::

## If the task needs your terminal

A login, a prompt or a TUI needs your keyboard. Mark the task interactive and run it by hand; an interactive task cannot have an event.

```yaml devlanes
tasks:
  login:
    run: gh auth login
    interactive: true
```

## Related

- [A database for each lane](../../learn/databases.md)
- [Share one database server between lanes](../infra/shared-database.md)
- [Start order and readiness](../../concepts/start-order.md)
