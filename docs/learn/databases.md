---
title: A database for each lane
description: One Postgres container serves every lane of myapp, and each lane gets a database of its own in it.
kind: learn
---

The API of myapp is about to keep data in Postgres. One Postgres container serves both lanes with a database for each, so a migration you try on the feature branch runs against that branch's database and leaves the main branch's alone.

## One container serves every lane

Postgres runs from a Compose file next to the project file. devlanes starts the containers a project lists under `infra` before anything else in a lane, and waits until their healthcheck passes.

```yaml title="docker-compose.yml"
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

The port mapping leaves the host port to Docker, and devlanes reads back the one Docker chose. The lanes share the container, so it keeps running while you stop them.

## Each lane computes the name of its database

You add these lines to the project file on the main branch. Under `infra` they name the Compose file and its project.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}

infra:
  compose: docker-compose.yml
  project_name: myapp
  setup:
    database: |
      docker compose exec -T postgres psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
      SELECT 'CREATE DATABASE ${{ lane.db_name }}'
      WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${{ lane.db_name }}')\gexec
      SQL
  teardown:
    database: docker compose exec -T postgres dropdb -U postgres --if-exists --force ${{ lane.db_name }}

global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}

tasks:
  migrate:
    run: docker compose exec -T postgres psql -U postgres -d ${{ lane.db_name }} -v ON_ERROR_STOP=1 -c "CREATE TABLE IF NOT EXISTS notes (id serial PRIMARY KEY, body text NOT NULL)"
    when: lane.start
```

Each lane computes `lane.db_name` from its own name: myapp_main in lane 0 and myapp_lane1 in lane 1. `global_env` gives every service and task of the lane a database URL built from that name and the port Docker chose.

## Setup creates the database once, the migration runs at each start

Put a step in setup when it prepares data inside a container and a clean should undo it, such as a database, and make it a `lane.create` task when it prepares the worktree itself, such as its packages.

| Kind | For | Runs | Runs again | Undone by | In myapp |
|---|---|---|---|---|---|
| Setup entry | Data inside the containers | At the lane's first start, after the containers | When its entry changes | Its teardown, at a clean | `database` |
| `lane.create` task | The worktree | At each start of a stopped lane until it succeeds | When its entry changes | Nothing | `install` |
| `lane.start` task | Work due at each start | At each start of a stopped lane | At every such start | Nothing | `migrate` |

Both lanes are stopped, and you merge main into the checkout branch. Then you start each lane.

```console title="~/dev/myapp"
$ devlanes start --no-ui
```

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
```

Each start brings up Postgres if it is not running yet, runs the lane's setup, which creates its database, then the migration, then the services; the install already succeeded in both lanes. A later start of a stopped lane skips setup and runs the migration again, so the migration is written to skip what already exists.

## Each lane connects to its own database

```console title="~/dev/myapp-checkout"
$ devlanes env show api
```

The API of lane 1 gets a `DATABASE_URL` ending in myapp_lane1; the API of lane 0 gets myapp_main in the same container.

A stopped lane keeps its database. `devlanes lane clean` deletes a lane and runs its teardown, so cleaning lane 1 drops myapp_lane1 while lane 0 keeps myapp_main.

Both lanes keep running, each on its own database, so when the API breaks on the feature branch in [When a service fails](failures.md), the main branch goes on as it was.

## On your own project

- [Share one database server between lanes](../how-to/infra/shared-database.md)
- [Give each lane its own container](../how-to/infra/container-per-lane.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Start order and readiness](../concepts/start-order.md)
