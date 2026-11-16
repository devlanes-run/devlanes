---
title: Bring lanes back after a reboot
description: Start the lanes that ran before a reboot or a stop of the daemon, and have the daemon start at login.
kind: how-to
---

Use this after a reboot or a stop of the daemon, when the lanes you had running show as idle.

## Start the lane you work in

A new daemon knows what each lane ran but not the shell environment it ran with ([The daemon](../../concepts/daemon.md) explains why), so each lane waits for a start from a shell. A plain start in a worktree brings back what its lane ran, with this shell's environment, and names the other lanes that ran.

```console title="~/dev/myapp"
$ devlanes start --no-ui
```

## Resume the other lanes

This starts every other lane that ran, in every project, with this shell's environment. Lanes you stopped by hand stay stopped.

```console title="~/dev/myapp"
$ devlanes lane resume --no-ui
```

## Start the daemon at login

A login service starts the daemon when you log in and keeps it running. The lanes still wait for a start or a resume from a shell.

```console
$ devlanes daemon install
```

`devlanes daemon uninstall` removes it again.

## If the daemon is still running

To give your services a changed variable, such as a new `PATH`, restart the daemon from a shell that has it. It asks first, then starts again what every lane ran, with that shell's environment.

```console title="~/dev/myapp"
$ devlanes daemon restart
```

If a lane comes back failed, run a plain start in its worktree.

## If a plain start should begin from scratch

Set [`start.restore`](/docs/reference/config/user/start/#start-restore) to false in the user config, and a plain start runs the lane's default profile instead of what ran before.

## Related

- [The daemon](../../concepts/daemon.md)
- [Environment and expressions](../../concepts/environment.md)
- [Prepare a box](../remote/prepare-a-box.md)
