---
title: Start a service only on demand
description: Keep a service such as a docs site or a webhook tunnel stopped until someone starts it by name.
kind: how-to
---

A service that nobody needs on every start, such as a component library's docs or a webhook tunnel, can stay stopped until someone asks for it, while the rest of the lane starts as usual.

::example{slug="workflows-profiles-tags"}

## Mark the service

With autostart off, a plain start of the lane leaves the service out and starts the rest:

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

In a project with one other service, the API, a plain start runs only the API:

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
```

The service keeps its port and its hostname while it is stopped. A request to the hostname gets the proxy's [DL201 Service not running](../../errors/service-idle.md) page. Of the commands the page suggests, the one that names the service starts it; a start of the whole lane leaves it out again.

When you need the service, name it in a start, or name one of its tags with a plus, as in `+docs`. It starts next to what the lane already runs:

```console title="~/dev/myapp"
$ devlanes start docs --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  docs  https://docs.myapp.localhost
```

The summary counts only the services this start brought up.

After it has run, a plain start brings it back with the rest, such as after a reboot. Stopping the service, or the whole lane, leaves it out again.

## Use it with profiles

A profile, a named selection of services in the project file, picks them by name, glob or tag. A name or a tag includes an on-demand service; a glob such as `*` skips it, so a profile meant to start everything still leaves it out:

```yaml devlanes
profiles:
  all:
    include: ["*"]
```

A profile named `default` changes what a plain start of a fresh or stopped lane runs: exactly that profile's selection. Autostart then matters only for globs, so use one of the two to leave a service out, not both. [Share settings and choose which services start](../projects/templates-and-profiles.md) covers profiles.

## If only some lanes need it

Autostart also takes an expression, evaluated for each lane. This one starts the service by default in lane 0 only, and leaves it on demand in every other lane:

```yaml devlanes
services:
  docs:
    autostart: ${{ lane.index == 0 }}
```

## Related

- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Bring lanes back after a reboot](../lanes/after-a-reboot.md)
- [Run workers and replicas](workers.md)
