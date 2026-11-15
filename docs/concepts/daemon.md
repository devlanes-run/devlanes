---
title: The daemon
description: One background process per user runs the lanes, outlives the terminal that started it, exits when unused, and waits for a shell after a reboot.
kind: concept
---

One background process per user, the daemon, runs the lanes of all your projects. The command line, the TUI and the MCP server only send it requests and show what it reports. Because the processes belong to the daemon and not to a terminal, closing a terminal or quitting the TUI leaves your lanes running.

## The daemon holds the lanes

The daemon starts and stops each unit, hands out the ports, runs the proxy that answers the hostnames, keeps the state database and writes the logs. It also holds the connections to boxes, the machines whose lanes are mirrored to yours. A client holds none of this, so a terminal and an agent looking at the same lane see the same thing. The MCP server is a client too: each tool call runs one devlanes command.

![The CLI, the TUI and the MCP server send requests to the daemon, which holds the processes of each lane, the proxy, the state and the logs](daemon.svg)

When you stop the daemon with `devlanes daemon stop`, it runs each lane's stop tasks and then stops every process it manages. Containers keep running, but their hostnames answer only while a daemon runs.

## It starts on first use and exits when unused

A command that needs the daemon and finds none starts it. A few commands only report and never start one, such as `devlanes doctor` and the daemon's own status. Inside an agent sandbox the start is refused ([Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)).

A daemon that a command started exits by itself once it has nothing to do, which means all of these hold:

- no unit runs or is starting, in any lane;
- no box is connected or reconnecting, and no machine is connected to this one as a box;
- no pairing code is waiting to be used.

It then waits about 15 seconds after the last TUI, or command showing progress, has closed, so you can quit and reopen the TUI without waiting for a new daemon. A daemon that no TUI or progress view ever watched, as when only scripts or agents used it, waits about 5 minutes instead, since no closing TUI marks the end of their work. One installed as a login service with `devlanes daemon install` does not exit, because the login service would not bring it back. [`daemon.exit_when_idle`](/docs/reference/config/user/daemon/) turns the exit off.

## Sleep pauses the lanes with the machine

When you close the lid, the operating system suspends the daemon and every lane together, and they carry on where they were when the machine wakes. Nothing is restarted. A connection to a box drops while the machine sleeps, and the daemon connects it again after waking ([Remote mirroring](remote-mirroring.md)).

## A reboot leaves the lanes waiting for a shell

The daemon saves which services each lane ran and with which profiles. It does not save the environment of the shell that started them, which [Environment and expressions](environment.md) explains. So after a reboot, or after you stop the daemon, the next daemon knows what ran but not with which variables, and it waits.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH         STATE  SERVICES  CPU  MEM  URL
  0     main           idle   –         –    –    https://myapp.localhost  (needs start from a shell: devlanes start --lane myapp@0)
  1     feat/checkout  idle   –         –    –    https://myapp1.localhost  (needs start from a shell: devlanes start --lane myapp@1)

nothing is running
```

Idle here means a lane that ran before and waits for an environment. Both lanes keep their numbers, ports and hostnames. A daemon installed as a login service starts again at login but waits the same way, since it has no shell either. Containers come back with the lane's next start, which brings them up first, unless Docker's own restart setting started them earlier.

On macOS and Linux, processes that outlive a daemon that crashed keep running, and the next daemon finds them. On Windows a unit run in a terminal of its own, the default there, stops with the daemon ([Windows](../reference/windows.md)).

## A start from a shell brings the lanes back

A plain `devlanes start` in a worktree starts the services its lane ran before, with the environment of the shell you typed it in, and names the other lanes that ran. `devlanes lane resume` starts all of them at once: every lane you did not stop by hand gets the services it ran and this shell's environment, even lanes you first started from different shells. [Bring lanes back after a reboot](../how-to/lanes/after-a-reboot.md) walks through both.

`devlanes daemon restart` needs no second step. It starts the new daemon before the old one stops, hands it the restarting shell's environment in memory, and starts again what ran. That one shell's environment goes to every lane.

## A failed service stays failed

When a service exits or cannot start, it shows as failed with its error and the end of its output. It stays that way until you start, restart or stop it, because devlanes does not restart a failed unit by itself: the failure stays in view so you see what broke. [When a service fails](../learn/failures.md) shows what that looks like.

A dev server that restarts itself after a file change keeps its process, so it does not count as failed. While it comes back, the proxy retries refused connections for a few seconds.

## Resources are measured, not capped

The daemon measures the CPU and memory of each lane and shows them in status, and it does not limit them. On its own, it slows down and then suspends a lane nobody uses, which keeps the lane's processes and memory ([Throttling and freezing](freezing.md)). It also stops a lane when you delete its worktree ([Lanes and projects](lanes-and-projects.md)).

## Some sessions end the daemon with them

On Windows, some terminals and SSH servers end every process they started when they close, a daemon started there included; [Windows](../reference/windows.md) says which and what to do. On Linux, logging out when linger is off for your user lets systemd stop your processes, the daemon among them, which matters on a box you reach over SSH ([Prepare a box](../how-to/remote/prepare-a-box.md)).

## Related

- [A service, a port and a hostname](../learn/first-service.md)
- [Bring lanes back after a reboot](../how-to/lanes/after-a-reboot.md)
- [Throttling and freezing](freezing.md)
- [Security](security.md)
