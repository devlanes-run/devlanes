---
title: A database for each lane
description: One Postgres container serves every lane of myapp, and each lane gets a database of its own in it.
kind: learn
---

The API of myapp is about to keep data in Postgres. Both lanes need a database, and a migration tried on the feature branch must leave the data of the main branch alone. One Postgres container serves both lanes, with a database for each lane inside it.

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

The port mapping leaves the host port to Docker, and devlanes reads back the one Docker chose. The container is the project's, shared by its lanes, so it keeps running while lanes stop, even when every lane is stopped. It stops when the project's last lane is cleaned, and its volume keeps the data.

## The project file gives each lane a database

Under `infra` the file names the Compose file and fixes the Compose project's name, so `docker compose -p myapp` reaches the same container from any terminal. Each lane computes a database name from its own name, and a global variable gives every service and task of the lane a database URL built from that name and the port Docker chose.

```yaml devlanes
# devlanes.yaml
project: myapp

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
  install:
    run: [npm, install]
    when: lane.create
  migrate:
    run: docker compose exec -T postgres psql -U postgres -d ${{ lane.db_name }} -v ON_ERROR_STOP=1 -c "CREATE TABLE IF NOT EXISTS notes (id serial PRIMARY KEY, body text NOT NULL)"
    when: lane.start

services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz

  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

A value under `lanes.vars` is read as `lane.db_name`, the same way as the lane's domain, and comes out as myapp_main in lane 0 and myapp_lane1 in lane 1. devlanes runs setup, teardown and tasks with Compose's project name and file in their environment, so the Compose commands in them reach the shared container from either worktree, and psql runs inside the container. A project with a migration tool runs that tool as the task instead, and it reads the same database URL.

## Setup creates the database once, the migration runs at each start

Setup is a list of commands that belong to the containers, each preparing what a lane needs inside them, here its database. An entry runs once per lane and again after it changes, and its teardown entry undoes it when the lane is cleaned. A `lane.create` task such as the install prepares the worktree, and runs at each start that brings the lane up until it has succeeded once. A `lane.start` task such as the migration runs at each start that brings the lane up, so it is written to skip what already exists.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ infra
✓ setup
✓ migrate
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

Both lanes have been stopped since the web app arrived, so this start brings lane 0 up with the new file. Containers come first, then setup, then tasks, then services, so the API starts against a database that exists and is migrated. After a stop, the next start skips setup, which already ran in lane 0 with this entry, and runs the migration again. The install runs in neither start, because it succeeded in this lane when the web app arrived.

```console title="~/dev/myapp"
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

## Each lane connects to its own database

The checkout worktree takes the new file by merging main. Its start finds the container up and runs a setup of its own, which creates myapp_lane1.

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
✓ infra
✓ setup
✓ migrate
✓ services ready
myapp@1 · feat/checkout
✓ services  2 of 2 ready
  ✓  api  https://api.myapp1.localhost
  ✓  web  https://myapp1.localhost
```

Its API gets the database of lane 1, on the port Docker chose for the container.

```console title="~/dev/myapp-checkout"
$ devlanes env show api
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:50204/myapp_lane1
DEVLANES_LANE=1
DEVLANES_LANE_NAME=lane1
```

## Cleaning a lane drops its database

A lane outlives a stop: its database and ports stay for the next start. A dry run of `devlanes lane clean` lists what a clean would do, and does none of it.

```console title="~/dev/myapp"
$ devlanes lane clean 1 --dry-run
would clean myapp@1 (feat/checkout):
  stop      myapp@1:api
  stop      myapp@1:web
  teardown  database: docker compose exec -T postgres dropdb -U postgres --if-exists --force ${{ lane.db_name }}
  delete    logs (3.3 KiB)
  delete    run and task history
  free      ports 20070-20079
next: devlanes lane clean myapp@1 --yes
```

The teardown line shows the entry as the file has it; a clean runs it for lane 1 and drops myapp_lane1, while lane 0 keeps its own database in the same container. [Remove lanes](../how-to/lanes/clean-up.md) has everything a clean deletes.

Both lanes keep running, each on its own database, so when the API breaks on the feature branch in [When a service fails](failures.md), the main branch goes on as it was.

## On your own project

- [Share one database server between lanes](../how-to/infra/shared-database.md)
- [Give each lane its own container](../how-to/infra/container-per-lane.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Start order and readiness](../concepts/start-order.md)
