---
title: A database for each lane
description: One Postgres container serves every lane of myapp, and each lane gets a database of its own in it.
kind: learn
---

The API of myapp starts keeping data in Postgres. Both lanes need a database, and a migration tried on the feature branch must leave the data of the main branch alone. One Postgres container serves both lanes, with a database for each lane inside it.

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

The port mapping leaves the host port to Docker, and devlanes reads back the one Docker chose. The container is the project's, shared by its lanes, so stopping a lane leaves it running, even when every lane is stopped. It stops when the project's last lane is cleaned, and its volume keeps the data.

## Each lane computes the name of its database

The project file points at the Compose file and fixes the Compose project's name, so `docker compose -p myapp` reaches the same container from any terminal. A lane can also compute values of its own, which the rest of the file reads like the lane's domain.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}

infra:
  compose: docker-compose.yml
  project_name: myapp

global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}
```

The file reads the value as `lane.db_name`, which comes out as myapp_main in lane 0 and myapp_lane1 in lane 1, after the lane names. The database URL is built from it and from the port Docker gave the container, and as a global variable it reaches every service and task of the lane.

## Setup creates the database, a task migrates it

Setup is a list of commands that prepare a lane's data. It runs once per lane, after the containers are ready, and again when its entry changes. A `lane.create` task, like the install, runs at each start that brings the lane up until it has succeeded once there. A `lane.start` task runs at every start that brings the lane up.

```yaml devlanes
infra:
  setup:
    # Postgres has no CREATE DATABASE IF NOT EXISTS; \gexec runs the CREATE only when the SELECT returns a row.
    database: |
      docker compose exec -T postgres psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
      SELECT 'CREATE DATABASE ${{ lane.db_name }}'
      WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${{ lane.db_name }}')\gexec
      SQL
  teardown:
    database: docker compose exec -T postgres dropdb -U postgres --if-exists --force ${{ lane.db_name }}

tasks:
  migrate:
    run: docker compose exec -T postgres psql -U postgres -d ${{ lane.db_name }} -v ON_ERROR_STOP=1 -c "CREATE TABLE IF NOT EXISTS notes (id serial PRIMARY KEY, body text NOT NULL)"
    when: lane.start
```

Teardown removes what setup made, when the lane is cleaned. devlanes points `docker compose` in these commands at the shared project, and psql runs inside the container, so the machine needs no Postgres client. The migration runs again at each such start, so it is written to skip what already exists.

## A start runs them in order

Containers come first, then setup, then tasks, then services, so the API starts against a database that exists and is migrated. [Start order and readiness](../concepts/start-order.md) has the details.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ infra
✓ setup
✓ migrate
✓ services ready
$ devlanes stop
stopped myapp@0:api
stopped myapp@0:web
$ devlanes start --no-ui
✓ infra
! setup skipped: already done
✓ migrate
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

The second start skips setup, since the database of lane 0 exists, and runs the migration again. The install task does not run in either start, because it succeeded in this lane when the web app arrived.

## Each lane connects to its own database

In the checkout worktree the API gets the database of lane 1, on the port Docker chose for the container.

```console title="~/dev/myapp-checkout"
$ devlanes env show api
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:50204/myapp_lane1
DEVLANES_LANE=1
DEVLANES_LANE_NAME=lane1
```

The API reads the variable like any other. To stay small, myapp takes only the database's name from it and returns that with the lane number.

```js
// api/server.mjs
const database = process.env.DATABASE_URL ? new URL(process.env.DATABASE_URL).pathname.slice(1) : null;
```

## Cleaning a lane drops its database

A lane outlives a stop: its database, ports and data folder stay for the next start. The data folder is a folder devlanes keeps for each lane in its home and passes to the lane's units as `DEVLANES_LANE_DATA`. A dry run of `devlanes lane clean` lists what a clean would do, and does none of it.

```console title="~/dev/myapp, on Windows 11"
$ devlanes lane clean 1 --dry-run
would clean myapp@1 (feat/checkout):
  stop      myapp@1:api
  stop      myapp@1:web
  teardown  database: docker compose exec -T postgres dropdb -U postgres --if-exists --force ${{ lane.db_name }}
  delete    data C:\Users\you\.devlanes\data\myapp\209 (0 B)
  delete    logs (3.3 KiB)
  delete    run and task history
  free      ports 20070-20079
next: devlanes lane clean myapp@1 --yes
```

The teardown line shows the entry as the file has it; a clean runs it for lane 1 and drops myapp_lane1, while lane 0 keeps its own database in the same container. The data folder is empty because myapp keeps nothing there. [Remove lanes](../how-to/lanes/clean-up.md) has what a clean deletes and when.

Both lanes keep running, each on its own database, so when the API breaks on the feature branch in [When a service fails](failures.md), the main branch goes on as it was.

## On your own project

- [Share one database server between lanes](../how-to/infra/shared-database.md)
- [Give each lane its own container](../how-to/infra/container-per-lane.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Start order and readiness](../concepts/start-order.md)
