---
title: Start a service only on demand
description: Keep a service such as a docs site or a webhook tunnel stopped until someone starts it by name.
kind: how-to
---

A service nobody needs on every start, such as a component library's docs or a webhook tunnel, can stay stopped until you ask for it.

::example{slug="workflows-profiles-tags"}

## Turn off autostart

```yaml devlanes
services:
  docs:
    run_script:
      file: apps/docs/package.json
      name: dev
    hosts: "docs.${{ lane.domain }}"
    tags: [docs]
    autostart: false
```

A plain start of the lane now leaves the service out and starts the rest. Its hostname stays routed, and a request to it gets the proxy's [DL201 Service not running](../../errors/service-idle.md) page.

## Start it by name

Name the service, or one of its tags with a plus, as in `+docs`:

```console title="~/dev/myapp"
$ devlanes start docs --no-ui
```

It starts next to what the lane already runs. Until you stop it, a plain start brings it back with the rest.

## If you use profiles

A profile, a named selection of services, includes an on-demand service when it names it or its tag, and a glob such as `*` skips it. With a profile named `default`, a plain start runs exactly that profile, so use one of the two to leave a service out, not both. [Share settings and choose which services start](../projects/templates-and-profiles.md) covers profiles.

## If only some lanes need it

Autostart also takes an expression, evaluated for each lane. This one starts the service in lane 0 and leaves it on demand everywhere else:

```yaml devlanes
services:
  docs:
    autostart: ${{ lane.index == 0 }}
```

## Related

- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Run workers and replicas](workers.md)
