---
title: Run containers without Compose
description: Declare a container in the project file, read its port, wait for it and give it a hostname, shared or once per lane.
kind: how-to
---

When a project needs a container or two, such as a mock login provider, and has no Compose file, you declare them in the project file and devlanes runs them through Docker.

::example{slug="infra-plain-containers"}

## Declare the container

Name the container ports you need. devlanes publishes each one on a host port on 127.0.0.1 and picks the host port itself. A relative path in a volume starts at the folder of the project file.

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
```

When the lane starts, devlanes creates the container before setup, tasks and services ([Start order and readiness](../../concepts/start-order.md)). A start into a lane that already runs creates nothing, so in a running lane bring the new container up yourself:

```console
$ devlanes infra up
```

## Run one per lane

A container is shared by every lane of the project unless you mark it per lane. A shared one runs once and every lane reads the same port; a per-lane one gets a port from each lane's block of ports. Here the login provider runs per lane, next to a small shared web server:

```yaml devlanes
infra:
  containers:
    oidc:
      per_lane: true
    whoami:
      image: traefik/whoami:v1.11.0
      ports:
        http: 80
```

The containers of a lane are listed from its worktree. Lane 0 has its own login provider and the shared server:

```console title="~/dev/myapp"
$ devlanes infra status
infra for myapp@0
  containers:
    name: devlanes-myapp-whoami
    scope: shared
    ports:
      80: 20082

    name: devlanes-myapp-lane0-oidc
    scope:
      lane: 0
    ports:
      8080: 20071
```

Lane 1 has a login provider of its own on another port, and the same shared server on the same port:

```console title="~/dev/myapp-checkout"
$ devlanes infra status
infra for myapp@1
  containers:
    name: devlanes-myapp-lane1-oidc
    scope:
      lane: 1
    ports:
      8080: 20091

    name: devlanes-myapp-whoami
    scope: shared
    ports:
      80: 20082
```

A per-lane container runs from the lane's worktree, so its relative volume paths do too. A shared one reads them from the primary worktree.

## Pass its port to the services

Services run on your machine, outside Docker, so they reach the container through its host port, which differs between lanes for a per-lane container.

```yaml devlanes
global_env:
  OIDC_PORT: ${{ infra.oidc.ports.http }}
```

## Wait until it is ready

devlanes waits for the image's healthcheck, or for its first port to accept a connection when the image has none. A container that answers its port before it serves requests needs a ready check of its own.

```yaml devlanes
infra:
  containers:
    oidc:
      ready:
        http:
          path: /isalive
```

A check that does not pass in time fails the start. The fields are in [`infra.containers.<name>.ready`](/docs/reference/config/project/infra/#infra-containers-name-ready).

## Give it a hostname

A browser that has to open the container, as a login redirect does, needs it on a hostname under the lane's domain. The hostname routes to the container's first port.

```yaml devlanes
infra:
  hosts:
    oidc: "auth.${{ lane.domain }}"
```

## If you change the container

A running container with the same image, command, variables, mounts and ports is kept as it is. After you change any of these, the next start of a stopped lane replaces it. A start into a lane that runs leaves it as it was, so in a running lane replace it yourself:

```console title="~/dev/myapp"
$ devlanes infra up
infra up for myapp@0
  SERVICE  PORT   PORTS
  oidc     20071  http->20071
  whoami   20082  http->20082
```

This run replaced lane 0's login provider after a new variable in its `env`, and left the shared server alone. The output lists every container with its port and does not say which one it replaced. The replaced container keeps its port, so the services need no reload.

A shared container follows the project file of the lane that last brought it up. A branch that changes its image replaces it for every lane when that branch's lane comes up, and a lane of another branch puts the old one back the next time it comes up. Change a shared container on the main branch.

## If it keeps data

A bare name as the source of a volume makes a named volume, kept per project for a shared container and per lane for a per-lane one. Cleaning a lane removes its per-lane containers and their volumes. Shared containers stop when you clean the project's last lane, and their volumes stay. [Remove lanes](../lanes/clean-up.md) covers the clean.

## If containers need to find each other

Plain containers join Docker's default network, where containers cannot look each other up by name. Containers that talk to each other by name belong in a Compose file, as in [Share one database server between lanes](shared-database.md).

## Related

- [Give each lane its own container](container-per-lane.md)
- [Give a server you run yourself a hostname](../services/external.md)
- [Keep Redis, RabbitMQ, S3 and mail apart per lane](datastores.md)
