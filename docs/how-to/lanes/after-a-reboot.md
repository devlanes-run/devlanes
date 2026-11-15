---
title: Bring lanes back after a reboot
description: Start the lanes that ran before a reboot or a stop of the daemon, and have the daemon start at login.
kind: how-to
---

Use this after a reboot or a stop of the daemon, when the lanes you had running show as idle and their services are down.

## Check what the lanes show

Each lane waits in the `idle` state with the command that brings it back, because the new daemon does not have the environment its services were started with ([The daemon](../../concepts/daemon.md) explains why).

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH         STATE  SERVICES  CPU  MEM  URL
  0     main           idle   –         –    –    https://myapp.localhost  (needs start from a shell: devlanes start --lane myapp@0)
  1     feat/checkout  idle   –         –    –    https://myapp1.localhost  (needs start from a shell: devlanes start --lane myapp@1)

nothing is running
```

The lanes keep their numbers, ports and hostnames while they wait, and come back on the same ones.

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

The last line names the other lanes of myapp and counts those of other projects, since one daemon runs them all.

## Resume the other lanes

Run this from a shell that has the variables your services need, since every lane it starts gets that shell's environment. A lane you stopped by hand before the daemon went away stays stopped.

```console title="~/dev/myapp"
$ devlanes lane resume --no-ui
```

## Start the daemon at login

A login service starts the daemon when you log in and keeps it running while nothing runs.

```console
$ devlanes daemon install
```

On macOS and Linux this starts the daemon at once, under launchd or as a systemd user unit. On Windows it adds a Run entry, and the daemon starts from it at the next logon; until then a running daemon is left alone. A daemon started from the login service belongs to no terminal, so closing one does not end it. It still has no shell to start the lanes from, so they wait as above until a start or a resume.

On macOS, running the install again when the definition it writes has changed, as for a devlanes binary at another path, reloads the agent. That restarts the daemon and stops your services, so resume them afterwards. `devlanes daemon uninstall` removes the login service and leaves a running daemon running.

## If the daemon is still running

When the daemon is up but your services need a changed variable, such as a new `PATH` or a token, restart it from a shell that has the variable. The new daemon gets that shell's environment and starts the services every lane ran, after asking when processes are running.

```console title="~/dev/myapp"
$ devlanes daemon restart
```

[The daemon](../../concepts/daemon.md) explains why this takes one step and a reboot takes two.

## If a plain start should begin from scratch

Set [`start.restore`](/docs/reference/config/user/start/) to false in the user config, and a plain start runs the lane's default profile instead of what ran before. A resume and a daemon restart bring back what ran either way.

## Related

- [The daemon](../../concepts/daemon.md)
- [Environment and expressions](../../concepts/environment.md)
- [Prepare a box](../remote/prepare-a-box.md)
- [States and exit codes](../../reference/states-and-exit-codes.md)
