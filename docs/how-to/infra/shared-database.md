---
title: Share one database server between lanes
description: Run one Postgres or MySQL container for every lane of a project and give each lane its own database in it.
kind: how-to
---

When every lane of a project needs a database and one server is enough, you run that server once and give each lane a database of its own in it, created when the lane is first started and dropped when you clean the lane from its worktree.

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

devlanes runs the shared Compose project from the Compose files of the primary worktree, with the `.env` next to its project file, so an edit to the Compose file on another branch does not reach the shared container. The settings under `infra` in the project file come from the lane being started, though. A branch that changes them changes the shared container when its lane comes up, so make such changes on the main branch.

## Name a database for each lane

A lane variable computes the name once, and the commands and the URL below all read it, so the database a lane creates is the one its clean drops.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}
```

Lane 0 gets `myapp_main` and lane 1 `myapp_lane1`.

## Create it in setup and drop it in teardown

Setup entries run after the containers are healthy, and devlanes points their `docker compose` calls at the shared project. Teardown entries run when you clean the lane.

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

The teardown is skipped when the lane's worktree is gone, or when nothing has started the lane or run a command in it since the daemon last started, as after a reboot. The database then stays, and the next lane with the same number uses it as it is, because setup only creates what is missing. [Remove lanes](../lanes/clean-up.md) has the rest.

## Run setup again by hand

A setup entry that succeeded is not run again until you change it in the project file. To run it once more, name it:

```console
$ devlanes run infra.setup.database
```

The entry checks before it creates, so running it again is safe.

## Give the services the connection URL

The URL joins the port Docker gave the container and the lane's database name.

```yaml devlanes
global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}
```

To see the URL a service gets, print its variables in the lane's worktree. In lane 1 the URL ends in that lane's database, on the port Docker gave the shared container:

```console title="~/dev/myapp-checkout"
$ devlanes env show api
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:50204/myapp_lane1
DEVLANES_LANE=1
DEVLANES_LANE_NAME=lane1
```

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

The shape is the same. The Compose service publishes the MySQL port and sets the root password the commands use:

```yaml
# docker-compose.yml
services:
  mysql:
    image: mysql:8
    environment:
      MYSQL_ROOT_PASSWORD: root
    ports: ["127.0.0.1::3306"]
    volumes: [mysqldata:/var/lib/mysql]

volumes:
  mysqldata:
```

The image accepts no connections until its first-start initialisation is done, so add a healthcheck that passes only once the server accepts them. The commands go through the MySQL client inside the container:

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

Stopping lanes leaves the container running, even when none of them runs. This stops it and keeps its volume:

```console
$ devlanes infra down
```

The next start of a stopped lane brings it back. While lanes run, bring it back yourself:

```console
$ devlanes infra up
```

Cleaning the last lane of the project stops the container too.

## If you pulled a newer image

A container that runs healthy with the same Compose config is left alone, so a pull does not reach it. Remove the container, then bring it back up; the named volume keeps the data.

```console
$ docker compose -p myapp rm -sf postgres
```

```console
$ devlanes infra up
```

A start into a lane that already runs does not bring the container back, so the lanes have no database until `devlanes infra up` runs.

## If each lane needs its own server

A version pinned per branch or a migration that would break the other lanes calls for a container per lane: [Give each lane its own container](container-per-lane.md).

## Related

- [A database for each lane](../../learn/databases.md)
- [Keep Redis, RabbitMQ, S3 and mail apart per lane](datastores.md)
- [Remove lanes](../lanes/clean-up.md)
