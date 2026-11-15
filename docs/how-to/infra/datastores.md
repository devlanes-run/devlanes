---
title: Keep Redis, RabbitMQ, S3 and mail apart per lane
description: Share one Redis, RabbitMQ or S3 store between lanes with a namespace per lane, and give each lane its own mail inbox.
kind: how-to
---

When the lanes of a project share a store such as Redis or RabbitMQ, you give each lane a namespace of its own inside it, so the code of one lane does not see another lane's keys, queues or files.

::example{slug="infra-multi-database"}

## Name each namespace from the lane

Each store gets one lane variable. Setup creates the namespace, teardown removes it when you clean the lane, and the URL the services get points into it, all from that one name. Redis databases are numbered, so its value is the lane number.

```yaml devlanes
lanes:
  vars:
    redis_db: "${{ lane.index }}"
    vhost: "myapp_${{ lane.name }}"
    # S3 bucket names allow no underscores.
    bucket: "myapp-${{ lane.name }}"
```

The stores stay shared, as every Compose service is unless you list it under [`infra.per_lane`](/docs/reference/config/project/infra/). Setup and teardown entries reach them with a plain `docker compose`, as in [Share one database server between lanes](shared-database.md).

## Create the namespace and pass its URL

::::tabs
:::tab[Redis]
A lane uses the database that matches its number and needs nothing created. The clean empties it.

```yaml
# docker-compose.yml
services:
  redis:
    image: redis:8.10.3-alpine
    ports: ["127.0.0.1::6379"]
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 2s
      retries: 30
```

```yaml devlanes
infra:
  compose: docker-compose.yml
  teardown:
    redis: docker compose exec -T redis redis-cli -n ${{ lane.redis_db }} FLUSHDB
global_env:
  REDIS_URL: redis://127.0.0.1:${{ infra.redis.port }}/${{ lane.redis_db }}
```

A lane cleaned without its teardown, such as one whose worktree you deleted first, leaves its keys to the next lane with the same number.
:::
:::tab[RabbitMQ]
A vhost per lane keeps exchanges and queues apart. RabbitMQ keeps its data under a node name taken from the container's hostname, so a fixed hostname lets a recreated container find its vhosts again. The healthcheck waits until RabbitMQ itself runs, which is what the setup entry needs.

```yaml
# docker-compose.yml
services:
  rabbitmq:
    image: rabbitmq:4.3.7-management-alpine
    hostname: rabbitmq
    environment:
      RABBITMQ_DEFAULT_USER: app
      RABBITMQ_DEFAULT_PASS: app
    ports: ["127.0.0.1::5672", "127.0.0.1::15672"]
    volumes: [rabbitdata:/var/lib/rabbitmq]
    healthcheck:
      test: ["CMD", "rabbitmq-diagnostics", "-q", "check_running"]
      interval: 5s
      retries: 30

volumes:
  rabbitdata:
```

The setup entry names the container, so it runs again whenever the container has restarted, in case the vhost is gone.

```yaml devlanes
infra:
  compose: docker-compose.yml
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

The container publishes the broker and the management UI, so the URL picks the broker by its container port. Every lane connects as the same user `app`, which has rights on every lane's vhost.
:::
:::tab[S3]
A bucket per lane, in RustFS here. Its command line runs from a second Compose service, so your machine needs no S3 tool. The profile keeps that service out of the containers devlanes starts; Compose still runs it when a command names it. Leave `tools` out of [`infra.profiles`](/docs/reference/config/project/infra/), or the helper starts with every lane.

```yaml
# docker-compose.yml
services:
  rustfs:
    image: rustfs/rustfs:1.0.4
    environment:
      RUSTFS_ACCESS_KEY: rustfsadmin
      RUSTFS_SECRET_KEY: rustfsadmin
      RUSTFS_CONSOLE_ENABLE: "true"
    ports: ["127.0.0.1::9000", "127.0.0.1::9001"]
    volumes: [s3data:/data]
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://127.0.0.1:9000/health"]
      interval: 2s
      retries: 30

  rc:
    image: rustfs/rc:v0.1.37
    profiles: [tools]
    environment:
      RC_HOST_local: http://rustfsadmin:rustfsadmin@rustfs:9000

volumes:
  s3data:
```

Port 9000 is the S3 API and 9001 the web console.

```yaml devlanes
infra:
  compose: docker-compose.yml
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
A mail catcher has nothing to split, so each lane runs its own Mailpit and gets its own inbox. [Give each lane its own container](container-per-lane.md) covers services listed as per lane.

```yaml
# docker-compose.yml
services:
  mailpit:
    image: axllent/mailpit:v1.33.0
    ports: ["127.0.0.1::8025", "127.0.0.1::1025"]
    healthcheck:
      test: ["CMD", "/mailpit", "readyz"]
      interval: 2s
      retries: 30
```

Port 1025 takes the mail and 8025 serves the inbox.

```yaml devlanes
infra:
  compose: docker-compose.yml
  per_lane: [mailpit]
global_env:
  SMTP_URL: smtp://127.0.0.1:${{ infra.mailpit.ports['1025'] }}
```
:::
::::

A failed teardown entry does not stop the clean either way; `ignore_errors` marks the failure as expected in the clean's report.

## Open a store's web UI on a hostname

A hostname under each lane's domain opens a store's console in the browser. A shared store shows the same console, with every lane's namespace, under each lane's hostname, while the per-lane Mailpit shows the lane's own inbox. Each route names the container port it opens.

```yaml devlanes
infra:
  hosts:
    rabbitmq: { host: "rabbitmq.${{ lane.domain }}", port: "15672" }
    rustfs:
      - { host: "rustfs.${{ lane.domain }}", port: "9001" }
      - { host: "s3.${{ lane.domain }}", port: "9000" }
    mailpit: { host: "mail.${{ lane.domain }}", port: "8025" }
```

The second RustFS route serves the S3 API. Build the presigned URLs that the browser opens on that hostname.

## If you run more than 16 lanes on Redis

Redis has 16 databases unless its config raises `databases`. Raise it, or list Redis under `per_lane`.

## If your Redis is a cluster

A Redis Cluster supports database 0 alone. Build a key prefix from the lane name instead, and have your code pass it to its Redis client:

```yaml devlanes
global_env:
  REDIS_KEY_PREFIX: myapp_${{ lane.name }}
```

## Related

- [Share one database server between lanes](shared-database.md)
- [Give each lane its own container](container-per-lane.md)
- [Remove lanes](../lanes/clean-up.md)
