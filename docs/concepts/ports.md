---
title: Ports
description: Each lane takes a block of ports from one pool the whole machine shares, and each service has the same place in each lane's block.
kind: concept
---

The lanes of all projects on a machine take their ports from one shared pool, so two projects and several lanes of each run at the same time without anyone keeping a list of ports. A service does not choose its port. It is given one, in the same place of its lane's block in every lane, and listens there.

## Each lane takes a block from one pool

The pool runs from 20000 to 32767 by default. That range sits below the one Linux usually picks ports for outgoing connections from, so the system does not hand a lane's port to some other program's connection. [`ports.range`](/docs/reference/config/user/ports/) moves it.

![The port pool from 20000 to 32767 holds a block of ten ports for each lane of myapp; api and web have the same place in lane 0 and lane 1](ports.svg)

A lane gets a block of consecutive ports that overlaps no other lane's, whatever project it belongs to. The block holds what the project needs plus half again, rounded up to ten, so a project with one or two services gets ten ports and one with seven gets twenty. The spare room lets a running lane keep its block when the project file gains a service. When an edit needs more than the block holds, the lane has to be stopped and started again to move to a larger one; devlanes says so instead of moving ports under running services.

## A service has the same place in every lane

Inside the block, each service has an offset, and the offset is the same in every lane of the project. In myapp the API is the first port of each block and the web app the second.

```console title="~/dev/myapp-checkout"
$ devlanes url ls
HOST                      TARGET       PORT   STATE
api.myapp.localhost   ->  myapp@0:api  20060  ready
myapp.localhost       ->  myapp@0:web  20061  ready
api.myapp1.localhost  ->  myapp@1:api  20070  ready
myapp1.localhost      ->  myapp@1:web  20071  ready
proxy: http :80, https :443
```

The daemon stores the offsets, so they survive restarts of the daemon, edits to the project file and branch switches. A service the project file drops keeps its offset until no worktree's file has it and `devlanes lane gc` removes it, so a branch that still has the service finds it where it was.

## The service listens where it is told

devlanes passes a service its port in the `PORT` variable and the address in `HOST`, and the service has to listen there. The proxy, the ready checks and the URLs other services read all connect to that port on 127.0.0.1. devlanes does not look for the port a program picked by itself; it lists what a process listens on only to explain a failure.

A dev server that takes its port as a flag gets it from an expression instead.

```yaml devlanes
services:
  web:
    run: python -m http.server ${{ self.port }} --bind 127.0.0.1
```

The port is known before the process starts. That is what lets one service's config name another service's port or URL, as [Services that find each other](../learn/connecting-services.md) shows.

## Named ports come from the same block

A service that needs a second port, for a debugger or for metrics, names it. Naming it in the service's own command, hostnames or ready check gives it a port of its own in the block, with an offset that is the same in every lane.

```yaml devlanes
services:
  api:
    run: node --inspect=127.0.0.1:${{ self.ports.inspect }} server.mjs
```

Other services can read that port, but only the service that owns it allocates it. A reference from another service to a name its owner never uses fails as undefined. [Give a service several ports and hostnames](../how-to/services/ports-and-hostnames.md) routes a hostname to a named port.

## A port a service insists on is the same in every lane

Some programs need one port, because a client has it written into its code. A service can set its port to a number, which then holds in every lane and takes no place in the block.

```yaml devlanes
services:
  worker:
    env:
      PORT: 3998
```

Only one lane can run that service at a time. A second lane that starts it fails on a port that is taken, and says which lane holds it. A port computed from the start of the block, such as the start plus five, keeps the lanes apart and reserves that place in each block.

## Some ports are never handed out

Browsers refuse to connect to some ports, 6000 and 10080 among them. Blocks and offsets skip the ports Chrome and Firefox refuse, so each service's port opens in a browser. New blocks also keep clear of ports that a connected box mirrors to this machine. On Windows they skip the ranges Hyper-V and WSL reserve, which [Windows](../reference/windows.md) describes.

## Related

- [Every worktree gets a lane](../learn/worktrees.md)
- [Give a service several ports and hostnames](../how-to/services/ports-and-hostnames.md)
- [Lanes and projects](lanes-and-projects.md)
- [Environment variables](../reference/environment-variables.md)
