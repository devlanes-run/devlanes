---
title: Run many lanes on one machine
description: Tune when unused lanes slow down and freeze, keep the lanes that must stay awake, pace large starts and keep disk use in check.
kind: how-to
---

Use this when you keep many lanes on one machine, for yourself or for agents, and want the unused ones to cost little without stalling the ones that work in the background.

::example{slug="showcase-many-services"}

## Change when lanes slow down and freeze

An unused lane is throttled after a while and frozen later, and the next request thaws it ([Throttling and freezing](../../concepts/freezing.md) says what counts as use). Both times are settings in the user config, and `0s` turns either one off.

```yaml devlanes-user
lanes:
  throttle_after: 10m
  freeze_after: 1h
```

The throttle time has to be the shorter one when both are on. On Linux, lanes are not throttled, so only the freeze time applies. [`lanes.throttle_after`](/docs/reference/config/user/lanes/) and [`lanes.freeze_after`](/docs/reference/config/user/lanes/) have the defaults.

## Restart the daemon

The daemon reads the user config when it starts, so a change to the freeze times, the start gate or the log limits on this page applies after a restart. Run it from a shell that has the variables your services need, since the services that ran start again with its environment.

```console
$ devlanes daemon restart
```

## Keep a lane awake

A worker, a queue consumer or a tunnel does its work without requests, so a frozen lane would stall it. Mark such a service, and its whole lane is neither throttled nor frozen while it runs.

```yaml devlanes
services:
  worker:
    run: node worker.mjs
    keep_awake: true
```

The value can be an expression, such as one that keeps only lane 0 awake, and a local file can set it on your machine alone. [`services.<name>.keep_awake`](/docs/reference/config/project/services/) has the details.

## Check a lane after a freeze

Freeze the lane now, whatever the timers say, use the app, and look for connections a service does not open again or timers that misfire once it thaws.

```console title="~/dev/myapp"
$ devlanes debug freeze
```

Any request to the lane thaws it, and `devlanes debug thaw` thaws it without one.

## Pace large starts

Every start goes through a start gate shared by all lanes and projects, which limits how many services start at once. A start of many lanes, such as a resume after a reboot, is where you see it: services waiting for their turn show as queued.

```yaml devlanes-user
startup:
  mode: fixed
  max: 4
```

A fixed window keeps the number constant, and the default adapts it to CPU and memory. For one start that should skip the gate, add `--all-at-once` to it. [`startup.mode`](/docs/reference/config/user/startup/) lists the other settings.

## Free the memory of a lane

A frozen lane keeps all its memory, and devlanes does not trim it. To give the memory of its processes back, stop the lane in its worktree.

```console title="~/dev/myapp-checkout"
$ devlanes stop
```

The lane keeps its number, ports and data folder, and the next plain start begins from its default services. Its own containers keep running and keep their memory until the lane is cleaned, as [Remove lanes](clean-up.md) describes.

## Keep disk use in check

The logs of every lane share one size and age limit, and the oldest output is deleted first. Data folders grow with what services write to them, and like the volumes of a lane's own containers they stay until the lane is cleaned.

```yaml devlanes-user
logs:
  retention:
    days: 3
    max_size: 2GiB
```

A dry run of a clean shows how much a lane's data takes before you remove it. [`lanes.data_dir`](/docs/reference/config/user/lanes/) moves the data folders to another disk, and lanes then start with empty ones there unless you move the old folders by hand.

## If a lane never freezes

Something still counts as use. A browser tab whose dev client sends messages over a WebSocket through the proxy counts, and so does a watcher that keeps the lane's CPU busy. CPU use keeps a lane from freezing and brings a throttled lane back to full speed, but it never thaws a frozen lane, whose processes use none. Close the tab, or check the other kinds of use in [Throttling and freezing](../../concepts/freezing.md).

## If a project should hold fewer lanes

[`lanes.max`](/docs/reference/config/project/lanes/) in the project file caps the lanes of one project. A worktree past the cap gets no lane and no ports until another lane is cleaned. The machine's port range caps all projects together, since each lane takes a block from it.

## Related

- [Throttling and freezing](../../concepts/freezing.md)
- [Start order and readiness](../../concepts/start-order.md)
- [Run workers and replicas](../services/workers.md)
- [Remove lanes](clean-up.md)
