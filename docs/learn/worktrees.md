---
title: Every worktree gets a lane
description: A second worktree of myapp starts next to the first, on its own ports and its own hostname.
kind: learn
---

The API of myapp runs from the main branch. You start work on a checkout flow in a branch of its own, feat/checkout, and you want both up together: the old one to compare against, the new one to change.

## A second worktree starts as lane 1

You check the branch out into a second folder with Git. The project file is committed, so the new worktree has it, and you start its lane with the same command as before.

```console title="~/dev/myapp"
$ git worktree add ../myapp-checkout -b feat/checkout
Preparing worktree (new branch 'feat/checkout')
HEAD is now at 4e4341c Add api service
```

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
✓ services ready
myapp@1 · feat/checkout
✓ services  1 of 1 ready
  ✓  api  https://api.myapp1.localhost
```

The primary worktree, here the one on the main branch, is lane 0. This one became lane 1 when the start ran in it. The API has no dependencies to install, so the folder runs as it is; a project that has them can have devlanes install them when a lane starts, as [Services that find each other](connecting-services.md) shows.

## Both lanes run at once

The status lists every lane of the project, from whichever worktree you run it in.

```console title="~/dev/myapp-checkout"
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           running  1/1       0.0%  37.2 MiB  https://myapp.localhost
  1     feat/checkout  running  1/1       0.0%  38.5 MiB  https://myapp1.localhost
```

When you leave a lane unused for a while, with no requests, connections, devlanes commands or CPU use, devlanes throttles it and then freezes it. The next request, connection or command thaws it with its processes and ports as they were. [Throttling and freezing](../concepts/freezing.md) has the details.

## The lane number is in the hostname

The project file asks for one hostname, the API under the lane's domain. That domain is `myapp.localhost` in lane 0 and `myapp1.localhost` in lane 1, so the same line gives each lane a hostname of its own.

```console title="~/dev/myapp-checkout"
$ devlanes url ls
HOST                      TARGET       PORT   STATE
api.myapp.localhost   ->  myapp@0:api  20060  ready
api.myapp1.localhost  ->  myapp@1:api  20070  ready
```

A browser treats the two domains as separate sites, so a cookie set in lane 1 is not sent to lane 0. Each lane also takes a block of ports of its own from a pool the whole machine shares, and the API has the same place in each block.

## A service is told which lane it runs in

These are some of the variables the API gets: the ones that name its lane and service, which let the code tell lanes apart, and where to listen.

```console title="~/dev/myapp-checkout"
$ devlanes env show api
DEVLANES_LANE=1
DEVLANES_LANE_NAME=lane1
DEVLANES_SERVICE=api
HOST=127.0.0.1
PORT=20070
```

Lane names follow the lane number: lane 0 is named main whatever its branch, and every other lane is named after its number, so this one is lane1. [Environment variables](../reference/environment-variables.md) lists every variable devlanes sets.

## A stopped lane keeps its place

At the end of the day you stop each lane in its own worktree.

```console title="~/dev/myapp"
$ devlanes stop
stopped myapp@0:api
```

```console title="~/dev/myapp-checkout"
$ devlanes stop
stopped myapp@1:api
```

A stopped lane keeps its number, ports and hostnames, so the next start puts the API back where it was. Next, a web app joins, and each lane's page has to call the API of its own lane.

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Lanes and projects](../concepts/lanes-and-projects.md)
- [Ports](../concepts/ports.md)
