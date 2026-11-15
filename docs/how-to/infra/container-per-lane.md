---
title: Give each lane its own container
description: Run a Compose service once per lane, with its own port and volume, and remove it with the lane.
kind: how-to
---

When a branch needs its own version of a database, or runs migrations that would break the other lanes, you give each lane a container of its own instead of a namespace in a shared one.

::example{slug="infra-postgres-per-lane"}

## Write the service in the Compose file

The image creates the database `POSTGRES_DB` names on its first start, and the named volume holds the data. Each lane gets a volume of its own under that name.

```yaml
# docker-compose.yml
services:
  postgres:
    image: postgres:17.11-alpine
    environment:
      POSTGRES_PASSWORD: postgres
      POSTGRES_DB: myapp
    ports: ["127.0.0.1::5432"]
    volumes: [pgdata:/var/lib/postgresql/data]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -h 127.0.0.1 -U postgres -d myapp"]
      interval: 2s
      retries: 30

volumes:
  pgdata:
```

## List the service as per lane

Every Compose service is shared by the lanes of the project until you name it under [`infra.per_lane`](/docs/reference/config/project/infra/). A service listed there runs once for each lane, lane 0 included, in a Compose project named after the project and the lane number, such as `myapp-lane1`.

```yaml devlanes
infra:
  compose: docker-compose.yml
  per_lane: [postgres]
```

The lane's container runs from that lane's worktree. devlanes publishes its ports from the lane's block of ports, whatever host ports the Compose file declares, and the lane's Compose project has its own named volumes. This needs Compose 2.24.4 or newer.

## Point the URL at the lane's port

The expression is the same as for a shared container, and it gives a different port in each lane. The database name stays fixed, since each lane has the server to itself.

```yaml devlanes
global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/myapp
```

## Prepare its data in setup

In a setup entry, a plain `docker compose` reaches the shared project, where this service does not run. The lane's own project is named in a variable devlanes sets for setup entries and tasks, and not for services.

```yaml devlanes
infra:
  setup:
    extensions: docker compose -p "$DEVLANES_COMPOSE_LANE_PROJECT" exec -T postgres psql -U postgres -d myapp -c "CREATE EXTENSION IF NOT EXISTS pgcrypto"
```

A new lane runs the entry on its first start, against its new container. Removing the container by hand keeps its volume and what the entry created in it, so the entry does not need to run again.

## Stop, freeze and clean the lane

A stop leaves the lane's container running with its data, and it keeps using memory until you clean the lane or stop the container with Docker. When the lane is frozen, devlanes pauses its own containers and unpauses them when it thaws; shared containers keep running. [Throttling and freezing](../../concepts/freezing.md) says what thaws a lane.

Cleaning the lane removes its Compose project together with its volumes, so the lane's database goes with it. Add `--dry-run` to see what it would remove:

```console
$ devlanes lane clean 1 --dry-run
```

[Remove lanes](../lanes/clean-up.md) covers the clean itself and what else it deletes.

## If a shared service depends on it

A shared service whose `depends_on` names a per-lane service is refused, with both names in the message. Remove the dependency, or list both services as per lane.

The other way round is accepted. The lane's container may name a shared service in `depends_on`, and devlanes starts it without starting that service a second time. The lane's container runs in its own Compose project, though, so it cannot reach the shared service by its name.

## If the volume has a fixed name

A volume with its own `name:` or `external: true` in the Compose file is one volume for the whole machine, so every lane mounts the same data. Drop the fixed name to get a volume per lane.

## If the container is not in a Compose file

A container declared in the project file runs once per lane with [`infra.containers.<name>.per_lane`](/docs/reference/config/project/infra/). [Run containers without Compose](plain-containers.md) shows it.

## If you pulled a newer image

A running healthy container keeps its old image after a pull, and a start into a lane that already runs does not create it again. Remove the lane's container by its Compose project:

```console
$ docker compose -p myapp-lane1 rm -sf postgres
```

Then, in the lane's worktree, bring its containers up again, or stop and start the lane:

```console
$ devlanes infra up
```

## Related

- [Share one database server between lanes](shared-database.md)
- [Remove lanes](../lanes/clean-up.md)
- [Throttling and freezing](../../concepts/freezing.md)
