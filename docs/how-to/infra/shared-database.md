---
title: Share one database server between lanes
description: Run one Postgres container for every lane of a project and give each lane its own database in it.
kind: how-to
---

Use this when every lane needs a database and one server is enough: one Postgres runs for the whole project, and each lane gets a database of its own in it.

::example{slug="infra-postgres-shared"}

## Add the server to a Compose file

Leave the host port to Docker; devlanes reads it back. The healthcheck lets devlanes wait until Postgres accepts connections.

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

## Create a database per lane

Every Compose service is shared by the lanes unless you list it under [`infra.per_lane`](/docs/reference/config/project/infra/#infra-per_lane). The lane variable names the database, setup creates it on the lane's first start, and teardown drops it when you clean the lane.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}

infra:
  compose: docker-compose.yml
  project_name: ${{ project.name }}
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
```

Lane 0 gets `myapp_main` and lane 1 `myapp_lane1`. To migrate at each start, add a task as in [Run installs, migrations and seeds](../services/tasks.md).

## Start the lanes

Run the start in each worktree. The first one brings Postgres up, and each lane creates its own database.

```console
$ devlanes start
```

## If you stop or replace the container

Stopping lanes leaves Postgres running. To stop it, or to bring it back after you removed it, for example to use a newly pulled image:

```console
$ devlanes infra down --yes
$ devlanes infra up
```

Docker gives the container a new port when it comes back, so run a reload in each running lane to give its services the new address:

```console
$ devlanes reload
```

## If each lane needs its own server

Give each lane a container of its own: [Give each lane its own container](container-per-lane.md).

## Related

- [A database for each lane](../../learn/databases.md)
- [Keep Redis, RabbitMQ, S3 and mail apart per lane](datastores.md)
- [Remove lanes](../lanes/clean-up.md)
