---
title: Keep Redis, RabbitMQ, S3 and mail apart per lane
description: Share one Redis, RabbitMQ or S3 store between lanes with a namespace per lane, and give each lane its own mail inbox.
kind: how-to
---

When the lanes of a project share a store such as Redis or RabbitMQ, you give each lane a namespace of its own inside it, so no lane reads another lane's data.

::example{slug="infra-multi-database"}

## Name each namespace from the lane

Each store gets one lane variable. Setup creates the namespace, teardown removes it when you clean the lane, and the URL the services get points into it, all from that one name.

```yaml devlanes
lanes:
  vars:
    redis_db: "${{ lane.index }}"
    vhost: "myapp_${{ lane.name }}"
    # S3 bucket names allow no underscores.
    bucket: "myapp-${{ lane.name }}"
```

The containers stay shared, as every Compose service is unless you list it under [`infra.per_lane`](/docs/reference/config/project/infra/). Setup and teardown entries reach them with a plain `docker compose`, as in [Share one database server between lanes](shared-database.md).

## Create the namespace and pass its URL

::::tabs
:::tab[Redis]
Redis has numbered databases, so a lane uses the one that matches its index and needs nothing created. The clean empties it.

```yaml devlanes
infra:
  teardown:
    redis: docker compose exec -T redis redis-cli -n ${{ lane.redis_db }} FLUSHDB
global_env:
  REDIS_URL: redis://127.0.0.1:${{ infra.redis.port }}/${{ lane.redis_db }}
```
:::
:::tab[RabbitMQ]
A vhost per lane keeps exchanges and queues apart. The vhost lives inside the RabbitMQ node, so the entry names the container and runs again whenever it has restarted.

```yaml devlanes
infra:
  setup:
    rabbitmq:
      # After a restart the node can still be booting when the entry runs.
      run: |
        until docker compose exec -T rabbitmq rabbitmq-diagnostics -q check_running >/dev/null 2>&1; do sleep 2; done &&
        docker compose exec -T rabbitmq rabbitmqctl add_vhost ${{ lane.vhost }} &&
        docker compose exec -T rabbitmq rabbitmqctl set_permissions -p ${{ lane.vhost }} app '.*' '.*' '.*'
      container: rabbitmq
  teardown:
    rabbitmq:
      run: docker compose exec -T rabbitmq rabbitmqctl delete_vhost ${{ lane.vhost }}
      ignore_errors: true
global_env:
  AMQP_URL: amqp://app:app@127.0.0.1:${{ infra.rabbitmq.ports['5672'] }}/${{ lane.vhost }}
```

The container publishes the broker and the management UI, so the URL picks the broker by its container port.
:::
:::tab[S3]
A bucket per lane. The example creates it with the RustFS command line, run from a Compose service of its own so your machine needs no S3 tool; its profile keeps it out of the containers devlanes starts.

```yaml
# docker-compose.yml
services:
  rc:
    image: rustfs/rc:v0.1.37
    profiles: [tools]
    environment:
      RC_HOST_local: http://rustfsadmin:rustfsadmin@rustfs:9000
```

```yaml devlanes
infra:
  setup:
    s3: docker compose run --rm rc bucket create --ignore-existing --format human local/${{ lane.bucket }}
  teardown:
    s3:
      run: docker compose run --rm rc bucket remove --force --format human local/${{ lane.bucket }}
      ignore_errors: true
global_env:
  S3_URL: http://rustfsadmin:rustfsadmin@127.0.0.1:${{ infra.rustfs.ports['9000'] }}/${{ lane.bucket }}
```
:::
:::tab[Mail]
A mail catcher has nothing to split, so each lane runs its own and gets its own inbox. [Give each lane its own container](container-per-lane.md) covers per-lane services.

```yaml devlanes
infra:
  per_lane: [mailpit]
global_env:
  SMTP_URL: smtp://127.0.0.1:${{ infra.mailpit.ports['1025'] }}
```
:::
::::

A failed teardown entry does not stop the clean. `ignore_errors` counts it as done, for a namespace that may already be gone.

## Open a store's web UI on a hostname

A hostname under the lane's domain gives the browser each lane's own console. Without a port, the hostname routes to the container's first published port.

```yaml devlanes
infra:
  hosts:
    rabbitmq: { host: "rabbitmq.${{ lane.domain }}", port: "15672" }
    rustfs: { host: "rustfs.${{ lane.domain }}", port: "9001" }
    mailpit: "mail.${{ lane.domain }}"
```

For presigned S3 URLs that the browser opens, route a hostname to the S3 port instead of the console and build the URLs on it.

## If you run more than 16 lanes on Redis

Redis has 16 databases unless its config raises `databases`. Raise it, or list Redis under `per_lane`.

## If a library cannot select a Redis database

Some queue libraries handle Redis database numbers poorly. Build a key prefix from the lane name instead, and have your code pass it to the library:

```yaml devlanes
global_env:
  BULL_PREFIX: myapp_${{ lane.name }}
```

## Related

- [Share one database server between lanes](shared-database.md)
- [Give each lane its own container](container-per-lane.md)
- [Run installs, migrations and seeds](../services/tasks.md)
