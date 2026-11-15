---
title: Choose what a box shares
description: Keep a service or container on the box, and make a server devlanes does not run reachable from the machines that connect.
kind: how-to
---

A connected machine mirrors each service and container that the box's lanes run, with its ports and hostnames. You can keep some of them on the box, and add a server you run there yourself. Environments, secrets and logs stay on the box whatever you share, and [Remote mirroring](../../concepts/remote-mirroring.md) lists what crosses.

::example{slug="remote-docker-box"}

All of this goes in the project file on the box, or in the local file next to it when only your box needs it.

## Keep a service on the box

Mark the service as hidden. The machines that connect never learn its name, ports or hostnames, the box refuses connections to its ports from them, and its synced folders stay on the box.

```yaml devlanes
# devlanes.local.yaml on the box
services:
  admin:
    hidden: true
```

On the box itself the service runs as before, and the box's proxy routes its hostname. Put [`services.<name>.hidden`](/docs/reference/config/project/services/) in the project file instead when the whole team keeps that service on their boxes.

A service that already runs keeps what it started with, and a reload leaves it alone, since the flag does not change how it runs. Restart it on the box to apply the change.

```console title="~/dev/myapp on the box"
$ devlanes restart admin
```

## Keep a container on the box

Containers are hidden by name, in one list for the project. The names are those of [`infra.hidden`](/docs/reference/config/project/infra/): Compose services or plain containers.

```yaml devlanes
infra:
  hidden: [postgres]
```

A tool on your machine that used the container's port, such as a database client, then has nothing to reach, while services on the box keep using it.

## Mirror a server devlanes does not run

A server you start on the box yourself, such as a file server in an SSH shell, is not reachable from your machine until the box's config declares it. Declare its address and the hostname it gets.

```yaml devlanes
# devlanes.local.yaml on the box
infra:
  external:
    files:
      address: 127.0.0.1:8000
      hosts: "files.${{ lane.domain }}"
```

Your machine mirrors the port and routes the hostname while a lane of the project on the box runs a service, a task or a command, or has its containers up. The box connects to exactly the declared address, so the server has to listen there. Pick a port outside the box's port range, since a port that devlanes or a lane already uses is refused. [Give a server you run yourself a hostname](../services/external.md) covers the address.

## Keep a declared server on the box

An entry can be hidden on its own with [`infra.external.<name>.hidden`](/docs/reference/config/project/infra/). The box's proxy still routes its hostname on the box.

```yaml devlanes
infra:
  external:
    pgadmin:
      address: 127.0.0.1:5050
      hidden: true
```

## Related

- [Copy build output to a fixed folder](../services/sync-build-output.md)
- [Keep personal changes out of Git](../projects/local-overrides.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
- [Security](../../concepts/security.md)
