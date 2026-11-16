---
title: Migrate from another process manager
description: Have a coding agent turn an mprocs, Procfile, process-compose, pm2, tmuxinator, concurrently or Compose setup into a project file.
kind: how-to
---

Use this guide when mprocs, a Procfile, process-compose, pm2, tmuxinator, concurrently or Docker Compose runs your dev processes today. A coding agent with the devlanes skill does the migration; paste this into it from the repository.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then install devlanes on this machine if it is missing, and migrate this repository's dev setup to a devlanes.yaml, keeping the old files. Start the lane, give me the URL of each service, and tell me what did not carry over and what I have to do myself.
```

::example{slug="workflows-mprocs-migration"}

## Check what the agent wrote

The agent keeps the old files, so people who do not use devlanes keep their setup. Its project file follows these rules:

- A process that keeps running becomes a service with a ready check.
- A command that ends by itself, such as an install or a migration, becomes a task.
- A container stays in its Compose file, which the project file runs under `infra`.
- A fixed port becomes the port devlanes gives the service in each lane.

```text title="Procfile.dev"
web: bin/rails server -p $PORT
```

```yaml devlanes
services:
  web:
    run: bin/rails server -p $PORT -b 127.0.0.1
    hosts: "${{ lane.domain }}"
    ready:
      http:
        path: /up
```

## Settings that do not carry over

The agent lists the ones your setup had:

- Restart policies, because a failed service stays failed with its error ([The daemon](../../concepts/daemon.md)).
- Dependencies between services, because services do not wait for each other ([Start order and readiness](../../concepts/start-order.md)).
- Loading `.env` into every process ([Read .env values and pin tool versions](env-files-and-tools.md)).
- Log files and terminal layouts, because devlanes keeps each service's output.

## If you would rather write the file yourself

[Add devlanes to a project](add-devlanes.md) goes through it step by step.

## Related

- [Run scripts your project already has](run-scripts.md)
- [Share one database server between lanes](../infra/shared-database.md)
- [`services.<name>.stop`](/docs/reference/config/project/services/#services-name-stop)
