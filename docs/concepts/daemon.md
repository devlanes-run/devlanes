---
title: The daemon
description: One background process per user runs the lanes, outlives the terminal that started it, exits when nothing runs, and after a reboot waits for a start from a shell.
kind: concept
---

One background process per user, the daemon, runs the lanes of all your projects. The command line, the TUI and the MCP server, which lets a coding agent call devlanes, only send it requests and show what it reports. Because the processes belong to the daemon and not to a terminal, closing a terminal or quitting the TUI leaves your lanes running.

## The daemon holds the lanes

The daemon starts and stops each unit, a service, task or other process of a lane ([Glossary](../reference/glossary.md)). It hands out the ports, runs the proxy that answers the hostnames, keeps the state database and writes the logs. It also holds the connections to boxes, the machines whose lanes are mirrored to yours. A client holds none of this, so a terminal and an agent looking at the same lane see the same thing.

![The CLI, the TUI and the MCP server send requests to the daemon, which holds the processes of each lane, the proxy for hostnames over HTTPS, the state and the logs; containers and boxes sit outside it. The first command starts the daemon, it exits when nothing runs, and after a reboot the lanes wait for a start from a shell](daemon.svg)

Without a command, the daemon throttles and then freezes an unused lane ([Throttling and freezing](freezing.md)). When you delete a lane's worktree it stops that lane, or cleans it when the project sets `lanes.removed_worktrees: clean` ([Lanes and projects](lanes-and-projects.md)).

When you stop the daemon with `devlanes daemon stop`, it runs each lane's `lane.stop` tasks and then stops every process it manages. Containers keep running, but their hostnames answer only while a daemon runs.

## It starts on first use and exits when nothing runs

A command that needs the daemon and finds none starts it. A few commands only report and never start one, such as `devlanes doctor` and the daemon's own status. Inside an agent sandbox the start is refused ([Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)).

A daemon that a command started exits by itself once all of these hold:

- no unit runs, starts or stops, in any lane;
- no box is connected or reconnecting, and no other machine is connected to this one, using it as its box;
- no pairing code, the one-time code that lets another machine connect, is waiting to be used.

It waits about 15 seconds after the last TUI, or command showing progress, has closed, so you can quit and reopen the TUI without waiting for a new daemon. When no TUI or progress view ever watched it, as when only scripts or agents used it, it waits longer, since no closing TUI marks the end of their work. A daemon installed as a login service with `devlanes daemon install` does not exit, because the login service would not bring it back.

## Sleep pauses the lanes with the machine

When you close the lid, the operating system suspends the daemon and every lane together, and they carry on where they were when the machine wakes. Nothing is restarted. On Windows the timers that freeze unused lanes count the night, so lanes you left are frozen soon after waking ([Throttling and freezing](freezing.md)). A connection to a box can drop while the machine sleeps, and the daemon connects it again ([Remote mirroring](remote-mirroring.md)).

## A reboot leaves the lanes waiting for a shell

A reboot or a logout stops the daemon without its `lane.stop` tasks. The daemon has saved which services each lane ran, and which profiles, the named selections of services, the start asked for. It keeps the environment of the last shell that started something in each lane in memory only, because that environment holds secrets ([Environment and expressions](environment.md)), so the next daemon knows what ran but not with which variables.

The lanes then run nothing and wait. Status shows each one as idle, with a note that it needs a start from a shell, and each keeps its number, ports and hostnames. A daemon installed as a login service starts again at login and waits the same way. Containers come back with the lane's next start, which brings them up first, unless Docker's own restart setting started them earlier.

On macOS and Linux, processes that outlive a daemon that crashed keep running, and the next daemon finds them. A unit run in a pseudoterminal, the default, stops with the daemon on Windows ([Windows](../reference/windows.md)).

## A start from a shell brings the lanes back

A plain `devlanes start` in a worktree starts the services its lane ran before, with the environment of the shell you typed it in, and names the other lanes that ran. `devlanes lane resume` starts every lane you did not stop by hand, with the services each one ran.

`devlanes daemon restart` needs no second step: it starts the new daemon before the old one stops, hands it your shell's environment in memory, and starts again what ran. A resume and a restart give one shell's environment to every lane they start, even lanes you first started from different shells; [Environment and expressions](environment.md) says which environment each command leaves a lane. [Bring lanes back after a reboot](../how-to/lanes/after-a-reboot.md) shows both paths.

## A failed service stays failed

When a service exits or cannot start, it shows as failed with its error and the end of its output. It stays that way until you start, restart or stop it, because devlanes does not restart a failed unit by itself: the failure stays in view so you see what broke. [When a service fails](../learn/failures.md) shows what that looks like.

A dev server that restarts itself after a file change keeps its process, so it does not count as failed. While it comes back, the proxy retries refused connections for a few seconds.

## Resources are measured, not capped

The daemon measures the CPU and memory of each lane and shows them in status, and it does not limit them. `devlanes debug top` lists the use of each lane and unit, so you can see which one takes the most.

## Some sessions end the daemon with them

On Windows, some terminals and SSH servers end every process they started when they close, a daemon started there included; [Windows](../reference/windows.md) says which and what to do. On Linux, systemd can stop a user's processes at logout unless linger is on for that user, and the daemon stops with them. That matters on a box you reach over SSH ([Prepare a box](../how-to/remote/prepare-a-box.md)).

## Related

- [A service, a port and a hostname](../learn/first-service.md)
- [Bring lanes back after a reboot](../how-to/lanes/after-a-reboot.md)
- [Throttling and freezing](freezing.md)
- [Security](security.md)
