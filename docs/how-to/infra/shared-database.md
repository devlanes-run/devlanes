---
title: Share one database server between lanes
description: Run one Postgres or MySQL container for every lane of a project and give each lane its own database in it.
kind: how-to
---

When every lane of a project needs a database and one server is enough, you run that server once and give each lane a database of its own in it, created when the lane is first started and dropped when you clean the lane from its worktree.

::example{slug="infra-postgres-shared"}

## Add the server to a Compose file

Leave the host port to Docker: devlanes reads back the port Docker picked and hands it to the lanes. Give the service a healthcheck, because devlanes waits for it before anything in the lane connects.

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

## Point the project file at it

A Compose service is shared by every lane of the project unless you list it under [`infra.per_lane`](/docs/reference/config/project/infra/#infra-per_lane). A fixed project name lets `docker compose -p myapp` reach the same container from a terminal outside devlanes.

```yaml devlanes
infra:
  compose: docker-compose.yml
  project_name: ${{ project.name }}
```

devlanes runs the shared Compose project from the Compose files of the primary worktree ([Glossary](../../reference/glossary.md)), with the `.env` next to its project file, so an edit to the Compose file on another branch does not reach the shared container. The settings under `infra` in the project file come from the lane being started, though. A branch that changes them changes the shared container when its lane comes up, so make such changes on the main branch.

## Name a database for each lane

A lane variable under [`lanes.vars`](/docs/reference/config/project/lanes/#lanes-vars) computes the name once, and the commands and the URL below all read it, so the database a lane creates is the one its clean drops.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}
```

Lane 0 gets `myapp_main` and lane 1 `myapp_lane1`.

## Create it in setup and drop it in teardown

Setup entries are commands that prepare a lane's data. They run after the containers are healthy, and devlanes points their `docker compose` calls at the shared project. Teardown entries remove that data when you clean the lane.

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

```console title="~/dev/myapp-checkout"
$ devlanes run infra.setup.database
exited myapp@1:infra.setup.database exit=0
```

The entry checks before it creates, so running it again is safe; here the database existed and it printed nothing.

## Give the services the connection URL

The URL joins the port Docker gave the container and the lane's database name.

```yaml devlanes
global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}
```

To see the URL a running service got, print its variables in the lane's worktree. In lane 0 the URL ends in `myapp_main`, on the port Docker gave the shared container:

```console title="~/dev/myapp"
$ devlanes env show api | grep DATABASE_URL
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:52325/myapp_main
```

[A database for each lane](../../learn/databases.md) shows the URL of lane 1 next to it.

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

```yaml title="docker-compose.yml"
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

Stopping lanes leaves the container running, even when none of them runs. `devlanes infra down` stops it and keeps its volume. While lanes run, it is refused and names them:

```console title="~/dev/myapp"
$ devlanes infra down
error: shared infra serves running lanes myapp@0, myapp@1; rerun with --yes
```

Confirm with the flag. The lanes keep running without their database until the container is back:

```console title="~/dev/myapp"
$ devlanes infra down --yes
infra down; container data is kept
  stopped myapp-postgres-1
```

The next start of a stopped lane brings the container back. While lanes run, bring it back yourself. Docker publishes it on a new port:

```console title="~/dev/myapp"
$ devlanes infra up
infra up for myapp@0
  SERVICE   PORT   PORTS
  postgres  58624  5432->58624
```

The services that were running keep the URL with the old port. The status of each lane flags it:

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  2/2       0.0%  76.9 MiB  https://myapp.localhost  (config changed: 2 changed; devlanes reload --lane myapp@0 applies it)

  UNIT      STATE             PORT   CPU   MEM       URL
  api       ready             20070  0.0%  37.3 MiB  https://api.myapp.localhost
  web       ready             20071  0.0%  39.6 MiB  https://myapp.localhost
  postgres  shared container  58624  1.2%  20.4 MiB
  api: env DATABASE_URL changed since it started
  web: env DATABASE_URL changed since it started
```

A reload gives the flagged services the new URL. Run it in the worktree of each lane that runs:

```console title="~/dev/myapp"
$ devlanes reload --no-ui
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

A container that runs once per lane does not move, because devlanes publishes it on a port from the lane's block of ports ([Give each lane its own container](container-per-lane.md)). Cleaning the last lane of the project stops the shared container too.

## If you pulled a newer image

A container that runs healthy with the same Compose config is left alone, so a pull does not reach it, and a start into a lane that already runs does not create it again. Remove the container, then bring it up; the named volume keeps the data.

```console title="~/dev/myapp"
$ docker compose -p myapp rm -sf postgres
Going to remove myapp-postgres-1
```

```console title="~/dev/myapp"
$ devlanes infra up
infra up for myapp@0
  SERVICE   PORT   PORTS
  postgres  61529  5432->61529
```

The new container has a new port as well, so reload each running lane as in [If you want the server stopped](#if-you-want-the-server-stopped).

## If each lane needs its own server

A version pinned per branch or a migration that would break the other lanes calls for a container per lane: [Give each lane its own container](container-per-lane.md).

## Related

- [A database for each lane](../../learn/databases.md)
- [Keep Redis, RabbitMQ, S3 and mail apart per lane](datastores.md)
- [Remove lanes](../lanes/clean-up.md)
