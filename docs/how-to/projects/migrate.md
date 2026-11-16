---
title: Migrate from another process manager
description: Have a coding agent turn an mprocs, Procfile, process-compose, pm2, tmuxinator, concurrently or Compose setup into a project file.
kind: how-to
---

Use this guide when mprocs, a Procfile, process-compose, pm2, tmuxinator, concurrently or Docker Compose runs your dev processes today. A coding agent with the devlanes skill reads that setup and the scripts it calls, writes the project file, starts the lane and tells you what did not carry over. Paste this into it from the repository; [Set up a coding agent](../agents/set-up.md) covers each agent host.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then install devlanes on this machine if it is missing, and migrate this repository's dev setup to a devlanes.yaml, keeping the old files. Start the lane, give me the URL of each service, and tell me what did not carry over and what I have to do myself.
```

::example{slug="workflows-mprocs-migration"}

## Map the entries

The agent follows the same rules for every tool, and you can check its file against them:

- A process that keeps running becomes a service.
- A command that ends by itself, such as an install, a migration or a seed, becomes a task that runs on a lane's first start, before every start, or when you ask for it.
- A container, or a process that runs one, becomes infra: its Compose file under `infra`, or a container declared in the project file.
- A fixed port becomes the port devlanes gives the service in each lane, so two lanes do not ask for the same one.
- Each service gets a ready check that fails when the app is broken.

The old files stay where they are, so people who do not use devlanes keep their setup.

::::tabs
:::tab[mprocs]
```yaml title="mprocs.yaml"
procs:
  api:
    shell: npm run dev
    cwd: api
  migrate:
    shell: npm run migrate
    cwd: api
    autostart: false
```

```yaml devlanes
services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
tasks:
  migrate:
    run_script:
      file: api/package.json
      name: migrate
    when: lane.start
```

A migration becomes a task that runs before the services on every start. A proc you started by hand now and then, such as a reset, becomes a task without `when`, which runs only when you ask for it ([Run installs, migrations and seeds](../services/tasks.md)).
:::
:::tab[Procfile]
```text title="Procfile.dev"
web: bin/rails server -p $PORT
worker: bundle exec sidekiq
```

```yaml devlanes
services:
  web:
    run: bin/rails server -p $PORT -b 127.0.0.1
    hosts: "${{ lane.domain }}"
    ready:
      http:
        path: /up
  worker:
    run: bundle exec sidekiq
```

`$PORT` keeps working, because devlanes sets it to the service's port. foreman, overmind, hivemind and honcho all read this file the same way.
:::
:::tab[process-compose]
```yaml title="process-compose.yaml"
processes:
  api:
    command: go run ./cmd/api --port 8080
    working_dir: ./services/api
    readiness_probe:
      http_get:
        port: 8080
        path: /health
```

```yaml devlanes
services:
  api:
    run: go run ./cmd/api --port ${{ self.port }}
    cwd: services/api
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /health
```

A probe on the process's own port becomes its ready check. A process that others wait for until it completes becomes a task.
:::
:::tab[pm2]
```json title="ecosystem.config.json"
{ "apps": [{ "name": "api", "script": "./api/server.js", "args": "--port 4000", "instances": 2 }] }
```

```yaml devlanes
services:
  api:
    run: node ./api/server.js --port ${{ self.port }}
    replicas: 2
```

Each replica gets a port of its own, so cluster mode, where instances share one port, has no counterpart. [Run workers and replicas](../services/workers.md) covers replicas.
:::
:::tab[tmuxinator]
```yaml title=".tmuxinator.yml"
on_project_first_start: bin/setup
windows:
  - editor: vim
  - server: bundle exec rails s -p 3000
```

```yaml devlanes
services:
  server:
    run: bundle exec rails s -p ${{ self.port }} -b 127.0.0.1
    hosts: "${{ lane.domain }}"
    ready:
      http:
        path: /up
tasks:
  setup:
    run: bin/setup
    when: lane.create
```

A pane with an editor or a shell is left out. The first-start hook becomes a task that runs once in each new lane. tmuxp files map the same way.
:::
:::tab[concurrently]
```json title="package.json"
{ "scripts": { "dev": "concurrently \"npm:dev:api\" \"npm:dev:web\"" } }
```

```yaml devlanes
services:
  api:
    run_script:
      file: package.json
      name: dev:api
  web:
    run_script:
      file: package.json
      name: dev:web
```

Each script it starts becomes a service, so each one gets its own port, log and state. npm-run-all and run-p scripts map the same way.
:::
:::tab[Compose]
```yaml devlanes
infra:
  compose: docker-compose.yml
  exclude: [caddy]
```

The containers keep their Compose file, and the lanes share them unless a service is listed as one per lane. A local reverse proxy is left out, because the devlanes proxy serves the hostnames. [Share one database server between lanes](../infra/shared-database.md) gives each lane its own database.
:::
::::

## Settings that do not carry over

The agent drops these and lists the ones your setup had:

- Restart policies and the process manager's own file watching, because a failed service stays failed with its error ([The daemon](../../concepts/daemon.md)). A watch flag in the command itself, such as `node --watch`, keeps working.
- Dependencies between services, because services do not wait for each other ([Start order and readiness](../../concepts/start-order.md)).
- Stop commands and key sequences other than Ctrl-C, because devlanes stops a service with a signal and a grace time you choose ([`services.<name>.stop`](/docs/reference/config/project/services/#services-name-stop)).
- Loading `.env` into every process, because devlanes reads it only where the project file names a value ([Read .env values and pin tool versions](env-files-and-tools.md)).
- Workarounds that turn off certificate checks in Node, Bun, Deno, Python and Rust services, because devlanes points those runtimes at its CA ([Run Node, Bun, Deno, Python and Rust services](runtimes.md)). For another runtime, such as Ruby, the agent points the runtime's own setting at the devlanes CA bundle instead.
- Log files, scrollback and layouts, because the daemon keeps each service's output ([`devlanes logs`](../../reference/cli/logs.md)).

## If the repository has several of these files

The agent merges them into one project file. When two of them start the same process in different ways, it asks you which one wins.

## If you would rather write the file yourself

[Add devlanes to a project](add-devlanes.md) goes through it step by step, and the tabs above show what each entry turns into.
