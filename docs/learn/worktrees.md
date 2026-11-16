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
```

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
```

The start runs the API as lane 1 of myapp, on feat/checkout, at `https://api.myapp1.localhost`. The primary worktree, here the one on the main branch, is lane 0. The API has no dependencies to install, so the folder runs as it is; a project that has them can have devlanes install them when a lane starts, as [Services that find each other](connecting-services.md) shows.

## Both lanes run at once

```console title="~/dev/myapp-checkout"
$ devlanes status
```

The status lists both lanes of the project, lane 0 on main and lane 1 on feat/checkout, from whichever worktree you run it in. When you leave a lane unused for a while, devlanes throttles it and then freezes it, and the next request or command thaws it with its processes and ports as they were; [Throttling and freezing](../concepts/freezing.md) has the details.

## The lane number is in the hostname

The project file asks for one hostname, the API under the lane's domain. That domain is `myapp.localhost` in lane 0 and `myapp1.localhost` in lane 1, so the same line gives each lane a hostname of its own.

```console title="~/dev/myapp-checkout"
$ devlanes url ls
```

The list maps `api.myapp.localhost` to lane 0's API and `api.myapp1.localhost` to lane 1's, each on its own port. A browser treats the two domains as separate sites, so a cookie set in lane 1 is not sent to lane 0. Each lane takes a block of ports from a pool the whole machine shares, and the API has the same place in each block.

## A service is told which lane it runs in

```console title="~/dev/myapp-checkout"
$ devlanes env show api
```

Among the API's variables are `DEVLANES_LANE=1` and `DEVLANES_LANE_NAME=lane1`, which let the code tell lanes apart, for a log line or the name of a database, next to its `PORT` and `HOST`. Lane names follow the lane number: lane 0 is named main whatever its branch, and every other lane is named after its number. [Environment variables](../reference/environment-variables.md) lists every variable devlanes sets.

When you stop a lane, it keeps its number, ports and hostnames for the next start. Next, a web app joins myapp, and each lane's page has to call the API of its own lane.

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Lanes and projects](../concepts/lanes-and-projects.md)
- [Ports](../concepts/ports.md)
