---
title: Keep Redis, RabbitMQ, S3 and mail apart per lane
description: Share one Redis, RabbitMQ or S3 store between lanes with a namespace per lane, and give each lane its own mail inbox.
kind: how-to
---

Use this when the lanes of a project share a store such as Redis or RabbitMQ: each lane gets a namespace of its own inside it, so one lane's code does not see another lane's keys, queues or files.

::example{slug="infra-multi-database"}

## Name a namespace per lane

One variable per store under [`lanes.vars`](/docs/reference/config/project/lanes/#lanes-vars). Setup creates the namespace, teardown removes it when you clean the lane, and the URL points into it. The stores are services in your Compose file, as in [Share one database server between lanes](shared-database.md).

```yaml devlanes
lanes:
  vars:
    redis_db: "${{ lane.index }}"
    vhost: "myapp_${{ lane.name }}"
    # S3 bucket names allow no underscores.
    bucket: "myapp-${{ lane.name }}"
```

## Create it and pass its URL

::::tabs
:::tab[Redis]
Each lane uses the numbered database that matches its lane number.

```yaml devlanes
infra:
  teardown:
    redis: docker compose exec -T redis redis-cli -n ${{ lane.redis_db }} FLUSHDB
global_env:
  REDIS_URL: redis://127.0.0.1:${{ infra.redis.port }}/${{ lane.redis_db }}
```
:::
:::tab[RabbitMQ]
A vhost per lane. The Compose service sets the user `app` and a fixed `hostname`, so a recreated container keeps its vhosts. `container` runs the entry again after RabbitMQ restarts.

```yaml devlanes
infra:
  setup:
    rabbitmq:
      run: |
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
:::
:::tab[S3]
A bucket per lane, here in RustFS. The bucket commands run from an `rc` service in the Compose file that has `profiles: [tools]`, so it does not start with the lanes.

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
A mail catcher has nothing to split, so each lane runs its own Mailpit ([Give each lane its own container](container-per-lane.md)).

```yaml devlanes
infra:
  per_lane: [mailpit]
global_env:
  SMTP_URL: smtp://127.0.0.1:${{ infra.mailpit.ports['1025'] }}
```
:::
::::

Start each lane as usual, and its setup creates its namespaces.

```console
$ devlanes start
```

## If you want a store's web UI

Route a hostname under the lane's domain to the UI's container port. A shared store shows every lane's namespaces there.

```yaml devlanes
infra:
  hosts:
    rabbitmq: { host: "rabbitmq.${{ lane.domain }}", port: "15672" }
    mailpit: { host: "mail.${{ lane.domain }}", port: "8025" }
```

## If you run more than 16 lanes on Redis

Redis has 16 databases unless its `databases` setting raises the number.

## Related

- [Share one database server between lanes](shared-database.md)
- [Give each lane its own container](container-per-lane.md)
- [Remove lanes](../lanes/clean-up.md)
