---
title: Run installs, migrations and seeds
description: Run one-off commands when a lane is created, at each start, once its services are ready, at stop and at clean, or by hand.
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

It runs at each start until it succeeds once for the lane, and again after you change its entry in the project file. An edit to a file it reads does not count as a change.

## Migrate at each start

A task on the lane's start runs every time the lane starts, after its containers and before its services, so the migration tool has to skip what it already applied:

```yaml devlanes
tasks:
  migrate:
    run_script:
      file: package.json
      name: db:migrate
    when: lane.start
```

Tasks of one event run in the order of the file. A task that fails before the services fails the start and prints the end of its log. [Start order and readiness](../../concepts/start-order.md) has the whole order.

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

When a service fails or the start's wait runs out, these tasks are skipped. The first start of a lane reports one line per task. The seed's line sits with the others, although the seed ran only once the API was ready:

```console title="~/dev/myapp"
$ devlanes start --no-ui --output plain
ok infra
ok setup
ok install
ok migrate
ok seed
myapp@0 · main
ok services  1 of 1 ready
  ok  api  https://myapp.localhost
```

## Run a task by hand

A task without an event runs only when you name it, and any task can be run that way. Its output streams to the terminal, and the command exits with the task's exit code:

```console title="~/dev/myapp"
$ devlanes run migrate
```

`devlanes run --list` lists the tasks of the project.

## Combine steps and ask first

A task can be a list of other tasks and setup or teardown entries, run in order until one fails. Give a destructive one a question:

```yaml devlanes
tasks:
  reset-db:
    steps: [infra.teardown.database, infra.setup.database, migrate, seed]
    confirm: Drop ${{ lane.db_name }} and seed it again?
```

On a terminal, the run asks the question and waits. Scripts and agents have no terminal to ask on, so the run stops unless they pass `--yes`.

## At stop and at clean

A task on the lane's stop runs when the whole lane stops, before its processes do: a stop without unit names, or the daemon stopping or restarting. A task on the lane's clean runs when you clean the lane, after its processes stop and before the teardown entries remove its data from the containers.

```yaml devlanes
tasks:
  dump:
    run: ./scripts/dump-db.sh
    when: lane.stop
```

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
