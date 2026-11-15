---
title: Every worktree gets a lane
description: A second worktree of myapp starts next to the first, on its own ports and its own hostname.
kind: learn
---

The API of `myapp` runs from the `main` branch. Work on a checkout flow starts on a branch, and both versions have to be up at once: one to compare against, one to change. This page adds a second worktree and starts it next to the first.

## A second worktree starts as lane 1

Git puts the branch in a folder of its own. The project file is committed, so the new folder has it, and a start there needs nothing else.

```console title="~/dev/myapp"
$ git worktree add ../myapp-checkout -b feat/checkout
Preparing worktree (new branch 'feat/checkout')
HEAD is now at 712903a Add api service
$ cd ../myapp-checkout
$ devlanes start --no-ui
✓ services ready
myapp@1 · feat/checkout
✓ services  1 of 1 ready
  ✓  api  https://api.myapp1.localhost
```

A lane is one running copy of the project. The first worktree is lane 0, and this one became lane 1 the moment devlanes ran in it.

## Both lanes run at once

Neither lane knows about the other. Each has its own process, and each shows up as a row.

```console title="~/dev/myapp-checkout"
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           running  1/1       0.0%  37.2 MiB  https://myapp.localhost
  1     feat/checkout  running  1/1       0.0%  38.2 MiB  https://myapp1.localhost
```

A lane that sits unused for a while is suspended and wakes on the next request, so leaving several up costs little. [Idle lanes](../concepts/idle-lanes.md) has the details.

## Each lane has its own ports and its own name

The lane number is part of the hostname, and each lane has ports that no other lane uses.

```console title="~/dev/myapp-checkout"
$ devlanes url ls
HOST                      TARGET       PORT   STATE
api.myapp.localhost   ->  myapp@0:api  20060  ready
api.myapp1.localhost  ->  myapp@1:api  20070  ready
proxy: http :80, https :443
```

The names come from the one line in the project file that says `api.${{ lane.domain }}`. In lane 0 the lane's domain is `myapp.localhost`, and in lane 1 it is `myapp1.localhost`. A browser treats the two as separate sites, so a cookie set in one lane never reaches the other.

The ports come from a pool the whole machine shares. Every lane gets a block of them, ten here, and a service sits at the same place in each block: the API is first in both.

## The same request, two answers

Each copy knows which lane it is.

```console
$ curl -s https://api.myapp.localhost/
{"lane":0,"lane_name":"main","service":"api","port":20060}
$ curl -s https://api.myapp1.localhost/
{"lane":1,"lane_name":"lane1","service":"api","port":20070}
```

## On your own project

- [Run a folder twice](../how-to/lanes/two-lanes-one-folder.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Lanes and projects](../concepts/lanes-and-projects.md) and [Ports](../concepts/ports.md)
