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

The service keeps its port and its hostname while it is stopped. A request to the hostname gets a page from the proxy that names the command to start it.

## Start it by name

Name the service, or one of its tags, and the start runs it next to what the lane already runs:

```console title="~/dev/myapp"
$ devlanes start docs
```

The TUI can start it as well. Once it has run, the lane counts it among the services it runs: a plain start after a reboot or a daemon restart brings it back. A stop of the service, or of the whole lane, makes devlanes forget it again.

## Use it with profiles

A profile selects services by name, glob or tag. A name or a tag includes an on-demand service; a glob such as `*` skips it, so a profile meant to start everything still leaves it out:

```yaml devlanes
profiles:
  all:
    include: ["*"]
  docs: [+docs]
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
