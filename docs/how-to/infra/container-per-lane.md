---
title: Give each lane its own container
description: Run a Compose service once per lane, with its own port and volume, and remove it with the lane.
kind: how-to
---

When a branch needs its own version of a database, or runs migrations that would break the other lanes, you give each lane a container of its own instead of a namespace in a shared one.

::example{slug="infra-postgres-per-lane"}

## List the service as per lane

Every Compose service is shared by the lanes of the project until you name it under [`infra.per_lane`](/docs/reference/config/project/infra/). A service listed there runs once per lane, in a Compose project of its own named after the project and the lane number, such as `myapp-lane1`.

```yaml devlanes
infra:
  compose: docker-compose.yml
  per_lane: [postgres]
```

Each lane's copy runs from that lane's worktree. devlanes publishes its ports from the lane's block of ports, whatever host ports the Compose file declares, and gives it the lane's own named volumes. This needs Compose 2.24.4 or newer, and `devlanes doctor` shows the version you have.

## Point the URL at the lane's port

The expression is the same as for a shared container, and it gives a different port in each lane. The database name can stay fixed, since each lane has the server to itself. Here it is the database the image creates from `POSTGRES_DB: myapp` in the Compose file.

```yaml devlanes
global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/myapp
```

## Prepare its data in setup

In a setup entry, a plain `docker compose` reaches the shared project, where this service does not run. The lane's own project is named in a variable devlanes sets for setup entries and tasks.

```yaml devlanes
infra:
  setup:
    extensions: docker compose -p "$DEVLANES_COMPOSE_LANE_PROJECT" exec -T postgres psql -U postgres -d myapp -c "CREATE EXTENSION IF NOT EXISTS pgcrypto"
```

Services do not get that variable. A new lane runs the entry on its first start, against its new container.

## Stop, freeze and clean the lane

A stop leaves the lane's container running with its data. When the lane is frozen, devlanes pauses its own containers and unpauses them when it thaws; shared containers keep running. [Throttling and freezing](../../concepts/freezing.md) says what thaws a lane.

`devlanes lane clean` removes the lane's Compose project together with its volumes, so the lane's database goes with it. The next start in that worktree creates a fresh container and runs setup again.

## If a shared service depends on it

A shared service whose `depends_on` names a per-lane service is refused, with both names in the message. Remove the dependency, or list both services as per lane. The other direction works: a lane's copy may depend on a shared service, and devlanes starts the copy without starting that service a second time.

## If the volume has a fixed name

A volume with its own `name:` or `external: true` in the Compose file is one volume for the whole machine, so every lane mounts the same data. Drop the fixed name to get a volume per lane.

## If the container is not in a Compose file

A container declared in the project file gets its own copy per lane with [`infra.containers.<name>.per_lane`](/docs/reference/config/project/infra/). [Run containers without Compose](plain-containers.md) shows it.

## If you pulled a newer image

A running healthy container keeps its old image after a pull. Remove the lane's copy by its Compose project and start the lane again:

```console
$ docker compose -p myapp-lane1 rm -sf postgres
```

## Related

- [Share one database server between lanes](shared-database.md)
- [Remove lanes](../lanes/clean-up.md)
- [Throttling and freezing](../../concepts/freezing.md)
