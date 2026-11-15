---
title: The daemon
description: One background process per user runs every lane, outlives the terminal that started it, and exits by itself when nothing needs it.
kind: concept
---

One background process per user, the daemon, runs the lanes of every project on the machine. The command line, the TUI and the MCP server are its clients: they send it requests and show what it reports back. Because the processes belong to the daemon and not to a terminal, closing a terminal or quitting the TUI leaves the lanes running.

## The daemon owns what runs

The daemon starts and stops each unit, hands out the ports, holds the proxy routes and the hostnames, keeps the connections to boxes and writes the logs. It also keeps the state database, which records the lanes, their port blocks and which services each one ran. A client holds none of this, so a terminal and an agent that look at the same lane see the same thing.

The MCP server is a client of the same kind. It does not connect to the daemon itself: each tool call runs one devlanes command, which talks to the daemon and ends, so an agent host that keeps the server registered costs one idle process. [Set up a coding agent](../how-to/agents/set-up.md) registers it.

## It starts on first use and exits when idle

A command that needs the daemon and finds none starts it. A few commands only report and never start one, such as the doctor and the daemon's own status. Inside an agent sandbox the start is refused; [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md) covers that case.

A daemon a command started exits about 15 seconds after the last TUI, or command that shows progress, closes, once nothing runs, no box is connected or reconnecting and no pairing code is waiting. The 15 seconds are enough to quit and reopen the TUI, or to type the next command, without waiting for a new daemon. When no TUI or progress view ever watched it, as when only scripts or agents used it, it waits about 5 minutes instead, since no closing TUI marks the end of their work.

A daemon installed as a login service with `devlanes daemon install` does not exit when idle, because the login service would not bring it back. Containers keep running when the daemon exits, but their hostnames answer only while it runs. [`daemon.exit_when_idle`](/docs/reference/config/user/daemon/) turns the idle exit off.

## A new daemon has no shell to start from

The daemon saves which services each lane ran and with which profiles. It does not save the environment of the shell that started them, which holds tokens and other secrets ([Environment and expressions](environment.md) has the reasons). After a reboot or a stop of the daemon, the next daemon knows what ran but not with what, so it waits. Processes that outlived a crashed daemon are different: they keep running, and the next daemon finds them.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH         STATE  SERVICES  CPU  MEM  URL
  0     main           idle   –         –    –    https://myapp.localhost  (needs start from a shell: devlanes start --lane myapp@0)
  1     feat/checkout  idle   –         –    –    https://myapp1.localhost  (needs start from a shell: devlanes start --lane myapp@1)

nothing is running
```

Both lanes keep their numbers, ports and hostnames while they wait. What they lack is an environment, and only a start from a shell supplies one.

## One start from a shell brings a lane back

A plain start in a worktree starts the services its lane ran before, with the environment of the shell it was typed in. It also names the other lanes that ran, and a resume of all lanes starts those.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
! myapp@1 also ran before the daemon stopped; devlanes lane resume starts it and 1 lane of other projects
```

A `devlanes daemon restart` goes the other way round. It starts the new daemon first and hands it the restarting shell's environment in memory, so the lanes come back without a second step. [Bring lanes back after a reboot](../how-to/lanes/after-a-reboot.md) has both paths.

## A failed process stays failed

When a service exits or cannot start, it shows as failed with its error and the end of its output, and stays that way until someone starts, restarts or stops it. Restarting it by itself would hide the failure behind a service that looks up, so devlanes does not. The cost is a service that stays down until you act; [When a service fails](../learn/failures.md) shows what that looks like.

A dev server that restarts itself on a file change is a different case. Its process keeps running, and the proxy retries a refused connection for a few seconds while it comes back.

The daemon measures the CPU and memory of each lane and does not limit them. The one thing it does to a lane on its own is slow down and suspend one that nobody uses, which keeps its processes, ports and memory ([Throttling and freezing](freezing.md)).

## Some terminals end the daemon with them

On Windows, a daemon started from a terminal leaves that terminal's group of processes so that closing the terminal does not reach it. Some terminals do not allow that and end every process they started when they close, an SSH session among them. A daemon started from one of those stops when it closes, and its services stop with it. The doctor warns about such a terminal.

```console title="on Windows 11"
$ devlanes doctor
terminal
  ! this terminal ends the processes it started when it closes, so a daemon started here stops with it
    fix: start the daemon from another terminal
```

The next command then starts a new daemon, which finds its lanes waiting for a start from a shell, as above. A daemon started from Windows Terminal, or installed as a login service, outlives the terminal. [Windows](../reference/windows.md) lists the other differences.

## Related

- [A service, a port and a hostname](../learn/first-service.md)
- [Bring lanes back after a reboot](../how-to/lanes/after-a-reboot.md)
- [Throttling and freezing](freezing.md)
- [Security](security.md)
