---
title: Give each lane its own container
description: Run a Compose service once per lane, with its own port and volume, and remove it with the lane.
kind: how-to
---

Use this when a branch needs its own version of a database, or runs migrations that would break the other lanes: each lane gets a container and a volume of its own.

::example{slug="infra-postgres-per-lane"}

## Write the service in the Compose file

The image creates the database `POSTGRES_DB` names, and each lane has a `pgdata` volume of its own.

```yaml title="docker-compose.yml"
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

## List it as per lane

A service under [`infra.per_lane`](/docs/reference/config/project/infra/#infra-per_lane) runs once for each lane, on a port from that lane's block of ports. The URL reads that port, so it differs in each lane. This needs Compose 2.24.4 or newer.

```yaml devlanes
infra:
  compose: docker-compose.yml
  per_lane: [postgres]

global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/myapp
```

A plain `docker compose` in a setup entry reaches the shared containers. To run a command in the lane's own container, name its Compose project with `-p "$DEVLANES_COMPOSE_LANE_PROJECT"`.

## Start and clean the lane

Each start brings up the lane's container. Stopping the lane leaves the container running; cleaning the lane removes the container and its volume, so its data is gone.

```console
$ devlanes start
$ devlanes lane clean 1 --yes
```

## If you replace the container

A start into a running lane does not recreate a container you removed, for example to use a newly pulled image. Bring it back in the lane's worktree:

```console
$ devlanes infra up
```

## If the container is not in a Compose file

Set [`infra.containers.<name>.per_lane`](/docs/reference/config/project/infra/#infra-containers-name-per_lane), as in [Run containers without Compose](plain-containers.md).

## Related

- [Share one database server between lanes](shared-database.md)
- [Remove lanes](../lanes/clean-up.md)
- [Throttling and freezing](../../concepts/freezing.md)
