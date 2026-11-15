---
title: Ports
description: Each lane takes a block of ports from one pool the whole machine shares, each service has the same place in each lane's block, and ports differ from one machine to another.
kind: concept
---

The lanes of all projects on a machine take their ports from one shared pool, so several projects, and several lanes of each, run at the same time without anyone keeping a list of ports. A service does not choose its port. It is given one, in the same place of its lane's block in each lane, and listens there.

## Each lane takes a block from one pool

The pool runs from 20000 to 32767 by default, below the range Linux usually picks ports for outgoing connections from, so the system does not hand a lane's port to another program's connection. [`ports.range`](/docs/reference/config/user/ports/) moves it.

A new lane takes the lowest block that is free, and no two blocks overlap, whatever project they belong to. In the captures on this site myapp's first block starts at 20060, because lanes of other projects on that machine held the blocks below it.

![The port pool from 20000 to 32767: other lanes hold the first blocks, then a block for each lane of myapp, where api and web have the same place and the rest of the block is spare](ports.svg)

A block holds the ports the project needs plus half again, rounded up to a multiple of ten. A project with one or two ports gets ten, and one with seven gets twenty. [`lanes.block_size`](/docs/reference/config/project/lanes/) fixes the size instead.

## A service has the same place in every lane

Inside the block each service has an offset, the same in each lane of the project. In myapp the API is the first port of each block and the web service the second.

```console title="~/dev/myapp-checkout"
$ devlanes url ls
HOST                      TARGET       PORT   STATE
api.myapp.localhost   ->  myapp@0:api  20060  ready
myapp.localhost       ->  myapp@0:web  20061  ready
api.myapp1.localhost  ->  myapp@1:api  20070  ready
myapp1.localhost      ->  myapp@1:web  20071  ready
proxy: http :80, https :443
```

The daemon stores the offsets, so they survive a restart of the daemon, an edit of the project file and a switch of branch. An offset stays even after you remove its service from the file, until no worktree's file has that service and `devlanes lane gc` drops it.

## Ports differ between machines and hostnames do not

Each machine has its own pool and its own lanes, so the ports of a lane depend on what else that machine holds. A colleague who clones myapp may get the API of lane 0 on 20000, where yours is on 20060. Hostnames are built from the project name and the lane number, so `api.myapp.localhost` names lane 0's API on both machines. Put hostnames, not ports, in a README, a bookmark or an OAuth callback.

## A block grows when the lane restarts

When you add a service, a running lane keeps its block as long as the new ports fit in it. When they do not fit, the lane has to restart to move to a larger block. `devlanes reload` without unit names restarts the whole lane for that, and a start into the running lane refuses and asks you to stop the lane and start it again.

A lane keeps its block while it exists, running or stopped. The block is freed when the lane is cleaned, by `devlanes lane clean` or by a garbage collection of lanes whose worktree is gone ([Remove lanes](../how-to/lanes/clean-up.md)).

## A service is handed its port

devlanes passes a service its port in the `PORT` variable and the address in `HOST`, and the service has to listen there. The proxy, the ready check and the URLs other services read all connect to that port on 127.0.0.1. devlanes does not look for a port a program picked by itself; it lists what a process listens on only to explain a failure.

A dev server that takes its port as a flag gets it from an expression instead.

```yaml devlanes
services:
  web:
    run: python -m http.server ${{ self.port }} --bind 127.0.0.1
```

## Named ports come from the same block

A service that needs a second port, for a debugger or for metrics, names it. Naming it in any expression of the service's own settings, its environment included, gives it a port in the block, with the same offset in each lane. Some presets and runtimes ask for one the same way.

```yaml devlanes
services:
  api:
    run: node --inspect=127.0.0.1:${{ self.ports.inspect }} server.mjs
```

Other services can read that port, but only the service that owns it allocates it, and a reference from elsewhere to a name the owner never uses fails as undefined. `devlanes status --json` lists each unit's ports by name. [Give a service several ports and hostnames](../how-to/services/ports-and-hostnames.md) routes a hostname to a named port.

## A fixed port is the same in every lane

A service can set its port itself, to a number or to a place in the block.

```yaml devlanes
services:
  web:
    env:
      PORT: 3998
  api:
    env:
      PORT: "${{ lane.port + 5 }}"
```

The number holds in each lane and takes no place in the block, so only one lane can run the web service at a time; a second lane that starts it fails on a port that is taken and names the lane that holds it. The expression reads the start of the lane's block, so each lane gets a port of its own and that place in the block is reserved.

## Some ports stay out of blocks

New blocks keep clear of ports that a connected box, a machine whose lanes are mirrored to yours, holds on this machine. Browsers refuse to connect to a list of ports, all of them at 10080 or below, and blocks and offsets skip those; that matters only when `ports.range` or a fixed block start reaches that low. On Windows, blocks also skip the ranges Hyper-V and WSL reserve ([Windows](../reference/windows.md)).

## Related

- [Every worktree gets a lane](../learn/worktrees.md)
- [Give a service several ports and hostnames](../how-to/services/ports-and-hostnames.md)
- [Lanes and projects](lanes-and-projects.md)
- [Hostnames and HTTPS](hostnames-and-https.md)
