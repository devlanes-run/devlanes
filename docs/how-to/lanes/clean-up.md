---
title: Remove lanes
description: Clean up the lanes that deleted worktrees and renamed projects leave behind, clean one lane by hand, or reset devlanes completely.
kind: how-to
---

Use this after you delete worktrees, rename a project or want a lane to start over with empty data.

:::caution
A clean deletes the lane's data folder and its own containers with their volumes. devlanes keeps no copy.
:::

## Clean the lanes of deleted worktrees

When you delete a worktree, its lane stops and stays, holding its ports and data. This lists such lanes in every project on the machine, then cleans them.

```console title="~/dev/myapp"
$ devlanes lane gc
$ devlanes lane gc --yes
```

The list also takes a worktree whose branch has no project file, so switch it back first to keep its lane. A lane whose worktree is gone cannot run its teardown, so drop what its setup made outside devlanes, such as its database in a shared Postgres, by hand.

## Clean one lane

Check first with `--dry-run`, which lists what the clean would stop, run and delete. Then clean it.

```console title="~/dev/myapp"
$ devlanes lane clean 1 --dry-run
$ devlanes lane clean 1 --yes
```

A worktree that is still there gets a new, empty lane at its next start.

## Have the daemon clean them by itself

With this in the project file, the daemon cleans a lane shortly after its worktree is deleted. Stop a lane before you move its worktree.

```yaml devlanes
lanes:
  removed_worktrees: clean
```

## If you renamed the project

The lanes of the old name stay and keep running. Stop them, then clean them with `devlanes lane gc --yes`.

## If you want to start over completely

This stops everything and forgets every lane and box. Containers, volumes and data folders stay, so clean the lanes first if you want those gone.

```console
$ devlanes daemon reset-state --yes
```

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Run a folder as two lanes](two-lanes-one-folder.md)
- [Share one database server between lanes](../infra/shared-database.md)
- [Files and folders](../../reference/files.md)
