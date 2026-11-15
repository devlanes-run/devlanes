---
title: Bring lanes back after a reboot
description: Start the lanes that ran before a reboot or a stop of the daemon, and have the daemon start at login.
kind: how-to
---

Use this after a reboot or a stop of the daemon, when the lanes you had running show as idle and their services are down.

## Check what the lanes show

A new daemon knows which services each lane ran and with which profiles, but not the environment of the shell that started them, which devlanes never writes to disk ([Environment and expressions](../../concepts/environment.md) has the reasons). So each lane waits, in the `idle` state, with the command that brings it back.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH         STATE  SERVICES  CPU  MEM  URL
  0     main           idle   –         –    –    https://myapp.localhost  (needs start from a shell: devlanes start --lane myapp@0)
  1     feat/checkout  idle   –         –    –    https://myapp1.localhost  (needs start from a shell: devlanes start --lane myapp@1)

nothing is running
```

The lanes keep their numbers, ports and hostnames while they wait, so nothing in the project changes when they come back.

## Start the lane you work in

A plain start in a worktree brings back the services its lane ran, with the same profiles and with the environment of the shell you type it in. It also names the other lanes that ran.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
! myapp@1 also ran before the daemon stopped; devlanes lane resume starts it and 1 lane of other projects
```

The last line names the other lanes of myapp and counts those of other projects, since one daemon runs them all. `devlanes lane resume` starts each of them with the environment of the shell you run it in, so run it from a shell that has the variables your services need. A lane you stopped by hand before the daemon went away stays stopped.

## Start the daemon at login

A login service starts the daemon when you log in and keeps it running while nothing runs. A daemon started that way belongs to no terminal, so closing one cannot end it and its services.

```console
$ devlanes daemon install
```

On macOS and Linux this starts the daemon at once, under launchd or as a systemd user unit. On Windows it adds a Run entry for the next logon and leaves a running daemon alone. A daemon started this way still has no shell to start the lanes from, so they wait as above until a start or `devlanes lane resume`. `devlanes daemon uninstall` removes the login service and leaves a running daemon running.

## If the daemon is still running

When the daemon is up but its services need a changed variable, such as a new `PATH` or a token, `devlanes daemon restart` from a shell that has it starts a new daemon, hands it that shell's environment, and starts the services that ran. It asks first when processes are running. [The daemon](../../concepts/daemon.md) explains why this takes one step and a reboot takes two.

## If a plain start should begin from scratch

Set [`start.restore`](/docs/reference/config/user/start/) to false in the user config, and a plain start runs the lane's default profile instead of what ran before. `devlanes lane resume` and a daemon restart bring back what ran either way.

## Related

- [The daemon](../../concepts/daemon.md)
- [Environment and expressions](../../concepts/environment.md)
- [Prepare a box](../remote/prepare-a-box.md)
- [States and exit codes](../../reference/states-and-exit-codes.md)
