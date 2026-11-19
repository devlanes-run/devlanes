---
title: Run containers without Compose
description: Declare a container in the project file, read its port, wait for it and give it a hostname, shared or once per lane.
kind: how-to
---

Use this when a project needs a container or two, such as a mock login provider, and has no Compose file: you declare them in the project file and devlanes runs them through Docker.

::example{slug="infra-plain-containers"}

## Declare the container

Name the container ports you need; devlanes publishes each on a host port on 127.0.0.1. A container is shared by every lane unless you set `per_lane`. The ready check makes the start wait until the provider answers, the hostname lets a browser open it, and the variable gives the services its port.

```yaml devlanes
infra:
  containers:
    oidc:
      image: ghcr.io/navikt/mock-oauth2-server:6.1.1
      ports:
        http: 8080
      env:
        JSON_CONFIG_PATH: /config/oidc.json
      volumes: ["./infra/oidc.json:/config/oidc.json:ro"]
      per_lane: true
      ready:
        http:
          path: /isalive
  hosts:
    oidc: "auth.${{ lane.domain }}"

global_env:
  OIDC_PORT: ${{ infra.oidc.ports.http }}
```

A relative volume path starts at the folder of the project file. The ready check's fields are in [`infra.containers.<name>.ready`](/docs/reference/config/project/infra/#infra-containers-name-ready).

## Start the lane

The start creates the container before setup, tasks and services, and waits until it is ready.

```console
$ devlanes start
```

## If you change the container

A start into a running lane brings back containers that stopped and leaves running ones as they are. After you change a container's image, command, variables, mounts or ports, recreate it in that lane:

```console
$ devlanes infra up
```

A shared container follows the project file of the lane that last brought it up, so change it on the main branch.

## If containers need to find each other by name

Plain containers cannot look each other up by name. Put such containers in a Compose file, as in [Share one database server between lanes](shared-database.md).

## Related

- [Give each lane its own container](container-per-lane.md)
- [Give a server you run yourself a hostname](../services/external.md)
- [Keep Redis, RabbitMQ, S3 and mail apart per lane](datastores.md)
