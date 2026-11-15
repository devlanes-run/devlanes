---
title: A database for each lane
description: One Postgres container serves every lane of myapp, and each lane gets a database of its own in it.
kind: learn
---

The API of myapp starts keeping data in Postgres. Both lanes need a database, and a migration tried on the feature branch must not touch the data of the main branch. One Postgres serves them all, with a database for each lane inside it.

## One container serves every lane

Postgres runs from a Compose file next to the project file. The project file names it under `infra`, which holds the containers a project depends on. devlanes starts them before anything else in the lane and waits until the healthcheck passes.

```yaml
# docker-compose.yml
services:
  postgres:
    image: postgres:17.6-alpine
    environment:
      POSTGRES_PASSWORD: postgres
    ports: ["127.0.0.1::5432"]
    volumes: [pgdata:/var/lib/postgresql/data]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 2s
      retries: 30

volumes:
  pgdata:
```

The port mapping leaves the host port to Docker, and devlanes reads back the one Docker chose. One container is shared by the lanes of the project, so a stop of a lane leaves it running. Containers need Docker with Compose v2.

## Each lane computes the name of its database

A lane can compute values of its own from expressions, and the rest of the file reads them like the lane's domain. Here the value is the database name. The database URL is built from it and from the port Docker gave the container.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}

global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}
```

The name is `myapp_main` in lane 0 and `myapp_lane1` in lane 1, after the lane names. A variable under `global_env` reaches every service and task of the lane, so the API and the migration task below both connect to the same database.

## Setup creates the database, a task migrates it

Two kinds of step prepare the database, and they run at different times. Setup belongs to the containers and runs once per lane. The migration is a task that runs at every start.

```yaml devlanes
infra:
  compose: docker-compose.yml
  project_name: ${{ project.name }}
  setup:
    # Postgres has no CREATE DATABASE IF NOT EXISTS; \gexec runs the CREATE only when the SELECT returns a row.
    database: |
      docker compose exec -T postgres psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
      SELECT 'CREATE DATABASE ${{ lane.db_name }}'
      WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${{ lane.db_name }}')\gexec
      SQL

tasks:
  migrate:
    run_script:
      file: api/package.json
      name: db:migrate
    when: lane.start
```

devlanes records a setup entry as done once it succeeds, and later starts of the lane skip it. devlanes points the setup command's Compose calls at the shared container, and the Postgres client runs inside it, so the machine needs none. The fixed project name lets `docker compose -p myapp` reach the same container from a terminal outside devlanes. A task on the lane start runs again each time, so the migration script has to skip what is already applied.

## A start brings them up in order

This start ran with plain output, which prints ok where a terminal shows a check mark. Its lines come in the order devlanes ran the steps.

```console title="~/dev/myapp"
$ devlanes start --no-ui --output plain
ok infra
ok setup
ok migrate
```

Containers come first, then setup, then tasks, then services, so the API starts against a database that exists and is migrated. A step that fails stops the start before the services. [Start order and readiness](../concepts/start-order.md) has the full order.

## Cleaning a lane drops its database

A lane outlives a stop. Its database, ports and data folder stay for the next start, until the lane is cleaned with `devlanes lane clean`. Cleaning runs the teardown entries, which remove what setup made.

```yaml devlanes
infra:
  teardown:
    database: docker compose exec -T postgres dropdb -U postgres --if-exists --force ${{ lane.db_name }}
```

Cleaning lane 1 drops `myapp_lane1`, while lane 0 keeps running on its own database in the same container. When the last lane of the project is cleaned, the container stops, and its volume keeps the data.

:::caution
Cleaning a lane deletes its data folder and logs as well as its database.
:::

## On your own project

- [Share one database server between lanes](../how-to/infra/shared-database.md)
- [Give each lane its own container](../how-to/infra/container-per-lane.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Start order and readiness](../concepts/start-order.md)
