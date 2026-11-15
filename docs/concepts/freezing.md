---
title: Throttling and freezing
description: A lane nobody uses is slowed down, then suspended, and comes back on the next request with its memory, ports and processes as they were.
kind: concept
---

A machine with several lanes runs several copies of each dev server and watcher, and many of them sit unused while you work in one lane. devlanes slows down a lane that sees no activity, then suspends it, and resumes it on the next request. Its processes are not stopped or restarted at any point, so the lane comes back where it was.

## An idle lane is slowed, then suspended

After 5 minutes without activity a lane is throttled: its processes run at background priority and keep serving requests. After 15 minutes it is frozen: every process of the lane is suspended and the containers that belong to the lane alone are paused. Both apply to the whole lane, so activity on one service brings back all of them.

![A lane goes from running to throttled after 5m idle and to frozen after 15m idle; activity brings it back to running](freezing.svg)

Shared containers, such as one Postgres for every lane, are not paused, because other lanes still use them. Both times are settings, [`lanes.throttle_after`](/docs/reference/config/user/lanes/) and [`lanes.freeze_after`](/docs/reference/config/user/lanes/), and either can be turned off. [Run many lanes on one machine](../how-to/lanes/many-lanes.md) changes them.

## A frozen lane keeps everything but CPU time

A frozen process keeps its memory, its open files, its ports and its PID. devlanes does not trim that memory on any OS; whether it moves to swap or gets compressed is up to the OS. Routes and the lane's environment stay too, and the kernel keeps accepting connections on the lane's ports, which are answered once the lane thaws. A frozen lane still counts as running, so the daemon stays up to thaw it.

That is the difference between freezing a lane and restarting it. A restart would throw away a dev server's state, its caches and its open connections; a frozen process resumes where it stopped. Neither throttling nor freezing is a limit either: once the lane is active again, its processes run at normal priority with no cap on CPU or memory.

## Activity is traffic, CPU or a command

Each of these resets a lane's timer and brings it back when it is throttled or frozen:

- a request through the devlanes proxy to one of the lane's hostnames
- bytes a client sends on an open WebSocket through the proxy
- a new TCP connection to one of the lane's ports, its containers' ports included
- the lane's services together using more than a quarter of one CPU core
- a devlanes command that needs the lane's processes, such as a start, a stop, a run or an exec
- typing into a terminal attached to one of the lane's units

Commands that only read, such as status, logs and url, do not count, and neither does the TUI showing a unit's output. So a TUI left open, or an agent that polls status, does not keep a lane awake. A connection to a port counts whether it comes from a browser, a database client or a box's tunnel.

## Waking takes about as long as a request

A request through the proxy to a frozen lane resumes it at once, and the request goes through after the thaw. If the thaw takes longer than 10 seconds, the request is passed on anyway. A direct connection to a port is noticed within about half a second, since devlanes looks for new connections more often while a lane is frozen.

The lane's own containers are unpaused before its processes resume, so a service finds its database answering. If resuming fails, the lane stays marked as throttled or frozen and the next activity tries again. Inside the lane, timers that came due while it was frozen fire at once, file watchers catch up on the changes they missed, and a database may have closed connections that the service then opens again.

## Some lanes stay awake

A lane is not throttled or frozen while a start, stop or clean runs on it, while a task, a setup step or an exec command runs, while it runs no service, or while processes left by an earlier daemon still run in it. A service marked to keep its lane awake keeps the whole lane at full speed, which suits workers and queue consumers that do their work without requests.

Some lanes stay awake without being told. A browser tab whose dev client pings over a WebSocket counts as activity for as long as it is open, and a dev server whose watcher polls the disk can stay above the CPU threshold. Closing the tab, or giving the watcher less to do, lets the lane go idle.

## Each OS suspends differently

The idea is the same on each system, and the mechanism is the one the OS offers.

::::tabs{sync="os"}
:::tab[macOS]
Throttling puts each process of the lane in the background band: the lowest CPU priority, slower disk and network access, and only the efficiency cores on Apple Silicon. Freezing sends a stop signal to each service's process group and to the processes devlanes tracks that left it, and thawing sends the matching continue signal. macOS compresses or swaps out the memory of suspended processes when memory runs short.
:::
:::tab[Linux]
Linux lanes are not throttled. A lane goes from running straight to frozen when its freeze time passes, and the throttle setting has no effect. Freezing and thawing use the same stop and continue signals as on macOS. The kernel swaps out a frozen lane's memory only when swap or zswap is set up.
:::
:::tab[Windows]
Throttling puts each process of the lane in efficiency mode, the same one Task Manager offers: idle priority plus EcoQoS. Freezing suspends each process in a service's job object, and thawing resumes them. Windows trims the working sets of idle processes on its own when memory runs short.
:::
::::

## Related

- [Run many lanes on one machine](../how-to/lanes/many-lanes.md)
- [`lanes.freeze_after`](/docs/reference/config/user/lanes/)
- [The daemon](daemon.md)
- [Start order and readiness](start-order.md)
