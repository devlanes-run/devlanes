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

The port mapping leaves the host port to Docker, and devlanes reads back the one Docker chose. The lanes share the container, so it keeps running while you stop them, and it stops when you clean the project's last lane, with its volume keeping the data.

## The project file gives each lane a database

The marked lines are new: lane values, the infra block with setup and teardown, global variables and the migration task. Under `infra` the file names the Compose file and fixes the Compose project's name, so `docker compose -p myapp` reaches the same container from any terminal.

```yaml devlanes ins={4-6,8-18,20-21,27-29}
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

Each lane computes `lane.db_name` from its own name, myapp_main in lane 0 and myapp_lane1 in lane 1, and `global_env` gives every service and task of the lane a database URL built from that name and the port Docker chose. Setup, teardown and tasks get Compose's project name and file in their environment, so their Compose commands reach the shared container from either worktree.

## Setup creates the database once, the migration runs at each start

Put a step in setup when it prepares data inside a container and a clean should undo it, such as a database, and make it a `lane.create` task when it prepares the worktree itself, such as its packages.

| Kind | For | Runs | Runs again | Undone by | In myapp |
|---|---|---|---|---|---|
| Setup entry | Data inside the containers | At the lane's first start, after the containers | When its entry changes | Its teardown, at a clean | `database` |
| `lane.create` task | The worktree | At each start of a stopped lane until it succeeds | When its entry changes | Nothing | `install` |
| `lane.start` task | Work due at each start | At each start of a stopped lane | At every such start | Nothing | `migrate` |

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

Both lanes are stopped, so lane 0 starts with the new file. Containers come first, then setup, then tasks, then services, so the API starts against a database that exists and is migrated. After a stop, the next start skips setup, which already ran in lane 0 with this entry, and the `!` marks that skipped step; the migration runs again. The install, which succeeded in this lane when the web app arrived, runs in neither.

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

You merge main into the checkout worktree and start lane 1, whose setup creates myapp_lane1 in the running container.

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

These are some of the variables the API of lane 1 gets: its database URL, on the port Docker chose, and its lane.

```console title="~/dev/myapp-checkout"
$ devlanes env show api
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:50204/myapp_lane1
DEVLANES_LANE=1
DEVLANES_LANE_NAME=lane1
```

## Cleaning a lane drops its database

A stopped lane keeps its database. `devlanes lane clean` removes a lane with its database, logs and ports, and with `--dry-run` only lists what it would do, after a query that shows one database per lane.

```console title="~/dev/myapp"
$ docker compose -p myapp exec -T postgres psql -U postgres -Atc "SELECT datname FROM pg_database WHERE datname LIKE 'myapp_%' ORDER BY datname"
myapp_lane1
myapp_main
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

The teardown line shows the entry as the file has it; a clean runs it for lane 1 and drops myapp_lane1, and myapp_main stays in the same container for lane 0. [Remove lanes](../how-to/lanes/clean-up.md) has everything a clean deletes.

Both lanes keep running, each on its own database, so when the API breaks on the feature branch in [When a service fails](failures.md), the main branch goes on as it was.

## On your own project

- [Share one database server between lanes](../how-to/infra/shared-database.md)
- [Give each lane its own container](../how-to/infra/container-per-lane.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Start order and readiness](../concepts/start-order.md)
