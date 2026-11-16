---
title: Run many lanes on one machine
description: Tune when unused lanes slow down and freeze, keep the lanes that must stay awake, pace large starts and keep disk use in check.
kind: how-to
---

Use this when you keep many lanes on one machine and want the unused ones to cost little without stalling the ones that work in the background.

::example{slug="showcase-many-services"}

## Change when lanes slow down and freeze

An unused lane is throttled, then frozen, and the next request brings it back ([Throttling and freezing](../../concepts/freezing.md)). Set both times in the user config; `0s` turns either off.

```yaml devlanes-user
lanes:
  throttle_after: 10m
  freeze_after: 1h
```

## Keep a lane awake

A worker or a queue consumer works without requests, so mark it, and its lane is never throttled or frozen while it runs.

```yaml devlanes
services:
  worker:
    run: node worker.mjs
    keep_awake: true
```

## Pace large starts

Every start goes through a start gate that limits how many services start at once across all lanes. A fixed window keeps that number constant.

```yaml devlanes-user
startup:
  mode: fixed
  max: 4
```

## Keep disk use in check

Logs share one age and size limit across all lanes. Data folders and the volumes of a lane's own containers stay until the lane is cleaned, as [Remove lanes](clean-up.md) shows.

```yaml devlanes-user
logs:
  retention:
    days: 3
    max_size: 2GiB
```

## Apply the changes

The daemon reads the user config when it starts, so restart it from a shell that has your services' variables.

```console
$ devlanes daemon restart
```

## If a lane never freezes

A browser tab whose dev client keeps sending over a WebSocket counts as use, and so does a busy watcher. Close the tab, or see [Throttling and freezing](../../concepts/freezing.md) for what else counts.

## Related

- [Throttling and freezing](../../concepts/freezing.md)
- [`lanes.freeze_after`](/docs/reference/config/user/lanes/#lanes-freeze_after)
- [`services.<name>.keep_awake`](/docs/reference/config/project/services/#services-name-keep_awake)
- [`startup.mode`](/docs/reference/config/user/startup/#startup-mode)
