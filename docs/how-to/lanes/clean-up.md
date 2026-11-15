---
title: Remove lanes
description: Clean up the lanes that deleted worktrees and renamed projects leave behind, clean one lane by hand, or reset devlanes completely.
kind: how-to
---

Use this after you delete worktrees, rename a project or want a lane to start over with empty data, so that the old lanes stop holding ports, data folders and containers.

## Clean the lanes of deleted worktrees

When you delete a worktree, with `git worktree remove` after a merge or otherwise, the daemon stops its lane and keeps it: its number, its block of ports, its data folder and its own containers stay, and the status shows it as worktree missing. Without `--yes`, `devlanes lane gc` names those lanes and cleans none. Run it again with the flag to clean them, along with lanes whose worktree no longer has the project file.

Each clean removes the lane's own containers, deletes its data folder and frees its number and ports. Its clean tasks and teardown do not run, because the worktree they would run in is gone, so the lane's database in a shared Postgres, for example, stays there. Its logs stay until log retention removes them.

## Clean one lane

`devlanes lane clean` cleans the lane you name, whether its worktree is gone or not, which is also how a lane you keep starts over with empty data.

:::caution
A clean deletes the lane's data folder and its logs, and the project's teardown, when it has one, drops what the lane had in shared containers. devlanes keeps no copy of any of it.
:::

Add `--dry-run` first to see what the clean would stop, run and delete, with sizes. The clean then stops the lane, runs its stop tasks, its clean tasks and its teardown, removes its own containers and deletes its data folder and logs.

```console title="~/dev/myapp"
$ devlanes lane clean 1 --yes
cleaned myapp@1: stopped 0 units
```

This lane was not running, so the clean stopped no units. A worktree that is still there gets a new lane the next time a start or a status runs in it, with a new, empty data folder and possibly other ports. A new lane takes the lowest free number, so the number of a cleaned lane can come back on another worktree. Cleaning a project's last lane also stops its shared containers and keeps their data.

## Have the daemon clean them by itself

Set the project's policy for deleted worktrees to clean, and the daemon cleans each such lane shortly after its folder is gone, as `devlanes lane gc --yes` would.

```yaml devlanes
lanes:
  removed_worktrees: clean
```

A worktree you move with Git's move command keeps its lane, since the daemon waits a moment before it cleans. One deleted while the daemon was not running keeps its lane too, because the daemon can no longer read the setting from the deleted file. `devlanes lane gc --yes` cleans it.

## If you renamed the project

A new name makes a new project with new lanes, and the lanes of the old name stay, still running whatever they ran ([Lanes and projects](../../concepts/lanes-and-projects.md) explains why). Stop them with `devlanes stop` and the old name's lane reference, then run `devlanes lane gc --yes`. Their clean tasks and teardown do not run, since the project file now describes the new project.

## If a data folder is still there

On Windows a folder cannot be deleted while a process holds a file in it, so the clean leaves it and the next `devlanes lane gc --yes` deletes it. [Windows](../../reference/windows.md) lists the related cases.

## If you want to start over completely

`devlanes daemon reset-state --yes` stops every process and empties the daemon's state database. Lanes then get new numbers and ports, their setup and create tasks run again, and pairings with boxes have to be made again. The reset deletes no data folder. New lanes get new, empty ones, and the old folders stay on disk until you delete them ([Files and folders](../../reference/files.md) says where they are).

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Run a folder as two lanes](two-lanes-one-folder.md)
- [Share one database server between lanes](../infra/shared-database.md)
- [Files and folders](../../reference/files.md)
