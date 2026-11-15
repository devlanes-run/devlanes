---
title: Every worktree gets a lane
description: A second worktree of myapp starts next to the first, on its own ports and its own hostname.
kind: learn
---

The API of myapp runs from the main branch. Work on a checkout flow happens on a branch of its own, and both have to be up together: the old one to compare against, the new one to change.

## A second worktree starts as lane 1

Git's worktree command checks the branch out into a second folder next to the first. The project file is committed, so the new folder has it, and a start there is the same command as before.

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
✓ services ready
myapp@1 · feat/checkout
✓ services  1 of 1 ready
  ✓  api  https://api.myapp1.localhost
```

The worktree of the main branch is lane 0. This one became lane 1 when the start ran in it. The API has no dependencies to install, so the folder runs as it is; a project that has them installs them with a task, as [Services that find each other](connecting-services.md) does.

## Both lanes run at once

The two lanes share no process and no folder. Each runs its own API from its own worktree.

```console title="~/dev/myapp-checkout"
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           running  1/1       0.0%  38.2 MiB  https://myapp.localhost
  1     feat/checkout  running  1/1       0.0%  38.6 MiB  https://myapp1.localhost
```

A lane nobody uses is suspended after a while and resumes on the next request, keeping its memory, ports and processes. [Throttling and freezing](../concepts/freezing.md) says what counts as use.

## The lane number is in the hostname

The project file asks for one hostname, the API under the lane's domain. That domain is `myapp.localhost` in lane 0 and `myapp1.localhost` in lane 1, so the same line gives each lane a hostname of its own.

```console title="~/dev/myapp-checkout"
$ devlanes url ls
HOST                      TARGET       PORT   STATE
api.myapp.localhost   ->  myapp@0:api  20060  ready
api.myapp1.localhost  ->  myapp@1:api  20070  ready
proxy: http :80, https :443
```

A browser treats the two domains as separate sites, so a cookie set in lane 1 is not sent to lane 0. The ports differ as well. Each lane takes a block of ports from a pool the whole machine shares, and the API has the same place in each block.

## A service is told which lane it runs in

Besides the port, devlanes sets a few variables for each service. They let the code tell lanes apart, for a log line or the name of a database.

```console title="~/dev/myapp-checkout"
$ devlanes env show api
DEVLANES_LANE=1
DEVLANES_LANE_NAME=lane1
DEVLANES_SERVICE=api
HOST=127.0.0.1
PORT=20070
```

Lane 0 is named main and each other lane is named after its number, which is why this one reads lane1 and not its branch. [Environment variables](../reference/environment-variables.md) lists the rest.

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Lanes and projects](../concepts/lanes-and-projects.md)
- [Ports](../concepts/ports.md)
