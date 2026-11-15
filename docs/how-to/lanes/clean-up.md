---
title: Remove lanes
description: Clean up the lanes that deleted worktrees and renamed projects leave behind, clean one lane by hand, or reset devlanes completely.
kind: how-to
---

Use this after you delete worktrees, rename a project or want a lane to start over with empty data, so that the old lanes stop holding ports, data folders and containers.

## List the lanes left behind

When you delete a worktree, with Git or otherwise, the daemon stops its lane and keeps it. Its number, its block of ports and its data folder stay, its own containers keep running, and the status shows it as worktree missing.

```console title="~/dev/myapp"
$ devlanes lane gc
```

Without `--yes` the command names the lanes it would clean and cleans none. Read the list: it covers every project on the machine, and besides deleted worktrees it takes a worktree whose checked-out branch has no project file, or one that names another project. Cleaning such a lane deletes the data you would find again by switching back, so switch the branch back first to keep it off the list.

## Clean them

:::caution
A clean deletes the lane's data folder, its own containers with their volumes, and its run history. A clean with `devlanes lane clean` deletes its logs too. devlanes keeps no copy of any of it.
:::

```console title="~/dev/myapp"
$ devlanes lane gc --yes
```

Each clean frees the lane's number and ports, and its logs stay until log retention removes them. A lane that still runs something is left alone.

The clean tasks and teardown of a lane whose worktree is gone do not run, because there is no folder to run them in. What its setup created outside devlanes therefore stays, such as its database in a shared Postgres. The next lane that gets the same number inherits it unless you drop it by hand.

## Clean one lane

`devlanes lane clean` cleans the lane you name, whether its worktree is gone or not, which is also how a lane you keep starts over with empty data. Look at what it would do first, with sizes.

```console title="~/dev/myapp"
$ devlanes lane clean 1 --dry-run
```

The clean tasks and teardown run only for a lane that has been started from a shell since the daemon started. After a reboot, for example, the dry run reports them as skipped because the lane needs a start from a shell. Start the lane once in its worktree, then clean it, or what its setup created stays behind for the next lane with that number.

```console title="~/dev/myapp"
$ devlanes lane clean 1 --yes
```

The clean stops the lane, runs its stop tasks, clean tasks and teardown when the lane can run them, removes its own containers and volumes, deletes its data folder, logs and history, and frees its number and ports. Cleaning a project's last lane, when no other lane of the project exists, running or not, also stops its shared containers and keeps their data.

A worktree that is still there gets a new lane at its next start or status, with an empty data folder and possibly other ports. A new lane takes the lowest free number, so a cleaned number can come back on another worktree.

## Have the daemon clean them by itself

With this setting the daemon cleans a lane once its worktree folder is gone, as `devlanes lane gc --yes` would, with the same skipped tasks.

```yaml devlanes
lanes:
  removed_worktrees: clean
```

Each worktree follows the setting in its own copy of the project file, so commit it on the main branch before you create the worktrees it should apply to. A worktree deleted while the daemon was not running keeps its lane, because the daemon can no longer read the setting there, and the command above cleans it.

A lane that runs nothing is cleaned a few moments after its folder goes, which lets Git's worktree move keep it. A running lane is cleaned as soon as its folder disappears, and a move makes the folder disappear for a moment. Stop a lane before you move its worktree.

## If you renamed the project

A new name makes a new project with new lanes, and the lanes of the old name stay, still running whatever they ran ([Lanes and projects](../../concepts/lanes-and-projects.md) explains why). Stop them with `devlanes stop` and the old name's lane reference, then clean them with `devlanes lane gc --yes`. Their clean tasks and teardown do not run, since the project file now describes the new project.

## If a data folder is still there

On Windows a folder cannot be deleted while a process holds a file in it, so the clean leaves it and the next `devlanes lane gc` deletes it. [Windows](../../reference/windows.md) lists the related cases.

## If you want to start over completely

```console
$ devlanes daemon reset-state --yes
```

The reset stops every process and empties the daemon's state database. Lanes are numbered again as worktrees come back, so a worktree can get another lane's number, ports and names, and setup and create tasks run again. Boxes and their pairings are forgotten. Containers and their volumes stay as they are, and so do the data folders ([Files and folders](../../reference/files.md) says where they are), so clean the lanes first if you want those gone.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Run a folder as two lanes](two-lanes-one-folder.md)
- [Share one database server between lanes](../infra/shared-database.md)
- [Files and folders](../../reference/files.md)
