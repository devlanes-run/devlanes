---
title: Throttling and freezing
description: An unused lane is slowed down, then frozen, and thaws on the next request with its memory, ports and processes as they were.
kind: concept
---

A machine with several lanes runs several copies of each dev server and watcher, and many of them sit unused while you work in one lane. devlanes slows an unused lane down, then freezes it, and thaws it when something uses it again. Its processes are not stopped or restarted along the way, so the lane comes back where it was.

## An unused lane is throttled, then frozen

By default a lane is throttled after 5 minutes without activity: its processes run at background priority and keep serving requests. By default it is frozen 15 minutes after its last activity: each of its processes is suspended, and the containers that belong to the lane alone are paused. Both apply to the whole lane, so activity on one service thaws all of them.

![An unused lane goes from running to throttled after the throttle time and to frozen once the freeze time has passed since its last activity; activity thaws it back to running](freezing.svg)

Shared containers, such as one Postgres for every lane, are not paused, since other lanes use them. The two times are [`lanes.throttle_after`](/docs/reference/config/user/lanes/) and [`lanes.freeze_after`](/docs/reference/config/user/lanes/) in the user config, and either can be turned off; [Run many lanes on one machine](../how-to/lanes/many-lanes.md) changes them.

## A frozen lane keeps its state

A frozen process keeps its memory, its open files, its ports and its PID. devlanes does not trim that memory; whether the OS moves some of it to swap is up to the OS. The lane keeps its routes and its environment, and the kernel goes on accepting connections to its ports, which are answered once the lane thaws.

A restart would throw away a dev server's state, its caches and its open connections, while a frozen process carries on where it stopped. A frozen lane still counts as running, so the daemon stays up to thaw it. Neither throttling nor freezing caps CPU or memory.

## Traffic and commands count as activity

Each of these resets a lane's timer, and thaws the lane when it is throttled or frozen:

- a request through the devlanes proxy to one of the lane's hostnames
- bytes your browser or another client sends on an open WebSocket through the proxy
- a new TCP connection to one of the lane's ports, its own containers' ports included
- a devlanes command that needs the lane's processes, such as a start, a stop or an exec
- typing into a terminal attached to one of the lane's units

The CPU use of the lane's services also counts while they run: a busy worker keeps its lane from being throttled and ends a throttle. It cannot thaw a frozen lane, whose processes use none. Connections to a shared container do not count, since they say nothing about one lane.

Commands that only look at a lane do not count, such as reading its logs or a status, even the status that gives a new worktree its lane number. Neither does the TUI showing a unit's output, so a TUI left open, or an agent that polls status, does not keep a lane awake. [States and exit codes](../reference/states-and-exit-codes.md) gives the CPU threshold and how quickly activity is noticed.

## A request waits for the lane to thaw

A request through the proxy to a frozen lane thaws it, and the proxy holds the request until the lane's processes run again, up to a limit, then passes it on. A direct connection to one of the lane's ports is noticed within moments while the lane is frozen. The lane's own containers are unpaused before its processes thaw, so a service finds its database answering.

If thawing fails, the lane stays marked as throttled or frozen, and the next activity tries again. Inside the lane, timers that came due while it was frozen fire at once, and a database may have closed connections that the service then opens again.

## Closing the lid changes nothing

When you close the lid, the machine suspends the daemon and every process with it, and devlanes stops nothing. The timers run on a clock that, on macOS and Linux, does not advance while the machine sleeps, so the night does not count as time unused there.

In the morning each lane is in the state it was in when you closed the lid. A frozen lane stays frozen until your first request or command thaws it. On macOS and Linux, a lane you were still using goes on counting from where it was when the lid closed.

## Some lanes stay awake

A lane is not throttled or frozen while:

- a start, a stop or a clean is under way in it
- a task, a setup entry or a command started with `devlanes exec` runs in it
- it runs no service
- processes that an earlier daemon left running are still in it
- one of its services has [`services.<name>.keep_awake`](/docs/reference/config/project/services/) set

The last suits workers and queue consumers, which do their work without requests and would stall in a frozen lane. Some lanes stay awake without being told: a browser tab whose dev client pings over a WebSocket counts as activity for as long as it is open, and a dev server whose watcher polls the disk can keep its CPU use up.

## Each OS suspends differently

::::tabs{sync="os"}
:::tab[macOS]
Throttling puts each process of the lane in the background band: the lowest CPU priority, slower disk and network access, and only the efficiency cores on Apple Silicon. Freezing sends a stop signal to each service's process group, and to the processes devlanes follows after they left that group. Thawing sends the matching continue signal.
:::
:::tab[Linux]
Linux lanes are not throttled, because Linux has no way for an unprivileged process to lower a process's priority and raise it again later. A lane goes from running straight to frozen, and the throttle time has no effect. Freezing and thawing use the same stop and continue signals as on macOS.
:::
:::tab[Windows]
Throttling puts each process of the lane in efficiency mode, the same one Task Manager offers: idle priority plus EcoQoS. Freezing suspends each process in a service's job object, and thawing lets them run again.
:::
::::

## Related

- [Run many lanes on one machine](../how-to/lanes/many-lanes.md)
- [`lanes.freeze_after`](/docs/reference/config/user/lanes/)
- [The daemon](daemon.md)
- [States and exit codes](../reference/states-and-exit-codes.md)
