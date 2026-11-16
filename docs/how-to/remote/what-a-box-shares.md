---
title: Choose what a box shares
description: Keep a service or container on the box, and make a server devlanes does not run reachable from the machines that connect.
kind: how-to
---

A connected machine mirrors each service and container that the box's lanes run, and you can keep some of them on the box or add a server you run there yourself. Environments, secrets and logs stay on the box either way, as [Remote mirroring](../../concepts/remote-mirroring.md) explains.

::example{slug="remote-docker-box"}

Put these lines in the project file on the box, or in the local file next to it when only your box needs them.

## Keep a service on the box

Mark the service as hidden with [`services.<name>.hidden`](/docs/reference/config/project/services/#services-name-hidden). Machines that connect never see it, and it runs on the box as before.

```yaml devlanes
# devlanes.local.yaml on the box
services:
  admin:
    hidden: true
```

A running service keeps the setting it started with, so restart it on the box.

```console title="~/dev/myapp on the box"
$ devlanes restart admin
```

## Keep a container on the box

List the containers by name under [`infra.hidden`](/docs/reference/config/project/infra/#infra-hidden).

```yaml devlanes
infra:
  hidden: [postgres]
```

## Mirror a server devlanes does not run

A server you start on the box yourself is reachable from your machine once the box's config declares its address and hostname. Your machine mirrors it while a lane of the project runs on the box. Pick a port outside the box's port range.

```yaml devlanes
# devlanes.local.yaml on the box
infra:
  external:
    files:
      address: 127.0.0.1:8000
      hosts: "files.${{ lane.domain }}"
```

[Give a server you run yourself a hostname](../services/external.md) covers the entry. To declare one and still keep it on the box, set [`infra.external.<name>.hidden`](/docs/reference/config/project/infra/#infra-external-name-hidden).

## Related

- [Copy build output to a fixed folder](../services/sync-build-output.md)
- [Keep personal changes out of Git](../projects/local-overrides.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
- [Security](../../concepts/security.md)
