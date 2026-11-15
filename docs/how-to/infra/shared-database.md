---
title: Share one database server between lanes
description: Run one Postgres or MySQL container for every lane of a project and give each lane its own database in it.
kind: how-to
---

When every lane of a project needs a database and one server is enough, you run that server once and give each lane a database of its own in it, created when the lane is first started and dropped when you clean it.

::example{slug="infra-postgres-shared"}

## Add the server to a Compose file

Leave the host port to Docker: devlanes reads back the port Docker picked and hands it to the lanes. Give the service a healthcheck, because devlanes waits for it before anything in the lane connects.

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

## Point the project file at it

A Compose service is shared by every lane of the project unless you list it under [`infra.per_lane`](/docs/reference/config/project/infra/). A fixed project name lets `docker compose -p myapp` reach the same container from a terminal outside devlanes.

```yaml devlanes
infra:
  compose: docker-compose.yml
  project_name: ${{ project.name }}
```

devlanes runs the shared Compose project from the files of the primary worktree, with the `.env` next to its project file. An edit to the Compose file on another branch does not reach the shared container.

## Name a database for each lane

A lane variable computes the name once, and the commands and the URL below all read it, so the database a lane creates is the one its clean drops.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}
```

Lane 0 gets `myapp_main` and lane 1 `myapp_lane1`.

## Create it in setup and drop it in teardown

Setup entries run after the containers are healthy, and devlanes points their `docker compose` calls at the shared project, so the client inside the container does the work and your machine needs no Postgres tools. Teardown entries run when you clean the lane.

```yaml devlanes
infra:
  setup:
    database: |
      docker compose exec -T postgres psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
      SELECT 'CREATE DATABASE ${{ lane.db_name }}'
      WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${{ lane.db_name }}')\gexec
      SQL
  teardown:
    database: docker compose exec -T postgres dropdb -U postgres --if-exists --force ${{ lane.db_name }}
```

devlanes records a setup entry as done and skips it on later starts, until you change the entry in the project file. `devlanes run infra.setup.database` runs it again by hand, which is why the command checks before it creates.

## Give the services the connection URL

The URL joins the port Docker gave the container and the lane's database name. A variable under `global_env` reaches every service and task of the lane.

```yaml devlanes
global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}
```

`devlanes env show` prints the URL a lane's processes get.

## Migrate at each start

A task on the lane's start runs after setup and before the services, so the API starts against a migrated database. [Run installs, migrations and seeds](../services/tasks.md) covers the task and a reset of the database; [Start order and readiness](../../concepts/start-order.md) has the whole order.

```yaml devlanes
tasks:
  migrate:
    run_script:
      file: package.json
      name: db:migrate
    when: lane.start
```

## If you use MySQL or MariaDB

The shape is the same, with the commands of the MySQL client and a healthcheck such as `mysqladmin ping` in the Compose file.

```yaml devlanes
infra:
  setup:
    database: docker compose exec -T mysql mysql -uroot -proot -e "CREATE DATABASE IF NOT EXISTS ${{ lane.db_name }}"
  teardown:
    database: docker compose exec -T mysql mysql -uroot -proot -e "DROP DATABASE IF EXISTS ${{ lane.db_name }}"
global_env:
  DATABASE_URL: mysql://root:root@127.0.0.1:${{ infra.mysql.port }}/${{ lane.db_name }}
```

## If you want the server stopped

Stopping a lane leaves the container running for the other lanes. `devlanes infra down` stops it and keeps its volume, and the next start brings it back. Cleaning the last lane of the project stops it too.

## If you pulled a newer image

A container that runs healthy with the same Compose config is left alone, so a pull does not reach it. Remove the container, then start a lane again; the named volume keeps the data.

```console
$ docker compose -p myapp rm -sf postgres
```

## If each lane needs its own server

A version pinned per branch or a migration that would break the other lanes calls for a container per lane: [Give each lane its own container](container-per-lane.md).

## Related

- [A database for each lane](../../learn/databases.md)
- [Keep Redis, RabbitMQ, S3 and mail apart per lane](datastores.md)
- [Remove lanes](../lanes/clean-up.md)
